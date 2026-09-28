//! The frozen `install_ready_update(sessionId)` IPC command (Phase 2D-B):
//! the only frontend surface into an already-trusted staged update.
//!
//! The frontend supplies exactly one value — a session id — and nothing
//! else. No path, no digest, no provider fact, no "verified" boolean can
//! enter through it: the backend re-resolves the session id strictly
//! against the canonical sessions root, re-authenticates the persisted
//! disk state through the shared verifier (an empty production trust store
//! fails closed until release-signing provisioning), re-confirms the
//! installed source version, and launches the canonical installed helper
//! with the exact frozen argv. After the durable handoff marker is
//! observed, the caller owes the canonical app shutdown — the normal exit
//! path, which releases the shared application lease so the session runner
//! can acquire the exclusive lease; never `std::process::exit`.

use std::time::Duration;

use crate::maintenance_admission::StartupContext;
use crate::updater::acquisition::recover_session;
use crate::updater::spawn::{
    spawn_helper_process, spawn_update_helper, confirm_installed_source, SpawnedUpdate,
    SpawnError,
};

/// Why the install request was refused or could not complete. Surfaced to
/// the frontend as stable strings; the app stays alive on every variant
/// except the successful path (which ends in the canonical shutdown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// Only a managed installation may install its staged update.
    NotManaged,
    /// The session id does not resolve under the canonical sessions root,
    /// or its persisted state failed re-authentication.
    SessionUnknown,
    /// The session is not in the `PackageStaged` milestone.
    NotStaged,
    /// The installed source version no longer matches the session baseline.
    SourceChanged,
    /// The canonical helper image is unusable or the spawn failed.
    Launch(String),
    /// The helper never durably confirmed the handoff: the app must stay
    /// alive and the transaction stays resumable.
    HandoffNotDurable,
    /// Canonical roots could not be resolved.
    CanonicalRoots,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotManaged => write!(f, "not-managed"),
            Self::SessionUnknown => write!(f, "session-unknown"),
            Self::NotStaged => write!(f, "session-not-staged"),
            Self::SourceChanged => write!(f, "source-version-changed"),
            Self::Launch(detail) => write!(f, "launch-failed: {detail}"),
            Self::HandoffNotDurable => write!(f, "handoff-not-durable"),
            Self::CanonicalRoots => write!(f, "installation-unresolved"),
        }
    }
}

impl From<SpawnError> for InstallError {
    fn from(error: SpawnError) -> Self {
        match error {
            SpawnError::NotStaged => Self::NotStaged,
            SpawnError::Prepare(
                crate::updater::session::PrepareError::NotStaged,
            ) => Self::NotStaged,
            SpawnError::SourceVersion { .. } => Self::SourceChanged,
            SpawnError::HelperImage { detail } => Self::Launch(detail),
            SpawnError::Launch { detail } => Self::Launch(detail),
            SpawnError::HandoffNotDurable => Self::HandoffNotDurable,
            other => Self::Launch(other.to_string()),
        }
    }
}

/// The command core: resolve, re-authenticate, spawn, and wait for the
/// durable handoff. Blocking; the Tauri command runs it off the main
/// thread. On success the caller performs the canonical shutdown.
pub fn run_install_ready_update(session_id: &str) -> Result<SpawnedUpdate, InstallError> {
    let paths = desktop_todo_maintenance::paths::Paths::resolve()
        .map_err(|_| InstallError::CanonicalRoots)?;
    run_install_ready_update_at(&paths, session_id)
}

/// [`run_install_ready_update`] against an explicit root set — the seam
/// deterministic tests use; production resolves canonical roots.
pub fn run_install_ready_update_at(
    paths: &desktop_todo_maintenance::paths::Paths,
    session_id: &str,
) -> Result<SpawnedUpdate, InstallError> {
    run_install_ready_update_with(paths, &confirm_installed_source, session_id)
}

/// [`run_install_ready_update_at`] with the source reconfirmation
/// injectable (deterministic tests; sandbox fixtures carry no PE version
/// resources). The production check fails closed on any disagreement.
pub fn run_install_ready_update_with(
    paths: &desktop_todo_maintenance::paths::Paths,
    confirm_source: &dyn Fn(&desktop_todo_maintenance::paths::Paths) -> Result<desktop_todo_update_core::Version, String>,
    session_id: &str,
) -> Result<SpawnedUpdate, InstallError> {
    let trust = desktop_todo_update_core::production_trust_store();
    // The session id is the only frontend input: resolved strictly against
    // the canonical sessions root and re-authenticated from persisted
    // bytes. An unknown or tampered session is a refusal, never a repair.
    let updates_root = paths.state().join("updates");
    let installed = confirm_source(paths).map_err(|_| InstallError::SourceChanged)?;
    let session = recover_session(&updates_root, session_id, &trust, &installed)
        .map_err(|_| InstallError::SessionUnknown)?;
    let spawned = spawn_update_helper(
        &session,
        paths,
        &confirm_installed_source,
        &spawn_helper_process,
    )?;
    // The caller may exit only after the handoff is durable.
    spawned.wait_for_handoff_durable(120, Duration::from_millis(250))?;
    Ok(spawned)
}

/// The frozen frontend command: `install_ready_update(sessionId)`. Only a
/// managed installation may call it. On success the app performs its
/// canonical shutdown (the normal exit path releases the shared lease).
#[tauri::command]
pub async fn install_ready_update(
    app: tauri::AppHandle,
    admission: tauri::State<'_, crate::maintenance_admission::Admission>,
    session_id: String,
) -> Result<(), String> {
    use tauri::Manager;
    if admission.context != StartupContext::Managed {
        return Err(InstallError::NotManaged.to_string());
    }
    let session_id = session_id.trim().to_string();
    let result = tauri::async_runtime::spawn_blocking(move || {
        run_install_ready_update(&session_id)
    })
    .await
    .map_err(|e| format!("launch-failed: {e}"))?;
    match result {
        Ok(spawned) => {
            app.state::<crate::qa_diagnostics::QaDiagnostics>()
                .record(format!(
                    "[updater] handoff durable for session {}; performing canonical shutdown",
                    spawned.session_id
                ));
            // Canonical shutdown: the normal exit path — never a hard exit.
            crate::product_window::quit_app(app);
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session id that does not resolve under the canonical sessions root
    /// is a stable refusal: the frontend can never create or point at a
    /// session the local registry does not know. (The source
    /// reconfirmation is injected because sandbox fixtures carry no PE
    /// version resources; the production check is exercised upstream.)
    #[test]
    fn unknown_session_refuses_and_surfaces_stable_strings() {
        let id = uuid::Uuid::new_v4().to_string();
        let root = std::env::temp_dir().join(format!("dtw-2db-install-{}", uuid::Uuid::new_v4()));
        let paths = desktop_todo_maintenance::paths::Paths::sandbox(root, &id);
        desktop_todo_maintenance::paths::create_dir(paths.state()).unwrap();
        let source_ok = |_: &desktop_todo_maintenance::paths::Paths| {
            Ok(desktop_todo_update_core::Version::parse("1.1.0").unwrap())
        };
        let error = run_install_ready_update_with(&paths, &source_ok, &uuid::Uuid::new_v4().to_string())
            .unwrap_err();
        assert_eq!(error, InstallError::SessionUnknown, "{error}");
        assert_eq!(error.to_string(), "session-unknown");
        // The closed refusal taxonomy stays stable — these strings are the
        // whole surface the frontend maps to localized copy.
        assert_eq!(InstallError::NotManaged.to_string(), "not-managed");
        assert_eq!(InstallError::NotStaged.to_string(), "session-not-staged");
        assert_eq!(
            InstallError::HandoffNotDurable.to_string(),
            "handoff-not-durable"
        );
        assert_eq!(
            InstallError::SourceChanged.to_string(),
            "source-version-changed"
        );
    }
}
