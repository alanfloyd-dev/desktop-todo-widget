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

/// Which kind of launch admission produced. A `PostUpdateProbation` context
/// will be added together with the HealthAck protocol; until an update
/// transaction exists there is nothing to model it on, and no state silently
/// stands in for it.
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
}

impl StartupContext {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::Unmanaged => "unmanaged",
            Self::Development => "development",
        }
    }
}

/// Long-lived admission record; holds the shared application lease so a
/// maintenance transaction cannot start while any instance is running.
pub struct Admission {
    pub context: StartupContext,
    pub installation_id: Option<String>,
    /// The install root this admission was resolved against (canonical for
    /// production; the QA sandbox under the maintenance-qa feature).
    pub(crate) install_root: PathBuf,
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

/// Classifies one launch. `process_image_dir` is the directory of the running
/// executable (`None` when it cannot be determined). Tests call this directly
/// with sandbox paths; the binary entry uses [`admit_or_report`].
pub fn admit(paths: &Paths, process_image_dir: Option<&Path>) -> Result<Admission, AdmissionBlock> {
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
            Ok(Admission {
                context,
                installation_id: Some(receipt.installation_id.clone()),
                install_root: paths.install().to_path_buf(),
                handoff_started: AtomicBool::new(false),
                _lease: Some(lease),
            })
        }
        None => {
            // Helper present without a receipt is broken managed state, not a
            // legacy install (v1.1 payloads never contained the helper).
            if paths.install().join(HELPER_EXE).exists() {
                return Err(AdmissionBlock::ManagedReceiptMissing);
            }
            Ok(Admission {
                context: StartupContext::Unmanaged,
                installation_id: None,
                install_root: paths.install().to_path_buf(),
                handoff_started: AtomicBool::new(false),
                _lease: Some(lease),
            })
        }
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
/// everywhere, then a non-zero exit). If the canonical roots themselves cannot
/// be resolved the launch degrades to pre-maintenance behaviour without a
/// lease, logged: on such a machine the helper could not run either, and the
/// Tauri data-dir resolution in `setup` still guards the database path.
pub fn admit_or_report() -> Admission {
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
                handoff_started: AtomicBool::new(false),
                _lease: None,
            };
        }
    };
    match admit(&paths, process_image_dir().as_deref()) {
        Ok(admission) => admission,
        Err(block) => {
            eprintln!("[maintenance-admission] launch refused: {block:?}");
            show_block_dialog(&block.user_message());
            std::process::exit(3);
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
}
