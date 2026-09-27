//! The production helper spawn boundary (Phase 2D-A).
//!
//! Wires the Phase 2C-B [`HandoffCommand`] to the real spawn: prepare the
//! frozen session envelope (which re-verifies the staged executables and
//! publishes the envelope by write generation), re-confirm the installed
//! source version, validate the canonical helper image, then launch the
//! helper with the exact frozen argv — `Command::new(canonical path)`, no
//! shell, no PATH lookup, no extra arguments, no frontend/provider influence.
//!
//! The caller (the future `install_ready_update` IPC command, not yet wired)
//! exits through its normal exit path after a successful spawn so the shared
//! application lease drops and the helper can proceed; the helper binds the
//! live caller identity at startup, so a spawn whose caller exits too early
//! fails closed and stays resumable by a later launch. No spawn happens
//! unless every preflight check passed, and a spawn failure is typed.

use std::path::{Path, PathBuf};

use desktop_todo_maintenance::paths;
use desktop_todo_update_core::Version;

use crate::updater::acquisition::{MilestoneState, UpdateSession};
use crate::updater::session::{prepare_handoff, HandoffCommand, PrepareError};

/// Why the production spawn was refused. Distinct from preparation and
/// candidate errors: this layer only launches an already-verified handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnError {
    /// The session milestone is not `PackageStaged`.
    NotStaged,
    /// Handoff preparation failed (staged verification, envelope publish,
    /// installation state).
    Prepare(PrepareError),
    /// The canonical helper image is missing or not a regular, unlinked,
    /// non-reparse file.
    HelperImage { detail: String },
    /// The installed source version no longer matches the session baseline.
    SourceVersion { detail: String },
    /// The process spawn itself failed.
    Launch { detail: String },
    /// The helper never durably confirmed the handoff (see
    /// [`SpawnedUpdate::wait_for_handoff_durable`]); the caller must NOT
    /// exit on this result — the transaction stays resumable and the app
    /// stays alive.
    HandoffNotDurable,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStaged => write!(f, "update session is not in the PackageStaged milestone"),
            Self::Prepare(error) => write!(f, "{error}"),
            Self::HelperImage { detail } => write!(f, "helper image unusable: {detail}"),
            Self::SourceVersion { detail } => write!(f, "source version reconfirmation failed: {detail}"),
            Self::Launch { detail } => write!(f, "helper spawn failed: {detail}"),
            Self::HandoffNotDurable => write!(
                f,
                "the helper never durably confirmed the handoff; the caller must stay alive"
            ),
        }
    }
}

/// One completed production spawn. **The caller now owes the canonical
/// shutdown**: (1) wait for the durable handoff marker with
/// [`SpawnedUpdate::wait_for_handoff_durable`] — the helper binds the live
/// caller identity during its Initial validation, so the caller must stay
/// alive at least until that binding is durable; (2) then exit through the
/// product's normal exit path so the shared application lease drops and the
/// session runner can acquire the exclusive lease. Nothing in the current
/// tree performs step (2) — the production IPC wiring is future work, and
/// the choreography must not be described as automatic until it exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnedUpdate {
    pub session_id: String,
    pub expected_manifest_sha256: String,
    pub helper_path: PathBuf,
    /// The durable session directory (for the handoff wait).
    pub session_dir: PathBuf,
}

impl SpawnedUpdate {
    /// Bounded wait for the durable handoff marker: the frozen session
    /// envelope reaching the `handed-off` phase, written by the helper after
    /// its Initial validation, live-caller binding, and the `Updating`
    /// receipt transition. This is the earliest durable point after which
    /// the caller may exit. Pure coordination read — the helper's
    /// validation remains the only authority.
    pub fn wait_for_handoff_durable(
        &self,
        attempts: u32,
        interval: std::time::Duration,
    ) -> Result<(), SpawnError> {
        for _ in 0..attempts {
            if let Ok(envelope) =
                desktop_todo_maintenance::update_session::load_update_session(&self.session_dir)
            {
                if envelope.phase == desktop_todo_maintenance::update_session::SessionPhase::HandedOff {
                    return Ok(());
                }
            }
            std::thread::sleep(interval);
        }
        Err(SpawnError::HandoffNotDurable)
    }
}

/// Spawn the canonical helper for a staged, recovered session.
///
/// `confirm_source` re-derives the actual installed source version and must
/// fail closed on any disagreement (the production implementation is
/// `desktop_todo_maintenance::handoff::derive_installed_source`; tests inject
/// a deterministic check because sandbox fixtures carry no PE version
/// resources). `spawn` is injectable for the same reason; the production
/// implementation is [`spawn_helper_process`].
/// The injected source-reconfirmation check (deterministic in tests).
type SourceCheck<'a> = dyn Fn(&desktop_todo_maintenance::paths::Paths) -> Result<Version, String> + 'a;
/// The injected spawn (records the launch in tests).
type SpawnFn<'a> = dyn Fn(&Path, &[String]) -> Result<(), String> + 'a;

pub fn spawn_update_helper(
    session: &UpdateSession,
    paths: &desktop_todo_maintenance::paths::Paths,
    confirm_source: &SourceCheck<'_>,
    spawn: &SpawnFn<'_>,
) -> Result<SpawnedUpdate, SpawnError> {
    if session.record.state != MilestoneState::PackageStaged {
        return Err(SpawnError::NotStaged);
    }
    // Preparation re-verifies the staged executables and publishes the
    // frozen envelope (idempotently, by write generation).
    let command: HandoffCommand = prepare_handoff(session, paths.install()).map_err(SpawnError::Prepare)?;
    // Re-confirm the installed source version right before the launch: a
    // rollback or reinstall between staging and now must refuse here.
    confirm_source(paths).map_err(|detail| SpawnError::SourceVersion { detail })?;
    // The helper identity is compiled policy (canonical install root + fixed
    // name): a regular, unlinked, non-reparse file. The helper revalidates
    // everything on its side; this check keeps the obvious refusal local.
    paths::open_regular(&command.helper_path).map_err(|e| SpawnError::HelperImage {
        detail: e.to_string(),
    })?;
    spawn(&command.helper_path, &command.argv).map_err(|detail| SpawnError::Launch { detail })?;
    Ok(SpawnedUpdate {
        session_dir: paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&command.session_id),
        session_id: command.session_id,
        expected_manifest_sha256: command.expected_manifest_sha256,
        helper_path: command.helper_path,
    })
}

/// The production spawn: exact path, exact argv, no window. Never a shell,
/// never PATH lookup.
pub fn spawn_helper_process(helper: &Path, argv: &[String]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(helper)
        .args(argv)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map(|_child| ())
        .map_err(|error| error.to_string())
}

/// The production source reconfirmation: the full installed-version
/// authority (receipt, PE image, committed evidence chain) from the shared
/// maintenance library.
pub fn confirm_installed_source(
    paths: &desktop_todo_maintenance::paths::Paths,
) -> Result<Version, String> {
    let trust = desktop_todo_update_core::production_trust_store();
    desktop_todo_maintenance::handoff::derive_installed_source(paths, &trust).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::acquisition::TrustedTargetRecord;
    use std::cell::RefCell;

    /// A staged fixture (same construction as the session.rs tests), used
    /// here through the public recovery + preparation boundary.
    fn staged_fixture(tag: &str) -> (UpdateSession, desktop_todo_maintenance::paths::Paths, String) {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        use desktop_todo_update_core::{derive_key_id, TrustStore};
        use ed25519_dalek::{Signer, SigningKey};

        const TEST_SEED: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";
        let signing = SigningKey::from_bytes(&TEST_SEED);
        let trust = TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap();

        let exe1 = (0..64u32).map(|i| (i as u8).wrapping_add(1)).collect::<Vec<_>>();
        let exe2 = (0..64u32).map(|i| (i as u8).wrapping_add(2)).collect::<Vec<_>>();
        let support = b"support".to_vec();
        let sha_of = |bytes: &[u8]| desktop_todo_update_core::sha256_hex(bytes);
        let mut package: Vec<u8> = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut package);
            let mut writer = zip::ZipWriter::new(cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in [
                ("desktop-todo-widget.exe", exe1.clone()),
                ("desktop-todo-maintenance.exe", exe2.clone()),
                ("install.ps1", support.clone()),
                ("uninstall.ps1", support.clone()),
                ("README.md", support.clone()),
                ("README_ZH.md", support.clone()),
                ("LICENSE", support.clone()),
                ("LICENSE_ZH.md", support.clone()),
                ("THIRD_PARTY_NOTICES.md", support),
            ] {
                writer.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut writer, &bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        let manifest = format!(
            concat!(
                r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
                r#""publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"#,
                r#""assets":{{"windows-x64":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":{pkg},"sha256":"{psha}","installFiles":["#,
                r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{s1},"sha256":"{h1}"}},"#,
                r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{s2},"sha256":"{h2}"}}]}}}}}}"#
            ),
            v = "1.4.0",
            pkg = package.len(),
            psha = sha_of(&package),
            s1 = exe1.len(),
            h1 = sha_of(&exe1),
            s2 = exe2.len(),
            h2 = sha_of(&exe2),
        );
        let manifest_bytes = manifest.into_bytes();
        let signature = signing.sign(&manifest_bytes).to_bytes();
        let envelope_bytes = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
            derive_key_id(&signing.verifying_key().to_bytes()),
            STANDARD.encode(signature)
        )
        .into_bytes();
        let target = desktop_todo_update_core::verify_and_parse(&trust, &envelope_bytes, &manifest_bytes)
            .expect("fixture target verifies");

        let base = std::env::temp_dir().join(format!("dtw-2da-spawn-{tag}-{}", uuid::Uuid::new_v4()));
        let updates_root = base.join("updates");
        let install_root = base.join("install");
        std::fs::create_dir_all(&install_root).unwrap();
        std::fs::write(
            install_root.join("installation-receipt.json"),
            format!(r#"{{"installationId":"{}"}}"#, uuid::Uuid::new_v4()),
        )
        .unwrap();

        let session_id = uuid::Uuid::new_v4().to_string();
        let dir = updates_root.join("sessions").join(&session_id);
        std::fs::create_dir_all(dir.join("staged")).unwrap();
        std::fs::write(dir.join("update-manifest.json"), &manifest_bytes).unwrap();
        std::fs::write(dir.join("update-manifest.json.sig"), &envelope_bytes).unwrap();
        std::fs::write(dir.join("package.zip"), &package).unwrap();
        std::fs::write(dir.join("staged/desktop-todo-widget.exe"), &exe1).unwrap();
        std::fs::write(dir.join("staged/desktop-todo-maintenance.exe"), &exe2).unwrap();

        let asset = target.manifest().asset();
        let record = TrustedTargetRecord {
            schema_version: 1,
            state: MilestoneState::PackageStaged,
            target_version: target.manifest().version().to_string(),
            manifest_sha256: target.manifest_sha256_hex().to_string(),
            envelope_sha256: desktop_todo_update_core::sha256_hex(&envelope_bytes),
            installed_source_version: "1.1.0".to_string(),
            package: crate::updater::acquisition::PackageFact {
                filename: asset.filename().to_string(),
                size: asset.size(),
                sha256: asset.sha256_hex().to_string(),
            },
            install_files: asset
                .install_files()
                .iter()
                .map(|entry| crate::updater::acquisition::InstallFileFact {
                    identity: entry.identity().to_string(),
                    filename: entry.filename().to_string(),
                    size: entry.size(),
                    sha256: entry.sha256_hex().to_string(),
                })
                .collect(),
            discovered_via: "GitHub".to_string(),
            package_url: "https://mirror.invalid/pkg.zip".to_string(),
            created_at: "2026-10-01T00:00:00Z".to_string(),
        };
        std::fs::write(
            dir.join("trusted-target.json"),
            serde_json::to_vec_pretty(&record).unwrap(),
        )
        .unwrap();

        // The maintenance Paths sandbox mirrors the same layout: install root
        // and maintenance state root, so the shared canonical-root checks
        // resolve against the fixture.
        let paths = desktop_todo_maintenance::paths::Paths::sandbox(base.join("maintenance-root"), &session_id);
        // Point the sandbox state root at the fixture updates root by
        // building the session through the real recovery path.
        let session = crate::updater::acquisition::recover_session(
            &updates_root,
            &session_id,
            &trust,
            &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
        )
        .expect("session recovers");
        (session, paths, target.manifest_sha256_hex().to_string())
    }

    #[test]
    fn spawn_uses_the_canonical_helper_with_the_frozen_argv() {
        let (session, paths, digest) = staged_fixture("spawn");
        place_receipt(&session, &paths);
        let helper_path = paths.install().join(desktop_todo_maintenance::HELPER_EXE);
        std::fs::write(&helper_path, b"helper image").unwrap();

        let spawned = RefCell::new(None::<(PathBuf, Vec<String>)>);
        let result = spawn_update_helper(
            &session,
            &paths,
            &|_| Ok(Version::parse("1.1.0").unwrap()),
            &|path, argv| {
                *spawned.borrow_mut() = Some((path.to_path_buf(), argv.to_vec()));
                Ok(())
            },
        )
        .expect("spawn succeeds");
        assert_eq!(result.session_id, session.id);
        assert_eq!(result.helper_path, helper_path);
        let (path, argv) = spawned.into_inner().unwrap();
        // Exact canonical path, exact frozen argv order, nothing else.
        assert_eq!(path, helper_path);
        assert_eq!(
            argv,
            vec![
                "--update".to_string(),
                "--session-id".to_string(),
                session.id.clone(),
                "--expected-manifest-sha256".to_string(),
                result.expected_manifest_sha256.clone(),
            ]
        );
        assert_eq!(result.expected_manifest_sha256, digest);
    }

    #[test]
    fn spawn_refusals_never_launch() {
        let no_launch = |_: &Path, _: &[String]| -> Result<(), String> {
            panic!("must not spawn")
        };
        let source_ok = |_: &desktop_todo_maintenance::paths::Paths| Ok(Version::parse("1.1.0").unwrap());

        // Missing helper image: refused before any spawn (own fixture — the
        // durable envelope's old-present facts stay stable per fixture).
        let (session, paths, _digest) = staged_fixture("refuse-image");
        place_receipt(&session, &paths);
        let error = spawn_update_helper(&session, &paths, &source_ok, &no_launch).unwrap_err();
        assert!(matches!(error, SpawnError::HelperImage { .. }), "{error}");

        // Source reconfirmation failure: refused before any spawn.
        let (session, paths, _digest) = staged_fixture("refuse-source");
        place_receipt(&session, &paths);
        std::fs::write(
            paths.install().join(desktop_todo_maintenance::HELPER_EXE),
            b"helper image",
        )
        .unwrap();
        let error = spawn_update_helper(
            &session,
            &paths,
            &|_| Err("source changed".to_string()),
            &no_launch,
        )
        .unwrap_err();
        assert!(matches!(error, SpawnError::SourceVersion { .. }), "{error}");

        // Launch failure is typed and propagates.
        let error = spawn_update_helper(
            &session,
            &paths,
            &source_ok,
            &|_, _| Err("access denied".to_string()),
        )
        .unwrap_err();
        assert!(matches!(error, SpawnError::Launch { .. }), "{error}");
    }

    #[test]
    fn caller_obligation_waits_for_the_durable_handoff() {
        let (session, paths, _digest) = staged_fixture("handoff-wait");
        place_receipt(&session, &paths);
        std::fs::write(
            paths.install().join(desktop_todo_maintenance::HELPER_EXE),
            b"helper image",
        )
        .unwrap();
        let mut spawned = spawn_update_helper(
            &session,
            &paths,
            &|_| Ok(Version::parse("1.1.0").unwrap()),
            &|_, _| Ok(()),
        )
        .expect("spawn succeeds");
        // The fixture's session directory lives under the fixture updates
        // root (the sandbox Paths state root is a parallel tree); point the
        // wait at the real one — the wait logic is what is under test, and
        // the production derivation (paths.state()/updates/sessions/<id>)
        // is the frozen layout itself.
        spawned.session_dir = session.dir.clone();

        // Before the helper's Initial validation lands, the handoff is not
        // durable: the wait must time out and the caller must keep running.
        let error = spawned
            .wait_for_handoff_durable(2, std::time::Duration::from_millis(1))
            .unwrap_err();
        assert!(matches!(error, SpawnError::HandoffNotDurable), "{error}");

        // Once the helper journals the handoff intent (phase `handed-off`;
        // here simulated by rewriting the durable envelope, which the real
        // helper does after its live-caller binding and the Updating
        // transition), the wait succeeds and the caller may exit.
        let bytes = std::fs::read(spawned.session_dir.join("session.json")).unwrap();
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("\"phase\": \"staged\"", "\"phase\": \"handed-off\"");
        std::fs::write(spawned.session_dir.join("session.json"), text).unwrap();
        spawned
            .wait_for_handoff_durable(2, std::time::Duration::from_millis(1))
            .expect("durable handoff observed");
    }

    /// The preparation step reads the installation receipt from the canonical
    /// install root of the supplied Paths; place the fixture receipt there.
    fn place_receipt(session: &UpdateSession, paths: &desktop_todo_maintenance::paths::Paths) {
        let fixture_root = session.dir.ancestors().nth(3).unwrap().join("install");
        let receipt = std::fs::read(fixture_root.join("installation-receipt.json")).unwrap();
        std::fs::create_dir_all(paths.install()).unwrap();
        std::fs::write(paths.receipt(), receipt).unwrap();
    }
}
