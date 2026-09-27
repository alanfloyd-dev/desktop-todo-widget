//! UpdateSession preparation and the frozen helper handoff command
//! (Phase 2C-B).
//!
//! Creation boundary (frozen): a frozen `UpdateSession` envelope can only
//! be published from a recovered session in the `PackageStaged` milestone —
//! that is, a session whose `VerifiedTarget` was re-derived from the
//! persisted raw signed bytes through the shared verifier, whose record
//! fields were re-checked against that target, whose compatibility baseline
//! was rebound against the current actual installed source version, and
//! whose staged executables re-verify against the signed `installFiles`
//! facts. The recovered `UpdateSession` type itself is only obtainable
//! through `persist_trusted_target`/`recover_session`, so there is no API
//! that could create a handoff from a parsed manifest, provider metadata,
//! frontend parameters, arbitrary paths, or a "verified" boolean.
//!
//! The handoff command is the frozen form: the canonical helper binary
//! invoked with `--update --session-id <UUID> --expected-manifest-sha256
//! <64 hex>` — nothing else, no package URL, no provider, no digest from
//! the frontend, no install root. The digest is derived exclusively from
//! the verified raw manifest bytes (`VerifiedTarget::manifest_sha256_hex`).
//! This phase constructs the command and stops: no spawn happens (the main
//! application has no production updater wiring yet), and the helper, when
//! launched, must reach its own validated state before anything mutates.

use std::path::{Path, PathBuf};

use desktop_todo_maintenance::package_zip::ArchiveError;
use desktop_todo_maintenance::update_session::{
    publish_update_session, HandoffFacts, ProcessIdentity, SessionError, UpdateSessionEnvelope,
};

use crate::updater::acquisition::{MilestoneState, UpdateSession};

/// The frozen helper handoff, fully constructed and ready to be launched by
/// the future wiring. No spawn happens in this phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffCommand {
    pub session_id: String,
    pub expected_manifest_sha256: String,
    /// Exact argv for the canonical helper, in the frozen order:
    /// `--update`, `--session-id`, `<UUID>`, `--expected-manifest-sha256`,
    /// `<64 hex>`.
    pub argv: Vec<String>,
    /// The canonical installed helper path (compiled identity: canonical
    /// install root + fixed helper filename). Never searched on PATH; never
    /// supplied by the frontend or a provider.
    pub helper_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// The session milestone is not `PackageStaged`.
    NotStaged,
    /// The staged executables no longer match the signed facts.
    StagedVerification(ArchiveError),
    /// The frozen envelope could not be published (IO, malformed durable
    /// state, or a conflict with an existing envelope).
    Publish(SessionError),
    /// The local installation state cannot support a handoff.
    InstallationInvalid { detail: String },
    /// The canonical installation identity could not be resolved.
    CanonicalRoots(String),
    Io { detail: String },
}

impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStaged => {
                write!(f, "update session is not in the PackageStaged milestone")
            }
            Self::StagedVerification(error) => {
                write!(f, "staged executables failed re-verification: {error:?}")
            }
            Self::Publish(error) => write!(f, "{error}"),
            Self::InstallationInvalid { detail } => {
                write!(f, "installation cannot support a handoff: {detail}")
            }
            Self::CanonicalRoots(detail) => {
                write!(f, "canonical installation identity unresolved: {detail}")
            }
            Self::Io { detail } => write!(f, "handoff preparation IO failure: {detail}"),
        }
    }
}

/// The current process identity for the frozen `parentProcess` field: PID,
/// process creation time (Windows FILETIME as a decimal string) and the
/// exact image path. PID alone is insufficient.
fn current_process_identity() -> Result<ProcessIdentity, PrepareError> {
    let image = std::env::current_exe().map_err(|e| PrepareError::Io { detail: e.to_string() })?;
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        unsafe {
            let mut created: FILETIME = FILETIME::default();
            let mut exited: FILETIME = FILETIME::default();
            let mut kernel: FILETIME = FILETIME::default();
            let mut user: FILETIME = FILETIME::default();
            GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
            .map_err(|e| PrepareError::Io { detail: e.to_string() })?;
            let filetime = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
            Ok(ProcessIdentity {
                pid: std::process::id(),
                process_created_at: filetime.to_string(),
                image_path: image,
            })
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(ProcessIdentity {
            pid: std::process::id(),
            process_created_at: "0".to_string(),
            image_path: image,
        })
    }
}

/// The installation UUID, read from the validated receipt at the install
/// root — the session's own installationId is written from this receipt for
/// the helper to cross-check against the installation it validates.
fn installation_id_from_receipt(install_root: &Path) -> Result<String, PrepareError> {
    let receipt_path = install_root.join("installation-receipt.json");
    let bytes = std::fs::read(&receipt_path).map_err(|e| PrepareError::Io {
        detail: format!("installation receipt unreadable: {e}"),
    })?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct InstallationId {
        installation_id: String,
    }
    let parsed: InstallationId = serde_json::from_slice(&bytes)
        .map_err(|e| PrepareError::Io { detail: format!("installation receipt malformed: {e}") })?;
    Ok(parsed.installation_id)
}

/// Prepare the frozen helper handoff for a staged, recovered session:
/// re-verify the staged executables against the signed facts, publish the
/// frozen `UpdateSession` envelope (idempotently, by write generation), and
/// construct the frozen CLI handoff. No spawn happens; nothing mutates the
/// installed runtime.
///
/// `install_root` is the canonical installation root snapshot (production
/// resolves it from compiled policy via
/// `desktop_todo_maintenance::paths::canonical_roots`; tests inject sandbox
/// roots). The helper re-derives it independently, so a wrong snapshot can
/// only cause a refusal.
pub fn prepare_handoff(
    session: &UpdateSession,
    install_root: &Path,
) -> Result<HandoffCommand, PrepareError> {
    if session.record.state != MilestoneState::PackageStaged {
        return Err(PrepareError::NotStaged);
    }

    // The staged milestone must still be true right now: fresh opens and
    // hash checks against the signed installFiles facts.
    let staged_dir = session.dir.join(crate::updater::acquisition::STAGED_DIR);
    let expectations = session
        .target
        .manifest()
        .asset()
        .install_files()
        .iter()
        .map(|entry| {
            (
                entry.identity().to_string(),
                entry.filename().to_string(),
                entry.size(),
                entry.sha256_hex().to_string(),
            )
        })
        .collect::<Vec<_>>();
    desktop_todo_maintenance::package_zip::verify_staged_executables(&staged_dir, &expectations)
        .map_err(PrepareError::StagedVerification)?;

    let installation_id = installation_id_from_receipt(install_root)?;
    // The updates root is the session directory's grandparent:
    // <updates>/sessions/<id>.
    let updates_root = session
        .dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            PrepareError::CanonicalRoots("session directory is not under an updates root".into())
        })?;

    let envelope: UpdateSessionEnvelope = publish_update_session(
        &session.target,
        HandoffFacts {
            session_id: &session.id,
            installation_id: &installation_id,
            from_version: &session.record.installed_source_version,
            discovered_via: &session.record.discovered_via,
            package_url: &session.record.package_url,
            install_root,
            updates_root,
            parent_process: current_process_identity()?,
        },
    )
    .map_err(PrepareError::Publish)?;

    // The published envelope must bind exactly the digest the CLI command
    // carries — both are derived from the same verified raw bytes.
    let expected_manifest_sha256 = session.record.manifest_sha256.clone();
    if envelope.manifest_sha256 != expected_manifest_sha256
        || envelope.session_id != session.id
        || envelope.from_version != session.record.installed_source_version
    {
        return Err(PrepareError::Publish(SessionError::Conflict {
            field: "manifestSha256".to_string(),
        }));
    }

    Ok(HandoffCommand {
        session_id: session.id.clone(),
        expected_manifest_sha256: expected_manifest_sha256.clone(),
        argv: vec![
            "--update".to_string(),
            "--session-id".to_string(),
            session.id.clone(),
            "--expected-manifest-sha256".to_string(),
            expected_manifest_sha256,
        ],
        helper_path: install_root.join(desktop_todo_maintenance::HELPER_EXE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use desktop_todo_maintenance::update_session::{load_update_session, SessionPhase};
    use desktop_todo_update_core::{derive_key_id, TrustStore};
    use ed25519_dalek::{Signer, SigningKey};

    /// TEST-ONLY seed. Never a production trust root, never release signing.
    const TEST_SEED: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";

    fn test_trust() -> TrustStore {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap()
    }

    fn sha_of(bytes: &[u8]) -> String {
        desktop_todo_update_core::sha256_hex(bytes)
    }

    fn exe_payload(seed: u8) -> Vec<u8> {
        (0..64u32).map(|i| (i as u8).wrapping_add(seed)).collect()
    }

    struct Fixture {
        updates_root: PathBuf,
        install_root: PathBuf,
        session: crate::updater::acquisition::UpdateSession,
        digest: String,
    }

    /// A staged, recovered session built directly from a signed fixture
    /// (no network): persisted raw manifest/envelope, package ZIP, staged
    /// EXEs, and a PackageStaged milestone record.
    fn staged_fixture(tag: &str) -> Fixture {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        let trust = test_trust();

        let exe1 = exe_payload(1);
        let exe2 = exe_payload(2);
        let support = b"support".to_vec();
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

        let base = std::env::temp_dir().join(format!("dtw-2cb-{tag}-{}", uuid::Uuid::new_v4()));
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

        // Milestone record in the PackageStaged state, matching the verified
        // target field-for-field (recovery re-checks every field).
        let asset = target.manifest().asset();
        let record = crate::updater::acquisition::TrustedTargetRecord {
            schema_version: 1,
            state: crate::updater::acquisition::MilestoneState::PackageStaged,
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

        let digest = target.manifest_sha256_hex().to_string();
        let session = crate::updater::acquisition::recover_session(
            &updates_root,
            &session_id,
            &trust,
            &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
        )
        .expect("session recovers");
        Fixture {
            updates_root,
            install_root,
            session,
            digest,
        }
    }

    #[test]
    fn prepare_produces_frozen_command_and_envelope() {
        let fixture = staged_fixture("prepare");
        let command =
            prepare_handoff(&fixture.session, &fixture.install_root).expect("handoff prepares");

        // Frozen argv form, exact order, nothing else.
        assert_eq!(
            command.argv,
            vec![
                "--update".to_string(),
                "--session-id".to_string(),
                fixture.session.id.clone(),
                "--expected-manifest-sha256".to_string(),
                fixture.digest.clone(),
            ]
        );
        assert_eq!(command.session_id, fixture.session.id);
        // The digest comes only from the verified raw manifest bytes.
        assert_eq!(
            command.expected_manifest_sha256,
            fixture.session.target.manifest_sha256_hex()
        );
        // The helper identity is compiled policy at the canonical root.
        assert_eq!(
            command.helper_path,
            fixture.install_root.join(desktop_todo_maintenance::HELPER_EXE)
        );

        // The frozen envelope is durable and binds the same digest.
        let dir = fixture
            .updates_root
            .join("sessions")
            .join(&fixture.session.id);
        let envelope = load_update_session(&dir).unwrap();
        assert_eq!(envelope.phase, SessionPhase::Staged);
        assert_eq!(envelope.manifest_sha256, fixture.digest);
        assert_eq!(envelope.to_version, "1.4.0");
        assert_eq!(envelope.from_version, "1.1.0");
        assert_eq!(envelope.generation, 1);
        assert_eq!(envelope.resources.len(), 2);
        assert_eq!(envelope.parent_process.pid, std::process::id());
    }

    #[test]
    fn prepare_requires_the_staged_milestone() {
        let mut fixture = staged_fixture("not-staged");
        // Rewrite the milestone back to trusted-target-persisted and recover.
        let dir = fixture
            .updates_root
            .join("sessions")
            .join(&fixture.session.id);
        let path = dir.join("trusted-target.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["state"] = serde_json::Value::String("trustedTargetPersisted".into());
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        fixture.session = crate::updater::acquisition::recover_session(
            &fixture.updates_root,
            &fixture.session.id,
            &test_trust(),
            &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
        )
        .unwrap();
        match prepare_handoff(&fixture.session, &fixture.install_root) {
            Err(PrepareError::NotStaged) => {}
            other => panic!("expected NotStaged, got {other:?}"),
        }
    }

    #[test]
    fn prepare_is_idempotent_across_restarts_and_bumps_generation() {
        let fixture = staged_fixture("resume");
        let first = prepare_handoff(&fixture.session, &fixture.install_root).unwrap();
        let dir = fixture
            .updates_root
            .join("sessions")
            .join(&fixture.session.id);
        let first_envelope = load_update_session(&dir).unwrap();
        // Simulated restart: recover again from disk, then re-prepare.
        let recovered = crate::updater::acquisition::recover_session(
            &fixture.updates_root,
            &fixture.session.id,
            &test_trust(),
            &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
        )
        .unwrap();
        let second = prepare_handoff(&recovered, &fixture.install_root).unwrap();
        assert_eq!(first.argv, second.argv, "frozen facts must not change");
        let envelope = load_update_session(&dir).unwrap();
        assert_eq!(envelope.generation, 2);
        // The original creation timestamp survives the resumed write.
        assert_eq!(envelope.created_at, first_envelope.created_at);
    }

    #[test]
    fn prepare_refuses_to_repoint_a_tampered_envelope() {
        let fixture = staged_fixture("conflict");
        prepare_handoff(&fixture.session, &fixture.install_root).unwrap();
        let dir = fixture
            .updates_root
            .join("sessions")
            .join(&fixture.session.id);
        let path = dir.join("session.json");
        let text = String::from_utf8(std::fs::read(&path).unwrap())
            .unwrap()
            .replace("\"toVersion\": \"1.4.0\"", "\"toVersion\": \"1.5.0\"");
        std::fs::write(&path, text).unwrap();
        match prepare_handoff(&fixture.session, &fixture.install_root) {
            Err(PrepareError::Publish(_)) => {}
            other => panic!("expected Publish conflict, got {other:?}"),
        }
    }

    #[test]
    fn prepare_reverifies_staged_executables() {
        let fixture = staged_fixture("staged-tamper");
        std::fs::write(
            fixture
                .updates_root
                .join("sessions")
                .join(&fixture.session.id)
                .join("staged/desktop-todo-widget.exe"),
            b"tampered",
        )
        .unwrap();
        match prepare_handoff(&fixture.session, &fixture.install_root) {
            Err(PrepareError::StagedVerification(_)) => {}
            other => panic!("expected StagedVerification, got {other:?}"),
        }
    }
}
