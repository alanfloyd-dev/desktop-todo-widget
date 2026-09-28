//! Managed-installation launch admission (Maintenance Protocol 1).
//!
//! Runs before every diagnostics write and before the early rendering-backend
//! database read: while a maintenance transaction holds the gate or durable
//! state is active, no business initialization may observe half-mutated
//! installation or data state (docs/application-lifecycle.md §11).
//!
//! Classification is check-only. This module never rewrites, repairs or
//! adopts a receipt — repair belongs to the maintenance helper. A missing
//! receipt is an unmanaged/legacy launch; a present-but-unacceptable receipt
//! is a managed-admission failure and refuses normal startup instead of
//! silently degrading.

use desktop_todo_maintenance::{
    lock::{self, AppLease, Gate},
    paths::{self, Paths},
    receipt::{Lifecycle, Receipt},
    Error, ErrorKind, HELPER_EXE,
};
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// Why a normal launch was refused. Variants mirror the structured admission
/// semantics; [`AdmissionBlock::user_message`] carries the user-facing text.
#[derive(Debug)]
pub enum AdmissionBlock {
    /// A maintenance transaction or another admission currently holds the
    /// exclusion. Retry after it finishes.
    MaintenanceLockHeld(String),
    /// Durable maintenance state (active uninstall journal) exists; finish the
    /// transaction with the helper first.
    DurableMaintenanceState(String),
    /// The canonical installation contains the maintenance helper but no
    /// receipt: broken managed state, never treated as legacy.
    ManagedReceiptMissing,
    /// Receipt bytes exist but cannot be parsed or fail structural policy.
    ReceiptMalformed(String),
    /// Receipt schema (or updater protocol) is newer than this binary.
    UnsupportedReceiptSchema,
    /// Receipt appId does not name this product.
    AppIdMismatch,
    /// Receipt install root does not match the canonical installation root.
    InstallRootMismatch,
    /// Receipt data root does not match the canonical data root.
    DataRootMismatch,
    /// The installation state is unsafe or cannot be inspected safely.
    UnsafeInstallation(String),
    /// Receipt lifecycle is transitional (Installing/Updating/Uninstalling/
    /// RecoveryRequired); an ordinary launch must not behave as a normal
    /// instance while it is.
    LifecycleBusy(String),
}

impl AdmissionBlock {
    fn user_message(&self) -> String {
        let detail = match self {
            Self::MaintenanceLockHeld(detail)
            | Self::DurableMaintenanceState(detail)
            | Self::ReceiptMalformed(detail) => detail.clone(),
            _ => String::new(),
        };
        let text = match self {
            Self::MaintenanceLockHeld(_) => "desktop-todo-widget cannot start right now.\n\n\
                 A maintenance operation on this installation is already running. \
                 Wait for it to finish, then start the app again."
                .into(),
            Self::DurableMaintenanceState(_) => {
                "desktop-todo-widget cannot start because a previous maintenance \
                 operation did not finish.\n\n\
                 Run desktop-todo-maintenance.exe from the installation folder \
                 to complete or undo it."
                    .into()
            }
            Self::ManagedReceiptMissing => {
                "desktop-todo-widget cannot start because the installation folder \
                 contains the maintenance helper but no installation receipt.\n\n\
                 Run the trusted installer or the maintenance helper to repair \
                 the installation."
                    .into()
            }
            Self::ReceiptMalformed(_) => {
                "desktop-todo-widget cannot start because the installation receipt \
                 is damaged.\n\n\
                 Run desktop-todo-maintenance.exe from the installation folder to \
                 repair or uninstall. Your data is not touched by this check."
                    .into()
            }
            Self::UnsupportedReceiptSchema => {
                "desktop-todo-widget cannot start because this installation was \
                 created by a newer version of the product.\n\n\
                 Update the app, or repair it with the installed maintenance helper."
                    .into()
            }
            Self::AppIdMismatch => {
                "desktop-todo-widget cannot start because the installation receipt \
                 belongs to a different application.\n\n\
                 The installation state is preserved; nothing was modified."
                    .into()
            }
            Self::InstallRootMismatch => {
                "desktop-todo-widget cannot start because the installation receipt \
                 does not match this installation folder.\n\n\
                 Run the trusted installer or the maintenance helper."
                    .into()
            }
            Self::DataRootMismatch => {
                "desktop-todo-widget cannot start because the installation receipt \
                 does not match the canonical data folder.\n\n\
                 The installation state is preserved; nothing was modified."
                    .into()
            }
            Self::UnsafeInstallation(detail) => {
                format!(
                    "desktop-todo-widget cannot start because the installation \
                         state is unsafe: {detail}\n\n\
                         Run the maintenance helper from the installation folder."
                )
            }
            Self::LifecycleBusy(state) => {
                format!(
                    "desktop-todo-widget cannot start because this installation \
                         is in the {state} state.\n\n\
                         Run desktop-todo-maintenance.exe from the installation \
                         folder to finish or recover the operation."
                )
            }
        };
        if detail.is_empty() {
            text
        } else {
            format!("{text}\n\nDetails: {detail}")
        }
    }
}

/// Which kind of launch admission produced. `PostUpdateProbation` is the
/// frozen helper-authorized probation child (protocol v1 "HealthAck",
/// lifecycle §11): recognized only by its session/nonce environment
/// bindings matching the `Updating` receipt's active session, the durable
/// health nonce, and the journaled probation process identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupContext {
    /// Receipt present, fully acceptable, and this process runs from the
    /// canonical installation root.
    Managed,
    /// No receipt and no managed runtime evidence: legacy/custom installs,
    /// portable copies, development and test launches without a managed
    /// installation. Fully functional; never claims Protocol 1 management.
    Unmanaged,
    /// A valid canonical receipt exists for the installation, but this
    /// process is not the installed runtime (developer loop, test harness,
    /// or a portable copy beside a managed install). Runs against the shared
    /// data root and holds the shared lease, but is not the managed runtime.
    Development,
    /// The helper-launched probation child of an in-flight update: admitted
    /// to initialize and write HealthAck, and nothing else. Holds the shared
    /// lease like any instance; never runs conflicting maintenance.
    PostUpdateProbation,
}

impl StartupContext {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Unmanaged => "unmanaged",
            Self::Development => "development",
            Self::PostUpdateProbation => "post-update-probation",
        }
    }
}

/// The validated probation bindings of a `PostUpdateProbation` launch, kept
/// on the [`Admission`] so the HealthAck write after core initialization
/// needs no re-derivation.
#[derive(Debug, Clone)]
pub struct ProbationFacts {
    pub session_id: String,
    pub installation_id: String,
    pub target_version: String,
    pub nonce: String,
}

/// Long-lived admission record; holds the shared application lease so a
/// maintenance transaction cannot start while any instance is running.
pub struct Admission {
    pub context: StartupContext,
    pub installation_id: Option<String>,
    /// The install root this admission was resolved against (canonical for
    /// production; the QA sandbox under the maintenance-qa feature).
    pub(crate) install_root: PathBuf,
    /// Validated probation bindings; `Some` only in the
    /// `PostUpdateProbation` context.
    pub(crate) probation: Option<ProbationFacts>,
    /// Set while an uninstaller handoff is in flight, so a double click can
    /// never spawn two helpers. Reset when the handoff is refused so the
    /// entry stays usable.
    handoff_started: AtomicBool,
    /// `None` only in the degraded path where canonical roots could not be
    /// resolved at all (pre-maintenance behaviour, logged).
    _lease: Option<AppLease>,
}

impl Admission {
    fn begin_handoff(&self) -> Result<PathBuf, &'static str> {
        // Only a managed installation may launch the uninstaller from here:
        // an unmanaged or development copy must not pretend to own the
        // managed uninstall capability, and adopting one is never automatic.
        if self.context != StartupContext::Managed {
            return Err("not-managed");
        }
        if self.handoff_started.swap(true, Ordering::SeqCst) {
            return Err("already-started");
        }
        // The frontend supplies no path at all: the helper identity is
        // compiled policy (canonical install root + fixed name), validated as
        // a regular, unlinked, non-reparse file by the shared path policy.
        // The helper itself revalidates everything it is told by this file —
        // this check only keeps the obvious refusal local and instant.
        let resolved = (|| {
            let helper = self.install_root.join(HELPER_EXE);
            paths::open_regular(&helper).map_err(|_| "helper-unavailable")?;
            Ok(helper)
        })();
        if resolved.is_err() {
            // Refusals keep the entry usable; only a successful handoff stays
            // latched (the app is about to exit anyway).
            self.handoff_started.store(false, Ordering::SeqCst);
        }
        resolved
    }
}

/// The full uninstall handoff: validate the managed helper, spawn it without
/// waiting for the transaction, then request the app's normal exit so the
/// shared lease drops and the helper proceeds. `spawn_helper` and
/// `request_exit` are the only side effects; tests supply their own.
pub fn perform_uninstall_handoff(
    admission: &Admission,
    spawn_helper: impl FnOnce(&Path) -> Result<(), String>,
    request_exit: impl FnOnce(),
) -> Result<(), &'static str> {
    let helper = admission.begin_handoff()?;
    if let Err(detail) = spawn_helper(&helper) {
        // The app must stay alive when the helper could not start.
        admission.handoff_started.store(false, Ordering::SeqCst);
        eprintln!("[maintenance-admission] helper launch failed: {detail}");
        return Err("launch-failed");
    }
    request_exit();
    Ok(())
}

fn spawn_helper_process(helper: &Path) -> Result<(), String> {
    // CREATE_NO_WINDOW: the helper is a GUI-subsystem executable; the flag
    // mirrors what the helper itself uses when it spawns its session runner.
    std::process::Command::new(helper)
        .arg("--uninstall")
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_child| ())
        .map_err(|error| error.to_string())
}

/// Reads the admission state for the frontend. Rust is the source of truth:
/// the frontend never checks receipts, executable locations or maintenance
/// files, and the receipt itself is never serialized to the webview.
#[tauri::command]
pub fn maintenance_admission_state(admission: tauri::State<'_, Admission>) -> AdmissionView {
    AdmissionView {
        mode: admission.context.as_str(),
        #[cfg(feature = "maintenance-qa")]
        qa_handoff: std::env::var("DTW_MAINTENANCE_QA_HANDOFF").is_ok(),
    }
}

/// Launches the canonical maintenance helper's uninstall transaction and then
/// exits the app. Refusals keep the app alive and return a stable code the
/// frontend maps to localized copy.
#[tauri::command]
pub fn start_uninstall(
    app: tauri::AppHandle,
    admission: tauri::State<'_, Admission>,
) -> Result<(), &'static str> {
    let exit_app = app.clone();
    perform_uninstall_handoff(&admission, spawn_helper_process, move || {
        crate::product_window::quit_app(exit_app)
    })
}

/// The structured admission state the frontend may read.
#[derive(Debug, serde::Serialize)]
pub struct AdmissionView {
    pub mode: &'static str,
    /// QA-only trigger field (maintenance-qa builds only): tells the QA
    /// frontend hook to drive the real `start_uninstall` command. Production
    /// builds do not compile the field at all, so the shipped frontend hook
    /// condition can never be true.
    #[cfg(feature = "maintenance-qa")]
    pub qa_handoff: bool,
}

fn block_lock(error: Error) -> AdmissionBlock {
    AdmissionBlock::MaintenanceLockHeld(error.to_string())
}

fn block_receipt(error: Error) -> AdmissionBlock {
    match error.kind {
        ErrorKind::ReceiptMalformed => AdmissionBlock::ReceiptMalformed(error.detail),
        ErrorKind::UnsupportedReceiptSchema => AdmissionBlock::UnsupportedReceiptSchema,
        ErrorKind::AppIdMismatch => AdmissionBlock::AppIdMismatch,
        ErrorKind::InstallRootMismatch => AdmissionBlock::InstallRootMismatch,
        ErrorKind::DataRootMismatch => AdmissionBlock::DataRootMismatch,
        other => AdmissionBlock::UnsafeInstallation(format!("{other:?}: {}", error.detail)),
    }
}

/// The result of one launch classification. `RecoverViaHelper` is the
/// frozen single recovery entrypoint (protocol v1 Q14): a normal launch
/// whose admission observed receipt `Updating` plus a valid update journal
/// routes the canonical maintenance helper's recovery executable and exits
/// — before any database open or UI work.
pub enum LaunchOutcome {
    Admitted(Admission),
    RecoverViaHelper { session_id: String, helper: PathBuf },
}
impl std::fmt::Debug for LaunchOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admitted(admission) => {
                write!(f, "Admitted({:?})", admission.context)
            }
            Self::RecoverViaHelper { session_id, helper } => f
                .debug_struct("RecoverViaHelper")
                .field("session_id", session_id)
                .field("helper", helper)
                .finish(),
        }
    }
}

/// Snapshot of the frozen probation environment bindings, taken from (and
/// cleared out of) the process environment before anything else runs — the
/// child must not forward them to descendants, and leftover values must
/// never leak into any spawned process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbationEnv {
    pub session_id: String,
    pub nonce: String,
}

/// Read and clear the probation bindings. Values are only meaningful when
/// the fixed-purpose launch mode matches exactly; any other combination is
/// treated as no probation launch (the values are cleared regardless, and
/// an `Updating`-state launch still fails closed through the ordinary
/// admission rules).
pub fn take_probation_env() -> Option<ProbationEnv> {
    use desktop_todo_maintenance::probation::{
        ENV_LAUNCH_MODE, ENV_NONCE, ENV_SESSION_ID, LAUNCH_MODE_PROBATION,
    };
    let mode = std::env::var(ENV_LAUNCH_MODE).ok();
    let session_id = std::env::var(ENV_SESSION_ID).ok();
    let nonce = std::env::var(ENV_NONCE).ok();
    std::env::remove_var(ENV_LAUNCH_MODE);
    std::env::remove_var(ENV_SESSION_ID);
    std::env::remove_var(ENV_NONCE);
    if mode.as_deref() != Some(LAUNCH_MODE_PROBATION) {
        return None;
    }
    let (session_id, nonce) = (session_id?, nonce?);
    if session_id.is_empty() || nonce.is_empty() {
        return None;
    }
    Some(ProbationEnv { session_id, nonce })
}

/// The helper-launched probation child (protocol v1 "HealthAck",
/// lifecycle §10 step 2): no admission gate is taken — the maintenance
/// transaction holds the gate for the whole probation, and the child only
/// ever takes the ordinary shared lease. Every binding is checked against
/// independently derived state; anything else refuses.
fn classify_probation(
    paths: &Paths,
    env: &ProbationEnv,
    own_image: Option<&Path>,
) -> Result<Admission, AdmissionBlock> {
    use std::os::windows::fs::MetadataExt;
    if desktop_todo_maintenance::receipt::validate_uuid(&env.session_id).is_err() {
        return Err(AdmissionBlock::UnsafeInstallation(
            "probation session id is not a canonical UUID".into(),
        ));
    }
    let receipt = Receipt::load(paths).map_err(block_receipt)?.ok_or(
        AdmissionBlock::UnsafeInstallation("probation launch without an installation".into()),
    )?;
    if receipt.lifecycle_state != Lifecycle::Updating {
        return Err(AdmissionBlock::LifecycleBusy(format!(
            "{:?}",
            receipt.lifecycle_state
        )));
    }
    if receipt.maintenance.active_session_id.as_deref() != Some(env.session_id.as_str()) {
        return Err(AdmissionBlock::UnsafeInstallation(
            "probation session does not match the active update session".into(),
        ));
    }
    let dir = paths
        .state()
        .join("updates")
        .join("sessions")
        .join(&env.session_id);
    let envelope = desktop_todo_maintenance::update_session::load_update_session(&dir)
        .map_err(|e| {
            AdmissionBlock::UnsafeInstallation(format!("update journal unusable: {e}"))
        })?;
    if envelope.session_id != env.session_id {
        return Err(AdmissionBlock::UnsafeInstallation(
            "update journal does not bind the probation session".into(),
        ));
    }
    // The nonce binding: canonical form, exact durable match.
    let durable = envelope.health_nonce.ok_or_else(|| {
        AdmissionBlock::UnsafeInstallation("no durable health nonce for the session".into())
    })?;
    if desktop_todo_maintenance::probation::canonical_nonce_bytes(&durable).is_err()
        || desktop_todo_maintenance::probation::canonical_nonce_bytes(&env.nonce).is_err()
        || durable != env.nonce
    {
        return Err(AdmissionBlock::UnsafeInstallation(
            "probation nonce does not match the durable session nonce".into(),
        ));
    }
    // This process must be the installed main executable at the canonical
    // root, and must be the exact journaled probation instance. (The image
    // is injectable for deterministic tests; production derives it from the
    // running executable.)
    let image = match own_image {
        Some(image) => image.to_path_buf(),
        None => std::env::current_exe()
            .map_err(|e| AdmissionBlock::UnsafeInstallation(format!("image unresolved: {e}")))?,
    };
    if !paths::equal(&image, &paths.install().join(desktop_todo_maintenance::MAIN_EXE)) {
        return Err(AdmissionBlock::UnsafeInstallation(
            "the probation child must run from the installed main executable".into(),
        ));
    }
    let journaled = envelope.probation_process.ok_or_else(|| {
        AdmissionBlock::UnsafeInstallation(
            "no probation launch is journaled for this session".into(),
        )
    })?;
    let own_created: u64 = own_creation_filetime();
    if journaled.pid != std::process::id()
        || journaled.process_created_at != own_created.to_string()
        || !paths::equal(&journaled.image_path, &image)
    {
        return Err(AdmissionBlock::UnsafeInstallation(
            "this process is not the journaled probation launch instance".into(),
        ));
    }
    // The installed main executable must currently hold the signed target
    // bytes (fresh open, exact size and hash from the journal's signed
    // facts).
    let target = envelope
        .resources
        .iter()
        .find(|entry| {
            desktop_todo_update_core::InstallIdentity::from_manifest_text(&entry.identity)
                == Some(desktop_todo_update_core::InstallIdentity::MainExecutable)
        })
        .ok_or_else(|| {
            AdmissionBlock::UnsafeInstallation("journal has no main executable fact".into())
        })?;
    let metadata = std::fs::metadata(paths.install().join(desktop_todo_maintenance::MAIN_EXE))
        .map_err(|e| AdmissionBlock::UnsafeInstallation(format!("installed main unreadable: {e}")))?;
    if metadata.file_attributes() & 0x0400 != 0 {
        return Err(AdmissionBlock::UnsafeInstallation(
            "the installed main executable passes through a reparse point".into(),
        ));
    }
    let mut file = paths::open_regular(&paths.install().join(desktop_todo_maintenance::MAIN_EXE))
        .map_err(|e| AdmissionBlock::UnsafeInstallation(format!("installed main unreadable: {e}")))?;
    let actual = desktop_todo_maintenance::probation::hash_file_facts(&mut file);
    if actual.0 != target.new_size || actual.1 != target.new_sha256 {
        return Err(AdmissionBlock::UnsafeInstallation(
            "the installed main executable does not hold the signed target bytes".into(),
        ));
    }
    // Everything binds: admit as the probation child on the ordinary
    // shared lease.
    let lease = lock::shared_app(paths).map_err(block_lock)?;
    Ok(Admission {
        context: StartupContext::PostUpdateProbation,
        installation_id: Some(receipt.installation_id.clone()),
        install_root: paths.install().to_path_buf(),
        probation: Some(ProbationFacts {
            session_id: env.session_id.clone(),
            installation_id: receipt.installation_id.clone(),
            target_version: envelope.to_version.clone(),
            nonce: env.nonce.clone(),
        }),
        handoff_started: AtomicBool::new(false),
        _lease: Some(lease),
    })
}

fn own_creation_filetime() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user)
            .expect("own process times are always queryable");
        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64
    }
}

/// The frozen Q14 recovery route: receipt `Updating` with a valid update
/// journal routes the canonical helper's `--recover` execution and exits
/// without opening the database or UI. An unusable journal fails closed
/// (recovery-required semantics: explicit repair, never a guessed launch).
fn recover_route(paths: &Paths, receipt: &Receipt) -> Result<LaunchOutcome, AdmissionBlock> {
    let Some(session_id) = receipt.maintenance.active_session_id.clone() else {
        return Err(AdmissionBlock::LifecycleBusy("Updating".into()));
    };
    let dir = paths
        .state()
        .join("updates")
        .join("sessions")
        .join(&session_id);
    if desktop_todo_maintenance::update_session::load_update_session(&dir).is_err() {
        return Err(AdmissionBlock::UnsafeInstallation(
            "the active update journal is missing or unreadable; the installation \
             requires explicit recovery"
                .into(),
        ));
    }
    let helper = paths.install().join(HELPER_EXE);
    paths::open_regular(&helper).map_err(|e| {
        AdmissionBlock::UnsafeInstallation(format!(
            "the maintenance recovery executable is unavailable: {e}"
        ))
    })?;
    Ok(LaunchOutcome::RecoverViaHelper {
        session_id,
        helper,
    })
}

/// Classifies one launch. `process_image_dir` is the directory of the running
/// executable (`None` when it cannot be determined). `probation` is the
/// taken-and-cleared environment snapshot (`None` for an ordinary launch).
/// Tests call this directly with sandbox paths; the binary entry uses
/// [`admit_or_report`].
pub fn classify_launch(
    paths: &Paths,
    process_image_dir: Option<&Path>,
    probation: Option<&ProbationEnv>,
) -> Result<LaunchOutcome, AdmissionBlock> {
    classify_launch_with_image(paths, process_image_dir, probation, None)
}

/// [`classify_launch`] with the running image injectable (deterministic
/// probation tests; production derives it from the running executable).
pub fn classify_launch_with_image(
    paths: &Paths,
    process_image_dir: Option<&Path>,
    probation: Option<&ProbationEnv>,
    own_image: Option<&Path>,
) -> Result<LaunchOutcome, AdmissionBlock> {
    // The probation child never takes the admission gate: the maintenance
    // transaction holds it for the whole probation (frozen lease
    // choreography), and the child only ever needs the shared lease.
    if let Some(env) = probation {
        return classify_probation(paths, env, own_image).map(LaunchOutcome::Admitted);
    }
    // Short admission gate: held only while classifying durable state, never
    // for the session. Concurrent launches pass sequentially; a running
    // maintenance helper holds the same gate and excludes this check.
    let _gate = Gate::acquire(paths).map_err(block_lock)?;
    if paths.state().join("active-uninstall.json").exists() {
        return Err(AdmissionBlock::DurableMaintenanceState(
            "active uninstall journal".into(),
        ));
    }
    // Ok(None) = receipt missing (unmanaged). Err = a receipt claims this
    // installation but is unacceptable: fail closed, never degrade to
    // unmanaged. `Receipt::load` validates schema, appId, both canonical
    // roots, UUIDs, timestamps and the compiled resource-identity policy.
    let receipt = Receipt::load(paths).map_err(block_receipt)?;
    if let Some(receipt) = &receipt {
        if receipt.lifecycle_state == Lifecycle::Updating {
            // The Q14 single recovery entrypoint — before any lease is
            // taken (this process exits; the recovery helper needs the
            // exclusive lease).
            return recover_route(paths, receipt);
        }
    }
    // The lease is taken under the gate and held until process exit, so a
    // later uninstall sees every running instance.
    let lease = lock::shared_app(paths).map_err(block_lock)?;
    match receipt {
        Some(receipt) => {
            if receipt.lifecycle_state != Lifecycle::Installed {
                return Err(AdmissionBlock::LifecycleBusy(format!(
                    "{:?}",
                    receipt.lifecycle_state
                )));
            }
            // Entering Managed additionally requires the process to run from
            // the canonical installation root. Anything else is the dev/test
            // loop or a portable copy: allowed, but never the managed runtime.
            let context = match process_image_dir {
                Some(dir) if paths::equal(dir, paths.install()) => StartupContext::Managed,
                _ => StartupContext::Development,
            };
            Ok(LaunchOutcome::Admitted(Admission {
                context,
                installation_id: Some(receipt.installation_id.clone()),
                install_root: paths.install().to_path_buf(),
                probation: None,
                handoff_started: AtomicBool::new(false),
                _lease: Some(lease),
            }))
        }
        None => {
            // Helper present without a receipt is broken managed state, not a
            // legacy install (v1.1 payloads never contained the helper).
            if paths.install().join(HELPER_EXE).exists() {
                return Err(AdmissionBlock::ManagedReceiptMissing);
            }
            Ok(LaunchOutcome::Admitted(Admission {
                context: StartupContext::Unmanaged,
                installation_id: None,
                install_root: paths.install().to_path_buf(),
                probation: None,
                handoff_started: AtomicBool::new(false),
                _lease: Some(lease),
            }))
        }
    }
}

/// Test-only admitted-or-blocked wrapper over [`classify_launch`]: the
/// existing deterministic admission fixtures classify ordinary launches
/// without probation input.
#[cfg(test)]
pub(crate) fn admit(
    paths: &Paths,
    process_image_dir: Option<&Path>,
) -> Result<Admission, AdmissionBlock> {
    match classify_launch(paths, process_image_dir, None)? {
        LaunchOutcome::Admitted(admission) => Ok(admission),
        LaunchOutcome::RecoverViaHelper { .. } => Err(AdmissionBlock::LifecycleBusy(
            "Updating".into(),
        )),
    }
}

fn process_image_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()))
}

fn show_block_dialog(message: &str) {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            &HSTRING::from("desktop-todo-widget"),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

/// Resolves the canonical roots, classifies this launch and reports refusal
/// through the minimal safe surface (console in debug builds, a native dialog
/// everywhere, then a non-zero exit). The frozen Q14 recovery route spawns
/// the canonical helper's `--recover` execution — identity validated exactly
/// like the uninstall handoff — and exits without opening the database or
/// UI. If the canonical roots themselves cannot be resolved the launch
/// degrades to pre-maintenance behaviour without a lease, logged: on such a
/// machine the helper could not run either, and the Tauri data-dir
/// resolution in `setup` still guards the database path.
pub fn admit_or_report() -> Admission {
    // The probation bindings are read and cleared before anything else can
    // observe them (the child never forwards them to descendants).
    let probation = take_probation_env();
    let paths = match Paths::resolve() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!(
                "[maintenance-admission] canonical roots unresolved, continuing \
                 without a maintenance lease: {error}"
            );
            return Admission {
                context: StartupContext::Unmanaged,
                installation_id: None,
                // Roots could not be resolved; this context is never Managed,
                // so the handoff refuses before ever touching this path.
                install_root: PathBuf::new(),
                probation: None,
                handoff_started: AtomicBool::new(false),
                _lease: None,
            };
        }
    };
    match classify_launch(&paths, process_image_dir().as_deref(), probation.as_ref()) {
        Ok(LaunchOutcome::Admitted(admission)) => admission,
        Ok(LaunchOutcome::RecoverViaHelper { session_id, helper }) => {
            eprintln!(
                "[maintenance-admission] Updating receipt with a valid journal: \
                 routing maintenance recovery for session {session_id}"
            );
            let spawned = std::process::Command::new(&helper)
                .arg("--recover")
                .arg("--session-id")
                .arg(&session_id)
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn();
            if spawned.is_err() {
                show_block_dialog(
                    "desktop-todo-widget cannot start because a previous update \
                     did not finish and the maintenance helper could not be started.\n\n\
                     Run desktop-todo-maintenance.exe from the installation folder \
                     to finish or undo the update.",
                );
                std::process::exit(3);
            }
            std::process::exit(0);
        }
        Err(block) => {
            eprintln!("[maintenance-admission] launch refused: {block:?}");
            show_block_dialog(&block.user_message());
            std::process::exit(3);
        }
    }
}

/// The probation child's HealthAck: called once core initialization has
/// succeeded (backend, database, settings, tray, main window). The marker
/// is the only proof of health the maintenance helper accepts; a failure to
/// write it means the update cannot be proven healthy, so the process exits
/// (the helper observes the exit and rolls back) after recording the cause.
pub fn acknowledge_update_health(admission: &Admission, diagnostics: &crate::qa_diagnostics::QaDiagnostics) {
    let Some(facts) = &admission.probation else {
        return; // ordinary launch: never writes health (frozen)
    };
    let outcome = (|| -> Result<(), Error> {
        use desktop_todo_maintenance::probation;
        let ack = probation::build_current_health_ack(
            &facts.session_id,
            &facts.installation_id,
            &facts.target_version,
            &facts.nonce,
        )?;
        let paths = Paths::resolve()?;
        probation::write_health_ack(&paths, &ack)
    })();
    match outcome {
        Ok(()) => {
            diagnostics
                .record("post-update probation health acknowledged (durable marker written)");
        }
        Err(error) => {
            diagnostics.record(format!(
                "[probation-fatal] the health marker could not be written: {error}; \
                 exiting so the update rolls back"
            ));
            // Never fake health by continuing: an unacknowledged probation
            // is a rollback. No dialog — the recovery path owns the UX.
            std::process::exit(6);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_todo_maintenance::receipt::FileRecord;
    use desktop_todo_maintenance::resources::Resource;
    use desktop_todo_maintenance::{APP_ID, HELPER_EXE, MAIN_EXE};
    use serde_json::Value;
    use std::fs;

    fn fixture() -> (Paths, PathBuf) {
        let id = uuid::Uuid::new_v4().to_string();
        // src-tauri/target/admission-tests/<uuid> — the repository build
        // directory, never the user's real APPDATA or installation.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("admission-tests")
            .join(&id);
        let paths = Paths::sandbox(root, &id);
        paths::create_dir(paths.install()).unwrap();
        // A process location that is not the (sandbox) installation root.
        let dev_dir = paths.install().parent().unwrap().join("dev-bin");
        paths::create_dir(&dev_dir).unwrap();
        (paths, dev_dir)
    }

    fn installed_receipt(paths: &Paths) -> String {
        let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
            .map(|identity| FileRecord {
                identity,
                version: "1.1.0".into(),
                size: 2,
                sha256: "a".repeat(64),
            })
            .to_vec();
        let mut receipt =
            desktop_todo_maintenance::receipt::Receipt::new(paths, "1.1.0", files, false);
        receipt.lifecycle_state = Lifecycle::Installed;
        receipt.save(paths).unwrap();
        receipt.installation_id.clone()
    }

    fn rewrite_receipt(paths: &Paths, mutate: impl FnOnce(&mut Value)) {
        let bytes = fs::read(paths.receipt()).unwrap();
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        mutate(&mut value);
        fs::write(paths.receipt(), serde_json::to_vec(&value).unwrap()).unwrap();
    }

    #[test]
    fn missing_receipt_is_unmanaged_and_gate_is_released() {
        let (p, dev) = fixture();
        let first = admit(&p, Some(&dev)).unwrap();
        assert_eq!(first.context, StartupContext::Unmanaged);
        assert_eq!(first.installation_id, None);
        drop(first);
        // Sequential launches pass: the admission gate is short-lived.
        let second = admit(&p, Some(&dev)).unwrap();
        assert_eq!(second.context, StartupContext::Unmanaged);
    }

    #[test]
    fn valid_installed_receipt_admits_managed_runtime_from_install_root() {
        let (p, _dev) = fixture();
        let id = installed_receipt(&p);
        let managed_dir = p.install().to_path_buf();
        let admission = admit(&p, Some(managed_dir.as_path())).unwrap();
        assert_eq!(admission.context, StartupContext::Managed);
        assert_eq!(admission.installation_id.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn valid_receipt_outside_install_root_is_development() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        let admission = admit(&p, Some(&dev)).unwrap();
        assert_eq!(admission.context, StartupContext::Development);
        assert!(admission.installation_id.is_some());
    }

    #[test]
    fn malformed_receipt_fails_closed_not_unmanaged() {
        let (p, dev) = fixture();
        fs::write(p.receipt(), b"{ not json").unwrap();
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::ReceiptMalformed(_))
        ));
    }

    #[test]
    fn wrong_app_id_fails_closed() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        rewrite_receipt(&p, |v| {
            v["appId"] = Value::from("com.other.product");
        });
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::AppIdMismatch)
        ));
    }

    #[test]
    fn wrong_install_root_fails_closed() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        rewrite_receipt(&p, |v| {
            v["installRoot"] = Value::from("C:\\Windows");
        });
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::InstallRootMismatch)
        ));
    }

    #[test]
    fn wrong_data_root_fails_closed() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        rewrite_receipt(&p, |v| {
            v["dataRoot"] = Value::from("C:\\Users\\elsewhere");
        });
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::DataRootMismatch)
        ));
    }

    #[test]
    fn unsupported_schema_fails_closed() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        rewrite_receipt(&p, |v| {
            v["schemaVersion"] = Value::from(2);
        });
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::UnsupportedReceiptSchema)
        ));
    }

    #[test]
    fn transitional_lifecycle_blocks_normal_launch() {
        let (p, dev) = fixture();
        installed_receipt(&p);
        for state in ["Installing", "Updating", "Uninstalling", "RecoveryRequired"] {
            rewrite_receipt(&p, |v| {
                v["lifecycleState"] = Value::from(state);
            });
            assert!(
                matches!(admit(&p, Some(&dev)), Err(AdmissionBlock::LifecycleBusy(_))),
                "state {state} must block normal launch"
            );
        }
    }

    #[test]
    fn helper_without_receipt_is_broken_managed_state() {
        let (p, dev) = fixture();
        fs::write(p.install().join(HELPER_EXE), b"helper").unwrap();
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::ManagedReceiptMissing)
        ));
    }

    #[test]
    fn main_exe_without_receipt_is_legacy_unmanaged() {
        let (p, dev) = fixture();
        fs::write(p.install().join(MAIN_EXE), b"legacy runtime").unwrap();
        let admission = admit(&p, Some(&dev)).unwrap();
        assert_eq!(admission.context, StartupContext::Unmanaged);
    }

    #[test]
    fn held_maintenance_gate_blocks_launch_before_anything_else() {
        let (p, dev) = fixture();
        let gate = Gate::acquire(&p).unwrap();
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::MaintenanceLockHeld(_))
        ));
        drop(gate);
        admit(&p, Some(&dev)).unwrap();
    }

    #[test]
    fn active_uninstall_journal_blocks_launch() {
        let (p, dev) = fixture();
        paths::create_dir(p.state()).unwrap();
        fs::write(p.state().join("active-uninstall.json"), "{}").unwrap();
        assert!(matches!(
            admit(&p, Some(&dev)),
            Err(AdmissionBlock::DurableMaintenanceState(_))
        ));
    }

    #[test]
    fn admission_lease_blocks_exclusive_maintenance_until_exit() {
        let (p, _dev) = fixture();
        let admission = admit(&p, None).unwrap();
        assert!(lock::exclusive_app(&p).is_err());
        drop(admission);
        assert!(lock::exclusive_app(&p).is_ok());
    }

    #[test]
    fn canonical_data_root_matches_legacy_env_derived_path() {
        // v1.1 derived the data root from %APPDATA% + app id and stored
        // alan-desktop.sqlite3 there. The shared resolver must keep resolving
        // exactly that location; nothing migrates.
        let canonical = paths::canonical_data_root().unwrap();
        assert_eq!(canonical.file_name().unwrap(), APP_ID);
        if let Some(roaming) = std::env::var_os("APPDATA") {
            let legacy = PathBuf::from(roaming).join(APP_ID);
            assert!(
                paths::equal(&canonical, &legacy),
                "canonical {canonical:?} != legacy {legacy:?}"
            );
        }
        let db = canonical.join("alan-desktop.sqlite3");
        assert_eq!(db.file_name().unwrap(), "alan-desktop.sqlite3");
    }

    #[test]
    fn uninstall_handoff_requires_managed_admission() {
        let (p, dev) = fixture();
        // Unmanaged: no receipt, no managed runtime evidence (the helper
        // file must not exist here — helper without receipt refuses
        // admission outright as broken managed state).
        let admission = admit(&p, Some(&dev)).unwrap();
        let result = perform_uninstall_handoff(
            &admission,
            |_| panic!("must not spawn"),
            || panic!("must not exit"),
        );
        assert_eq!(result, Err("not-managed"));

        // Development: a valid receipt exists, but the process is not the
        // installed runtime; it must not pretend to own the uninstall entry.
        installed_receipt(&p);
        let admission = admit(&p, Some(&dev)).unwrap();
        assert_eq!(admission.context, StartupContext::Development);
        let result = perform_uninstall_handoff(
            &admission,
            |_| panic!("must not spawn"),
            || panic!("must not exit"),
        );
        assert_eq!(result, Err("not-managed"));
    }

    #[test]
    fn managed_handoff_launches_canonical_helper_then_exits_once() {
        let (p, _dev) = fixture();
        installed_receipt(&p);
        let managed_dir = p.install().to_path_buf();
        let admission = admit(&p, Some(managed_dir.as_path())).unwrap();
        assert_eq!(admission.context, StartupContext::Managed);
        fs::write(p.install().join(HELPER_EXE), b"helper fixture").unwrap();

        let spawns = std::cell::Cell::new(0u32);
        let exits = std::cell::Cell::new(0u32);
        let spawned_path = std::cell::RefCell::new(PathBuf::new());
        let result = perform_uninstall_handoff(
            &admission,
            |helper| {
                // The helper identity comes from compiled policy resolved
                // against this admission's install root; the frontend can
                // never supply one.
                *spawned_path.borrow_mut() = helper.to_path_buf();
                spawns.set(spawns.get() + 1);
                Ok(())
            },
            || exits.set(exits.get() + 1),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(spawns.get(), 1);
        assert_eq!(exits.get(), 1);
        assert_eq!(spawned_path.borrow().clone(), p.install().join(HELPER_EXE));

        // A second handoff is refused without spawning or exiting again.
        let result = perform_uninstall_handoff(
            &admission,
            |_| panic!("must not spawn twice"),
            || panic!("must not exit twice"),
        );
        assert_eq!(result, Err("already-started"));
    }

    #[test]
    fn handoff_failures_keep_the_app_alive_and_the_entry_usable() {
        let (p, _dev) = fixture();
        installed_receipt(&p);
        let managed_dir = p.install().to_path_buf();
        let admission = admit(&p, Some(managed_dir.as_path())).unwrap();

        // Missing helper: refused, and the app must not exit.
        let result = perform_uninstall_handoff(
            &admission,
            |_| panic!("must not spawn"),
            || panic!("must not exit"),
        );
        assert_eq!(result, Err("helper-unavailable"));

        // A directory standing in for the helper is rejected by the same
        // regular-file validation.
        fs::create_dir(p.install().join(HELPER_EXE)).unwrap();
        let result = perform_uninstall_handoff(
            &admission,
            |_| panic!("must not spawn"),
            || panic!("must not exit"),
        );
        assert_eq!(result, Err("helper-unavailable"));

        // Once the helper is really present, a spawn failure still refuses to
        // exit and re-arms the entry for another attempt.
        fs::remove_dir(p.install().join(HELPER_EXE)).unwrap();
        fs::write(p.install().join(HELPER_EXE), b"helper fixture").unwrap();
        let result = perform_uninstall_handoff(
            &admission,
            |_| Err("access is denied".into()),
            || panic!("must not exit"),
        );
        assert_eq!(result, Err("launch-failed"));
        // The failed attempt reset the guard, so the user can retry.
        let result = perform_uninstall_handoff(&admission, |_| Ok(()), || ());
        assert_eq!(result, Ok(()));
    }

    // --------------------------------------------- Phase 2D-B probation + Q14

    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    use desktop_todo_maintenance::update_session::{
        ProcessIdentity, SessionOperation, SessionPhase, SourceInfo, UpdateSessionEnvelope,
    };

    struct UpdateFixture {
        paths: Paths,
        session_id: String,
        installation_id: String,
        nonce: String,
        journaled: ProcessIdentity,
    }

    /// A session at the `LaunchedAwaitingHealth` shape: `Updating` receipt
    /// bound to a durable journal that carries the nonce and this process's
    /// identity as the journaled probation child.
    fn updating_fixture(_tag: &str) -> UpdateFixture {
        let (paths, _dev) = fixture();
        let installation_id = installed_receipt(&paths);
        let mut receipt =
            desktop_todo_maintenance::receipt::Receipt::load(&paths).unwrap().unwrap();
        receipt.lifecycle_state = Lifecycle::Updating;
        receipt.maintenance.active_session_id = Some({
            // The receipt's active session is created below; rewrite after.
            receipt.maintenance.active_session_id.clone().unwrap_or_default()
        });
        let session_id = uuid::Uuid::new_v4().to_string();
        receipt.maintenance.active_session_id = Some(session_id.clone());
        receipt.save(&paths).unwrap();

        let main_bytes = b"signed target main fixture bytes".to_vec();
        let helper_bytes = b"signed target helper fixture bytes".to_vec();
        let sha = |bytes: &[u8]| desktop_todo_update_core::sha256_hex(bytes);
        let journaled = ProcessIdentity {
            pid: std::process::id(),
            process_created_at: own_creation_filetime().to_string(),
            image_path: paths.install().join(MAIN_EXE),
        };
        let envelope = UpdateSessionEnvelope {
            schema_version: 1,
            updater_protocol: 1,
            operation: SessionOperation::Update,
            session_id: session_id.clone(),
            installation_id: installation_id.clone(),
            app_id: desktop_todo_maintenance::APP_ID.to_string(),
            from_version: "1.1.0".to_string(),
            to_version: "1.2.0".to_string(),
            source: SourceInfo {
                discovered_via: "GitHub".to_string(),
                package_url: "https://mirror.invalid/pkg.zip".to_string(),
            },
            manifest_sha256: "a".repeat(64),
            package_sha256: "b".repeat(64),
            package_size: 16,
            phase: SessionPhase::HandedOff,
            generation: 5,
            install_root: paths.install().to_path_buf(),
            staging_root: paths
                .state()
                .join("updates")
                .join("sessions")
                .join(&session_id),
            created_at: "2026-10-01T00:00:00Z".to_string(),
            updated_at: "2026-10-01T00:00:01Z".to_string(),
            parent_process: ProcessIdentity {
                pid: std::process::id(),
                process_created_at: own_creation_filetime().to_string(),
                image_path: paths.install().join(MAIN_EXE),
            },
            resources: vec![
                desktop_todo_maintenance::update_session::SessionResource {
                    identity: "mainExecutable".to_string(),
                    new_sha256: sha(&main_bytes),
                    new_size: main_bytes.len() as u64,
                    old_present: true,
                    backup_slot: format!(
                        r".maintenance\{session_id}\mainExecutable.backup"
                    ),
                    old_sha256: None,
                    old_size: None,
                    intent: None,
                    completed: None,
                },
                desktop_todo_maintenance::update_session::SessionResource {
                    identity: "maintenanceHelper".to_string(),
                    new_sha256: sha(&helper_bytes),
                    new_size: helper_bytes.len() as u64,
                    old_present: true,
                    backup_slot: format!(
                        r".maintenance\{session_id}\maintenanceHelper.backup"
                    ),
                    old_sha256: None,
                    old_size: None,
                    intent: None,
                    completed: None,
                },
            ],
            probation_process: Some(journaled.clone()),
            health_nonce: Some(B64.encode([7u8; 32])),
            previous_receipt: None,
            previous_integration: None,
            accepted_health: None,
            commit_intent: None,
            last_error: None,
        };
        let dir = paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&session_id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.json"),
            serde_json::to_vec_pretty(&envelope).unwrap(),
        )
        .unwrap();
        fs::write(paths.install().join(MAIN_EXE), &main_bytes).unwrap();
        UpdateFixture {
            paths,
            session_id,
            installation_id,
            nonce: B64.encode([7u8; 32]),
            journaled,
        }
    }

    fn own_image_of(paths: &Paths) -> PathBuf {
        paths.install().join(MAIN_EXE)
    }

    #[test]
    fn probation_launch_with_exact_bindings_is_admitted() {
        let fx = updating_fixture("probation-ok");
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        let outcome = classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&own_image),
        )
        .unwrap();
        let LaunchOutcome::Admitted(admission) = outcome else {
            panic!("expected admission, got {outcome:?}");
        };
        assert_eq!(admission.context, StartupContext::PostUpdateProbation);
        let facts = admission.probation.as_ref().unwrap();
        assert_eq!(facts.session_id, fx.session_id);
        assert_eq!(facts.installation_id, fx.installation_id);
        assert_eq!(facts.target_version, "1.2.0");
        assert_eq!(facts.nonce, fx.nonce);
        // The child holds the shared lease: maintenance cannot start.
        assert!(desktop_todo_maintenance::lock::exclusive_app(&fx.paths).is_err());
    }

    #[test]
    fn probation_launch_fails_closed_on_every_wrong_binding() {
        // Wrong nonce (a copied or stale environment).
        let fx = updating_fixture("probation-nonce");
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: B64.encode([9u8; 32]),
        };
        let own_image = own_image_of(&fx.paths);
        assert!(classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&own_image)
        )
        .is_err());

        // Wrong session id (an env splice from another session).
        let fx = updating_fixture("probation-session");
        let env = ProbationEnv {
            session_id: uuid::Uuid::new_v4().to_string(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        assert!(classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&own_image)
        )
        .is_err());

        // Wrong running image: not the installed main executable.
        let fx = updating_fixture("probation-image");
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        let elsewhere = fx.paths.install().parent().unwrap().join("elsewhere.exe");
        fs::write(&elsewhere, b"not the installed runtime").unwrap();
        assert!(classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&elsewhere),
        )
        .is_err());

        // PID reuse: same image, different creation time.
        let fx = updating_fixture("probation-pid-reuse");
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        let mut stale = fx.journaled.clone();
        stale.process_created_at = "1".to_string();
        let dir = fx
            .paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&fx.session_id);
        let mut envelope: UpdateSessionEnvelope = serde_json::from_slice(
            &fs::read(dir.join("session.json")).unwrap(),
        )
        .unwrap();
        envelope.probation_process = Some(stale);
        fs::write(
            dir.join("session.json"),
            serde_json::to_vec_pretty(&envelope).unwrap(),
        )
        .unwrap();
        assert!(classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&own_image),
        )
        .is_err());

        // The installed main executable no longer holds the signed target
        // bytes: refuse even though every binding matches.
        let fx = updating_fixture("probation-bytes");
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        fs::write(fx.paths.install().join(MAIN_EXE), b"tampered bytes").unwrap();
        assert!(classify_launch_with_image(
            &fx.paths,
            Some(&own_image),
            Some(&env),
            Some(&own_image),
        )
        .is_err());
    }

    #[test]
    fn probation_launch_without_an_updating_receipt_is_refused() {
        let fx = updating_fixture("probation-installed");
        let mut receipt =
            desktop_todo_maintenance::receipt::Receipt::load(&fx.paths).unwrap().unwrap();
        receipt.lifecycle_state = Lifecycle::Installed;
        receipt.maintenance.active_session_id = None;
        receipt.save(&fx.paths).unwrap();
        let env = ProbationEnv {
            session_id: fx.session_id.clone(),
            nonce: fx.nonce.clone(),
        };
        let own_image = own_image_of(&fx.paths);
        assert!(matches!(
            classify_launch_with_image(
                &fx.paths,
                Some(&own_image),
                Some(&env),
                Some(&own_image),
            ),
            Err(AdmissionBlock::LifecycleBusy(_))
        ));
    }

    #[test]
    fn updating_receipt_routes_the_q14_recovery_helper() {
        let fx = updating_fixture("q14");
        // The helper image must be present and regular.
        fs::write(fx.paths.install().join(HELPER_EXE), b"helper image").unwrap();
        let outcome = classify_launch(&fx.paths, None, None).unwrap();
        let LaunchOutcome::RecoverViaHelper { session_id, helper } = outcome else {
            panic!("expected the Q14 recovery route, got {outcome:?}");
        };
        assert_eq!(session_id, fx.session_id);
        assert_eq!(helper, fx.paths.install().join(HELPER_EXE));
    }

    #[test]
    fn updating_receipt_fails_closed_without_a_valid_journal_or_helper() {
        // Journal missing: recovery-required semantics, never a guessed
        // launch and never a normal launch.
        let fx = updating_fixture("q14-no-journal");
        fs::remove_dir_all(
            fx.paths
                .state()
                .join("updates")
                .join("sessions")
                .join(&fx.session_id),
        )
        .unwrap();
        assert!(matches!(
            classify_launch(&fx.paths, None, None),
            Err(AdmissionBlock::UnsafeInstallation(_))
        ));

        // Valid journal but the helper image is missing: fail closed.
        let fx = updating_fixture("q14-no-helper");
        assert!(matches!(
            classify_launch(&fx.paths, None, None),
            Err(AdmissionBlock::UnsafeInstallation(_))
        ));
    }

    #[test]
    fn the_health_ack_write_uses_the_validated_bindings() {
        let fx = updating_fixture("ack-write");
        let admission = {
            let env = ProbationEnv {
                session_id: fx.session_id.clone(),
                nonce: fx.nonce.clone(),
            };
            let own_image = own_image_of(&fx.paths);
            match classify_launch_with_image(
                &fx.paths,
                Some(&own_image),
                Some(&env),
                Some(&own_image),
            )
            .unwrap()
            {
                LaunchOutcome::Admitted(admission) => admission,
                other => panic!("expected admission, got {other:?}"),
            }
        };
        // The child-side write uses the admission's validated facts (this
        // process IS the journaled instance in this fixture, so the marker
        // carries the journaled identity).
        let facts = admission.probation.as_ref().unwrap();
        let ack = desktop_todo_maintenance::probation::build_current_health_ack(
            &facts.session_id,
            &facts.installation_id,
            &facts.target_version,
            &facts.nonce,
        )
        .unwrap();
        desktop_todo_maintenance::probation::write_health_ack(&fx.paths, &ack).unwrap();
        let marker = fs::read(
            fx.paths
                .state()
                .join("updates")
                .join("sessions")
                .join(&fx.session_id)
                .join("health.json"),
        )
        .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&marker).unwrap();
        assert_eq!(parsed["sessionId"], fx.session_id.as_str());
        assert_eq!(parsed["installationId"], fx.installation_id.as_str());
        assert_eq!(parsed["version"], "1.2.0");
        assert_eq!(parsed["nonce"], fx.nonce.as_str());
        assert_eq!(parsed["schemaVersion"], 1);
    }
}
