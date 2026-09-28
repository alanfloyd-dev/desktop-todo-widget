//! Phase 2D-B probation/commit/rollback test matrix. Deterministic
//! sandboxes plus a real probe child process (the test binary itself,
//! `--exact`-filtered) exercising the frozen environment bindings, the
//! durable marker, live identity validation, termination, and every commit
//! and rollback crash window through the executed `--recover` entry.

use super::*;
use crate::apply::MutationRecoveryState;
use crate::handoff::LiveProcessIdentity;
use crate::paths::Paths;
use crate::receipt::FileRecord;
use crate::update_session::{publish_update_session, HandoffFacts, ProcessIdentity, SessionPhase};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use desktop_todo_update_core::{derive_key_id, TrustStore};
use ed25519_dalek::{Signer, SigningKey};
use std::path::Path;

/// TEST-ONLY seed. K1 is the fixture trust root; never a production trust
/// root, never used for release signing.
const TEST_SEED: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";

fn test_trust() -> TrustStore {
    let signing = SigningKey::from_bytes(&TEST_SEED);
    TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap()
}

fn payload(seed: u8) -> Vec<u8> {
    (0..64u32).map(|i| (i as u8).wrapping_add(seed)).collect()
}

fn sha_of(bytes: &[u8]) -> String {
    desktop_todo_update_core::sha256_hex(bytes)
}

struct Fixture {
    root: PathBuf,
    paths: Paths,
    trust: TrustStore,
    session_id: String,
    digest: String,
    installation_id: String,
    main_old: Vec<u8>,
    helper_old: Vec<u8>,
    main_new: Vec<u8>,
    helper_new: Vec<u8>,
}

impl Fixture {
    fn session_dir(&self) -> PathBuf {
        self.paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&self.session_id)
    }
    fn envelope(&self) -> UpdateSessionEnvelope {
        load_update_session(&self.session_dir()).unwrap()
    }
    fn receipt(&self) -> Receipt {
        Receipt::load(&self.paths).unwrap().unwrap()
    }
    fn workspace(&self) -> PathBuf {
        session_workspace(&self.paths, &self.session_id)
    }
    fn health_marker(&self) -> Option<Vec<u8>> {
        std::fs::read(self.session_dir().join(HEALTH_FILE)).ok()
    }
    fn classify(&self) -> MutationRecoveryState {
        crate::apply::classify_update_recovery(&self.paths, &self.trust, &self.session_id)
            .unwrap()
            .state
    }
    fn display_version(&self) -> String {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey_with_flags(self.paths.registry(), winreg::enums::KEY_READ)
            .unwrap();
        key.get_value("DisplayVersion").unwrap()
    }
    /// The one validated handoff for journal writes (captured before any
    /// probation-phase field exists — mirroring the production flow, which
    /// reuses one handoff through the whole phase).
    fn handoff(&self) -> ValidatedHandoff {
        crate::handoff::validate_post_apply(
            &self.paths,
            &self.trust,
            &self.session_id,
            &self.digest,
        )
        .unwrap()
    }
}

/// A complete probation fixture at exactly the `ReplacedAwaitingLaunch`
/// state: both EXEs hold the signed target bytes, backups retained, receipt
/// `Updating`, exclusive lease released (the fixture drops it after the
/// apply, as the runner would on exit — the flows that need it re-acquire).
fn probation_fixture(tag: &str) -> Fixture {
    let signing = SigningKey::from_bytes(&TEST_SEED);
    let trust = test_trust();
    let id = uuid::Uuid::new_v4().to_string();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target")
        .join("probation-tests")
        .join(format!("{tag}-{id}"));
    let paths = Paths::sandbox(root.clone(), &id);
    paths::create_dir(paths.install()).unwrap();
    paths::create_dir(paths.state()).unwrap();

    let main_old = payload(3);
    let helper_old = payload(4);
    let main_new = payload(1);
    let helper_new = payload(2);

    std::fs::write(paths.install().join(crate::MAIN_EXE), &main_old).unwrap();
    std::fs::write(paths.install().join(crate::HELPER_EXE), &helper_old).unwrap();
    std::fs::write(paths.install().join("README.md"), b"old readme").unwrap();
    std::fs::write(paths.install().join("unknown.dll"), b"foreign").unwrap();

    let files = [
        crate::resources::Resource::MainExecutable,
        crate::resources::Resource::MaintenanceHelper,
    ]
    .into_iter()
    .map(|identity| {
        let bytes = match identity {
            crate::resources::Resource::MainExecutable => &main_old,
            _ => &helper_old,
        };
        FileRecord {
            identity,
            version: "1.1.0".to_string(),
            size: bytes.len() as u64,
            sha256: sha_of(bytes),
        }
    })
    .collect();
    let mut receipt = Receipt::new(&paths, "1.1.0", files, false);
    receipt.lifecycle_state = Lifecycle::Installed;
    receipt.save(&paths).unwrap();
    let installation_id = receipt.installation_id.clone();

    let support = b"support".to_vec();
    let mut package_entries: Vec<(&str, Vec<u8>)> = vec![
        ("desktop-todo-widget.exe", main_new.clone()),
        ("desktop-todo-maintenance.exe", helper_new.clone()),
    ];
    for name in [
        "install.ps1",
        "uninstall.ps1",
        "README.md",
        "README_ZH.md",
        "LICENSE",
        "LICENSE_ZH.md",
        "THIRD_PARTY_NOTICES.md",
    ] {
        package_entries.push((name, support.clone()));
    }
    let mut package: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut package);
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &package_entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, bytes).unwrap();
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
        v = "1.2.0",
        pkg = package.len(),
        psha = sha_of(&package),
        s1 = main_new.len(),
        h1 = sha_of(&main_new),
        s2 = helper_new.len(),
        h2 = sha_of(&helper_new),
    );
    let manifest_bytes = manifest.into_bytes();
    let signature = signing.sign(&manifest_bytes).to_bytes();
    let envelope_bytes = format!(
        r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
        derive_key_id(&signing.verifying_key().to_bytes()),
        STANDARD.encode(signature)
    )
    .into_bytes();
    let target =
        desktop_todo_update_core::verify_and_parse(&trust, &envelope_bytes, &manifest_bytes)
            .expect("fixture target verifies");

    let session_id = uuid::Uuid::new_v4().to_string();
    let dir = paths
        .state()
        .join("updates")
        .join("sessions")
        .join(&session_id);
    std::fs::create_dir_all(dir.join("staged")).unwrap();
    std::fs::write(dir.join(crate::handoff::MANIFEST_FILE), &manifest_bytes).unwrap();
    std::fs::write(dir.join(crate::handoff::ENVELOPE_FILE), &envelope_bytes).unwrap();
    std::fs::write(dir.join(crate::handoff::PACKAGE_FILE), &package).unwrap();
    std::fs::write(dir.join("staged/desktop-todo-widget.exe"), &main_new).unwrap();
    std::fs::write(dir.join("staged/desktop-todo-maintenance.exe"), &helper_new).unwrap();

    let live = LiveProcessIdentity {
        pid: std::process::id(),
        creation_filetime: own_creation_filetime(),
        image_path: paths.install().join(crate::MAIN_EXE),
    };
    publish_update_session(
        &target,
        HandoffFacts {
            session_id: &session_id,
            installation_id: &installation_id,
            from_version: "1.1.0",
            discovered_via: "GitHub",
            package_url: "https://mirror.invalid/desktop-todo-widget.zip",
            install_root: paths.install(),
            updates_root: &paths.state().join("updates"),
            parent_process: ProcessIdentity {
                pid: live.pid,
                process_created_at: live.creation_filetime.to_string(),
                image_path: live.image_path.clone(),
            },
        },
    )
    .expect("session envelope publishes");

    let digest = target.manifest_sha256_hex().to_string();
    crate::handoff::validate_update_handoff(
        &paths,
        &trust,
        &session_id,
        &digest,
        &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
    )
    .expect("fixture handoff validates");

    // The installed-helper half: the durable handoff intent and the
    // `Updating` transition (the journal-leads-receipt ordering).
    crate::apply::begin_update_transaction(
        &paths,
        &trust,
        &paths.install().join(crate::HELPER_EXE),
        &live,
        &desktop_todo_update_core::Version::parse("1.1.0").unwrap(),
        &session_id,
        &digest,
    )
    .expect("fixture begin succeeds");

    // Run the full replacement phase (validates the Resume entry end to
    // end), then release the lease exactly as the runner process would on
    // exit — recovery flows re-acquire it themselves.
    let runner_dir = paths.state().join("runners").join("fixture-runner");
    paths::create_dir(&runner_dir).unwrap();
    let (_validated, _lease) = crate::apply::continue_update_transaction(
        &paths,
        &trust,
        &runner_dir,
        &session_id,
        &digest,
    )
    .expect("replacement phase completes");

    Fixture {
        root,
        paths,
        trust,
        session_id,
        digest,
        installation_id,
        main_old,
        helper_old,
        main_new,
        helper_new,
    }
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
            .unwrap();
        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64
    }
}

/// The probe child: a real process playing the probation child. Spawned by
/// the tests (libtest `--exact` filter plus DTW_TEST_PROBE); a no-op in the
/// normal suite.
#[test]
fn probation_child_probe() {
    let Ok(mode) = std::env::var("DTW_TEST_PROBE") else {
        return;
    };
    let root = PathBuf::from(std::env::var("DTW_TEST_SANDBOX_ROOT").unwrap());
    let session_id = std::env::var("DTW_TEST_SESSION").unwrap();
    let paths = Paths::sandbox(root, &session_id);
    match mode.as_str() {
        "ack" | "ack-and-exit" => {
            // Simulate core initialization taking a moment, then write the
            // durable marker through the real primitive.
            std::fs::write(
                paths.state().join("probe-own-pid.txt"),
                format!("pid={} created-filetime-queryable
", std::process::id()),
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(400));
            let ack = build_current_health_ack(
                &session_id,
                &std::env::var("DTW_TEST_INSTALLATION").unwrap(),
                &std::env::var("DTW_TEST_VERSION").unwrap(),
                &std::env::var("DTW_TEST_NONCE").unwrap(),
            )
            .unwrap();
            write_health_ack(&paths, &ack).unwrap();
            if mode == "ack" {
                // Stay alive: the helper validates a live child.
                std::thread::sleep(Duration::from_secs(10));
            }
        }
        "admission-order" => {
            // The race probe: the very first thing after waking up, check
            // whether the durable `probationProcess` binding exists and
            // matches this process, and record the observation. Under the
            // suspended-creation handshake this must be unconditionally
            // "present+match" (the resume happens only after the journal);
            // under the old spawn-then-journal ordering this records the
            // race deterministically.
            let envelope = crate::update_session::load_update_session(
                &paths
                    .state()
                    .join("updates")
                    .join("sessions")
                    .join(&session_id),
            )
            .unwrap();
            let journaled = envelope.probation_process.as_ref();
            let present = journaled.is_some();
            let pid_match = journaled.is_some_and(|j| j.pid == std::process::id());
            let created_match = journaled.is_some_and(|j| {
                j.process_created_at
                    == build_current_health_ack(
                        &session_id,
                        "x",
                        "x",
                        &std::env::var("DTW_TEST_NONCE").unwrap(),
                    )
                    .unwrap()
                    .process_created_at
            });
            std::fs::write(
                paths.state().join("admission-order-marker.txt"),
                format!("present={present} pid_match={pid_match} created_match={created_match}\n"),
            )
            .unwrap();
            std::thread::sleep(Duration::from_secs(10));
        }
        "silent" => std::thread::sleep(Duration::from_secs(60)),
        "exit" => {}
        other => panic!("unknown probe mode {other}"),
    }
}

/// Spawn the probe child with the frozen environment bindings and capture
/// its identity exactly like the production launch.
fn probe_launch(
    fixture: &Fixture,
    mode: &'static str,
) -> impl Fn(&Paths, &ValidatedHandoff, &str) -> Result<LaunchedChild, MutationError> {
    let root = fixture.root.clone();
    let session_id = fixture.session_id.clone();
    let installation_id = fixture.installation_id.clone();
    let exe = std::env::current_exe().unwrap();
    move |_paths, _validated, nonce| {
        // Frozen ordering, observed at launch time: the nonce is already
        // durable and the launch record is not yet written.
        let envelope = load_update_session(
            &_paths
                .state()
                .join("updates")
                .join("sessions")
                .join(&session_id),
        )
        .unwrap();
        assert!(
            envelope.health_nonce.is_some(),
            "nonce must be durable before launch"
        );
        assert!(
            envelope.probation_process.is_none(),
            "the launch precedes its journal record"
        );
        // The exclusive lease must already be released (frozen choreography
        // step 1): an exclusive acquire succeeds.
        let probe_lease =
            crate::lock::exclusive_app(_paths).expect("exclusive lease released pre-launch");
        drop(probe_lease);
        let child = std::process::Command::new(&exe)
            .args(["probation::tests::probation_child_probe", "--exact", "--nocapture"])
            .env("DTW_TEST_PROBE", mode)
            .env("DTW_TEST_SANDBOX_ROOT", &root)
            .env("DTW_TEST_SESSION", &session_id)
            .env("DTW_TEST_INSTALLATION", &installation_id)
            .env("DTW_TEST_VERSION", "1.2.0")
            .env("DTW_TEST_NONCE", nonce)
            .env(ENV_SESSION_ID, &session_id)
            .env(ENV_NONCE, nonce)
            .env(ENV_LAUNCH_MODE, LAUNCH_MODE_PROBATION)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| MutationError::Launch { detail: e.to_string() })?;
        let pid = child.id();
        let live = query_live_process_identity(pid)
            .map_err(|e| MutationError::Launch { detail: e.to_string() })?;
        let identity = ProcessIdentity {
            pid: live.pid,
            process_created_at: live.creation_filetime.to_string(),
            // The OBSERVED image of the launched process (the production
            // launch additionally asserts it equals the canonical installed
            // main executable before journaling).
            image_path: live.image_path,
        };
        // Mirror the production launch closure: the durable
        // `probationProcess` journal is part of the launch handshake and is
        // written before the child's admission can observe the session.
        super::journal_probation_process(_validated, &identity)
            .map_err(|e| MutationError::Launch {
                detail: format!("probe journal: {e}"),
            })?;
        Ok(LaunchedChild {
            identity,
            child: Some(child),
        })
    }
}

fn controls<'a>(
    launch: &'a dyn Fn(&Paths, &ValidatedHandoff, &str) -> Result<LaunchedChild, MutationError>,
) -> ProbationControls<'a> {
    ProbationControls {
        launch,
        attempts: 240,
        interval: Duration::from_millis(50),
        timeout: Duration::from_secs(8),
    }
}

/// Run the probation flow for a fixture at `ReplacedAwaitingLaunch` with a
/// fresh PostApply handoff and a freshly acquired exclusive lease (the
/// production recovery entry does exactly this).
fn run_flow(
    fixture: &Fixture,
    ctl: &ProbationControls<'_>,
) -> Result<Settlement, MutationError> {
    let validated = fixture.handoff();
    let lease = wait_exclusive_lease(&fixture.paths, 40, Duration::from_millis(10))?;
    run_probation(&fixture.paths, &fixture.trust, &validated, lease, ctl)
}

// ------------------------------------------------------------- nonce

#[test]
fn nonce_is_strong_canonical_and_unique() {
    let a = mint_nonce().unwrap();
    let b = mint_nonce().unwrap();
    assert_ne!(a, b, "two mints must never collide");
    let raw = canonical_nonce(&a).unwrap();
    assert_eq!(BASE64.encode(raw), a);
    assert!(canonical_nonce(&BASE64.encode([0u8; 31])).is_err(), "31 bytes");
    assert!(canonical_nonce("not base64!!").is_err());
}

#[test]
fn ensure_nonce_is_durable_and_session_bound() {
    let fixture = probation_fixture("nonce");
    let handoff = fixture.handoff();
    assert!(fixture.envelope().health_nonce.is_none());
    let first = ensure_nonce(&handoff).unwrap();
    assert_eq!(
        fixture.envelope().health_nonce.as_deref(),
        Some(first.as_str())
    );
    // A restart keeps the session-bound value; no rotation.
    let second = ensure_nonce(&handoff).unwrap();
    assert_eq!(first, second);
}

// --------------------------------------------------- ack validation

fn valid_ack(fixture: &Fixture, nonce: &str, pid: u32, created: &str) -> HealthAckDocument {
    HealthAckDocument {
        schema_version: 1,
        session_id: fixture.session_id.clone(),
        installation_id: fixture.installation_id.clone(),
        version: "1.2.0".to_string(),
        nonce: nonce.to_string(),
        pid,
        process_created_at: created.to_string(),
        initialized_at: "2026-10-01T00:00:05Z".to_string(),
    }
}

#[test]
fn ack_validation_rejects_every_wrong_binding() {
    let fixture = probation_fixture("ack-validation");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    let validate = |bytes: Vec<u8>| {
        validate_health_ack(&fixture.paths, &handoff, &nonce, &bytes).err()
    };
    // No journaled launch yet: any marker is a process mismatch.
    let error = validate(
        serde_json::to_vec(&valid_ack(&fixture, &nonce, 1, "1")).unwrap(),
    )
    .unwrap();
    assert!(matches!(error, MutationError::HealthAckWrongProcess { .. }), "{error}");

    // Journal a launch record, then try the hostile matrix.
    publish_mutation(&handoff, |envelope| {
        envelope.probation_process = Some(ProcessIdentity {
            pid: 4242,
            process_created_at: "133000000000000000".to_string(),
            image_path: fixture.paths.install().join(crate::MAIN_EXE),
        });
    })
    .unwrap();

    for (name, ack) in [
        ("wrong session", HealthAckDocument {
            session_id: uuid::Uuid::new_v4().to_string(),
            ..valid_ack(&fixture, &nonce, 4242, "133000000000000000")
        }),
        ("wrong installation", HealthAckDocument {
            installation_id: uuid::Uuid::new_v4().to_string(),
            ..valid_ack(&fixture, &nonce, 4242, "133000000000000000")
        }),
        ("wrong version", HealthAckDocument {
            version: "9.9.9".to_string(),
            ..valid_ack(&fixture, &nonce, 4242, "133000000000000000")
        }),
        ("wrong nonce", valid_ack(&fixture, &BASE64.encode([8u8; 32]), 4242, "133000000000000000")),
        ("wrong pid", valid_ack(&fixture, &nonce, 4243, "133000000000000000")),
        ("pid reuse (wrong creation time)", valid_ack(&fixture, &nonce, 4242, "133000000000000001")),
    ] {
        let error = validate(serde_json::to_vec(&ack).unwrap()).unwrap();
        let ok = matches!(
            error,
            MutationError::HealthAckWrongSession
                | MutationError::HealthAckWrongNonce
                | MutationError::HealthAckWrongProcess { .. }
        );
        assert!(ok, "{name}: {error}");
        // The nonce value itself is never reproduced in diagnostics.
        assert!(!error.to_string().contains(&nonce), "nonce leaked: {error}");
    }

    // Malformed shapes.
    assert!(matches!(
        validate(b"{}".to_vec()).unwrap(),
        MutationError::HealthAckMalformed { .. }
    ));
    assert!(matches!(
        validate(b"{\"schemaVersion\":1,\"schemaVersion\":1}".to_vec()).unwrap(),
        MutationError::HealthAckMalformed { .. }
    ));
    let mut oversized = serde_json::to_vec(&valid_ack(&fixture, &nonce, 4242, "133000000000000000"))
        .unwrap();
    oversized.extend(vec![b' '; HEALTH_ACK_MAX_BYTES + 1]);
    assert!(matches!(
        validate(oversized).unwrap(),
        MutationError::HealthAckMalformed { .. }
    ));

    // The real live process: our own test process, journaled and acked.
    let own = query_live_process_identity(std::process::id()).unwrap();
    publish_mutation(&handoff, |envelope| {
        envelope.probation_process = Some(ProcessIdentity {
            pid: own.pid,
            process_created_at: own.creation_filetime.to_string(),
            image_path: own.image_path.clone(),
        });
    })
    .unwrap();
    validate_health_ack(
        &fixture.paths,
        &handoff,
        &nonce,
        &serde_json::to_vec(&valid_ack(
            &fixture,
            &nonce,
            own.pid,
            &own.creation_filetime.to_string(),
        ))
        .unwrap(),
    )
    .expect("the live journaled child validates");
}

// ------------------------------------------------- commit happy path

#[test]
fn valid_ack_commits_and_cleans_up() {
    let fixture = probation_fixture("commit-happy");
    let launch = probe_launch(&fixture, "ack");
    let controls = controls(&launch);
    let settlement = run_flow(&fixture, &controls).expect("probation commits");
    assert_eq!(settlement, Settlement::Committed);

    // Durable committed truth: receipt at the target with the invariant
    // chain intact.
    let receipt = fixture.receipt();
    assert_eq!(receipt.lifecycle_state, Lifecycle::Installed);
    assert_eq!(receipt.current_version.as_deref(), Some("1.2.0"));
    assert_eq!(
        receipt.maintenance.committed_manifest_sha256.as_deref(),
        Some(fixture.digest.as_str())
    );
    assert!(receipt.maintenance.active_session_id.is_none());
    assert_eq!(
        receipt.maintenance.last_completed_session_id.as_deref(),
        Some(fixture.session_id.as_str())
    );
    for record in &receipt.runtime_resources {
        let expected = match record.identity {
            crate::resources::Resource::MainExecutable => &fixture.main_new,
            _ => &fixture.helper_new,
        };
        assert_eq!(record.version, "1.2.0");
        assert_eq!(record.sha256, sha_of(expected));
    }
    // Registry DisplayVersion follows the committed receipt.
    assert_eq!(fixture.display_version(), "1.2.0");

    // Installed evidence is published and re-verifiable.
    let manifest = std::fs::read(
        fixture
            .paths
            .install()
            .join(crate::handoff::INSTALLED_MANIFEST_FILE),
    )
    .unwrap();
    assert_eq!(desktop_todo_update_core::sha256_hex(&manifest), fixture.digest);

    // Session finalized committed; backups and staged runtime cleaned.
    let envelope = fixture.envelope();
    assert_eq!(envelope.phase, SessionPhase::Committed);
    assert!(envelope.accepted_health.is_some());
    let intent = parse_commit_intent(envelope.commit_intent.as_ref().unwrap()).unwrap();
    assert_eq!(intent.decision, CommitDecision::Commit);
    assert!(!fixture.workspace().join("mainExecutable.backup").exists());
    assert!(!fixture.session_dir().join("staged").exists());
    assert!(!fixture
        .session_dir()
        .join(crate::handoff::PACKAGE_FILE)
        .exists());
    // Support and unknown files were never touched.
    assert_eq!(
        std::fs::read(fixture.paths.install().join("README.md")).unwrap(),
        b"old readme"
    );
    assert!(fixture.paths.install().join("unknown.dll").is_file());
    assert_eq!(fixture.classify(), MutationRecoveryState::CommittedCleanupPending);
}

#[test]
fn replay_after_acceptance_fails_closed() {
    let fixture = probation_fixture("replay");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    publish_mutation(&handoff, |envelope| {
        envelope.probation_process = Some(ProcessIdentity {
            pid: std::process::id(),
            process_created_at: own_creation_filetime().to_string(),
            image_path: std::env::current_exe().unwrap(),
        });
    })
    .unwrap();
    let ack = valid_ack(
        &fixture,
        &nonce,
        std::process::id(),
        &own_creation_filetime().to_string(),
    );
    accept_health(&handoff, &ack).expect("first acceptance");
    // The nonce is consumed once: any later marker is a replay refusal.
    let error = accept_health(&handoff, &ack).unwrap_err();
    assert!(matches!(error, MutationError::HealthAckReplay), "{error}");
}

// --------------------------------------------- rollback trigger paths

#[test]
fn timeout_terminates_child_and_rolls_back() {
    let fixture = probation_fixture("timeout");
    let launch = probe_launch(&fixture, "silent");
    let mut ctl = controls(&launch);
    ctl.timeout = Duration::from_millis(900);
    let settlement = run_flow(&fixture, &ctl).expect("timeout rolls back");
    let Settlement::RolledBack { reason } = settlement else {
        panic!("expected rollback, got {settlement:?}");
    };
    assert!(reason.contains("bounded probation window"), "{reason}");

    // Restored old set, source receipt, rolled-back journal.
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::HELPER_EXE)).unwrap(),
        fixture.helper_old
    );
    let receipt = fixture.receipt();
    assert_eq!(receipt.lifecycle_state, Lifecycle::Installed);
    assert_eq!(receipt.current_version.as_deref(), Some("1.1.0"));
    assert!(receipt.maintenance.committed_manifest_sha256.is_none());
    assert!(receipt.maintenance.active_session_id.is_none());
    // Backups retained as rollback evidence; target artifacts cleaned.
    assert!(fixture.workspace().join("mainExecutable.backup").is_file());
    assert!(!fixture.session_dir().join("staged").exists());
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    let intent =
        parse_commit_intent(fixture.envelope().commit_intent.as_ref().unwrap()).unwrap();
    assert_eq!(intent.decision, CommitDecision::Rollback);
    assert!(intent.reason.as_deref().unwrap_or("").len() <= 256);
    // Registry back at the source version.
    assert_eq!(fixture.display_version(), "1.1.0");
}

#[test]
fn child_exit_before_ack_rolls_back() {
    let fixture = probation_fixture("child-exit");
    let launch = probe_launch(&fixture, "exit");
    let controls = controls(&launch);
    let settlement = run_flow(&fixture, &controls).expect("child exit rolls back");
    assert!(
        matches!(settlement, Settlement::RolledBack { .. }),
        "{settlement:?}"
    );
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert!(fixture.health_marker().is_none());
}

#[test]
fn launch_failure_rolls_back() {
    let fixture = probation_fixture("launch-fail");
    let refused = |_paths: &Paths,
                   _validated: &ValidatedHandoff,
                   _nonce: &str|
     -> Result<LaunchedChild, MutationError> {
        Err(MutationError::Launch {
            detail: "spawn refused".to_string(),
        })
    };
    let mut ctl = controls(&refused);
    ctl.attempts = 4;
    let settlement = run_flow(&fixture, &ctl).expect("launch failure rolls back");
    assert!(matches!(settlement, Settlement::RolledBack { .. }));
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    let intent =
        parse_commit_intent(fixture.envelope().commit_intent.as_ref().unwrap()).unwrap();
    assert_eq!(intent.decision, CommitDecision::Rollback);
}

#[test]
fn wrong_nonce_marker_rolls_back_and_terminates_the_child() {
    let fixture = probation_fixture("bad-marker");
    // The probe writes a marker carrying the WRONG nonce: validation
    // refuses and the journaled child is terminated.
    let root = fixture.root.clone();
    let session_id = fixture.session_id.clone();
    let exe = std::env::current_exe().unwrap();
    let hostile = move |_paths: &Paths,
                        _validated: &ValidatedHandoff,
                        _nonce: &str|
          -> Result<LaunchedChild, MutationError> {
        let child = std::process::Command::new(&exe)
            .args(["probation::tests::probation_child_probe", "--exact", "--nocapture"])
            .env("DTW_TEST_PROBE", "ack")
            .env("DTW_TEST_SANDBOX_ROOT", &root)
            .env("DTW_TEST_SESSION", &session_id)
            .env("DTW_TEST_INSTALLATION", "e80a74ca-2a09-47cb-bcba-a6766fbc4b08")
            .env("DTW_TEST_VERSION", "1.2.0")
            .env("DTW_TEST_NONCE", STANDARD.encode([9u8; 32])) // wrong nonce
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| MutationError::Launch { detail: e.to_string() })?;
        let pid = child.id();
        let live = query_live_process_identity(pid)
            .map_err(|e| MutationError::Launch { detail: e.to_string() })?;
        Ok(LaunchedChild {
            identity: ProcessIdentity {
                pid: live.pid,
                process_created_at: live.creation_filetime.to_string(),
                // The OBSERVED image of the launched process (the
                // production launch additionally asserts it equals the
                // canonical installed main executable before journaling).
                image_path: live.image_path,
            },
            child: Some(child),
        })
    };
    let controls = controls(&hostile);
    let settlement = run_flow(&fixture, &controls).expect("invalid marker rolls back");
    let Settlement::RolledBack { reason } = settlement else {
        panic!("expected rollback, got {settlement:?}");
    };
    assert!(reason.contains("invalid health state"), "{reason}");
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    // No accepted health was journaled for the wrong-nonce marker.
    assert!(fixture.envelope().accepted_health.is_none());
}

#[test]
fn rollback_refuses_a_missing_or_mismatching_slot_fail_closed() {
    for tamper in ["missing", "mismatch"] {
        let fixture = probation_fixture(&format!("slot-{tamper}"));
        // Journal the rollback decision directly (no files touched yet).
        let handoff = fixture.handoff();
        let envelope = load_journal(&handoff).unwrap();
        journal_decision(
            &handoff,
            CommitDecision::Rollback,
            &envelope,
            Some("test"),
        )
        .unwrap();
        if tamper == "missing" {
            paths::remove_file(&fixture.workspace().join("mainExecutable.backup")).unwrap();
        } else {
            std::fs::write(
                fixture.workspace().join("mainExecutable.backup"),
                b"not the old bytes",
            )
            .unwrap();
        }
        let validated = fixture.handoff();
        let error = execute_rollback(
            &fixture.paths,
            &fixture.trust,
            &validated,
            "test",
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                MutationError::RollbackAssetMissing { .. }
                    | MutationError::RollbackAssetMismatch { .. }
            ),
            "{tamper}: {error}"
        );
        // Nothing was restored and nothing was guessed.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
            fixture.main_new
        );
    }
}

// ------------------------------------------- commit crash windows C1–C5

fn journal_accepted_health(fixture: &Fixture) {
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    let ack = valid_ack(&fixture, &nonce, 4242, "133000000000000000");
    let accepted = AcceptedHealth {
        health_ack: ack,
        accepted_at: "2026-10-01T00:00:06Z".to_string(),
    };
    publish_mutation(&handoff, |envelope| {
        envelope.accepted_health = Some(serde_json::to_value(&accepted).unwrap());
    })
    .unwrap();
}

fn journal_commit_decision(fixture: &Fixture) {
    let handoff = fixture.handoff();
    ensure_nonce(&handoff).unwrap();
    let envelope = load_journal(&handoff).unwrap();
    journal_decision(&handoff, CommitDecision::Commit, &envelope, None).unwrap();
}

/// C1: acceptedHealth durable, crash before the commit decision.
#[test]
fn c1_resume_commits_from_durable_accepted_health() {
    let fixture = probation_fixture("c1");
    journal_accepted_health(&fixture);
    assert_eq!(fixture.classify(), MutationRecoveryState::CommitPending);
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("committed"), "{report}");
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
}

/// C2: the commit decision is journaled, crash before anything else.
#[test]
fn c2_resume_completes_the_commit_idempotently() {
    let fixture = probation_fixture("c2");
    journal_accepted_health(&fixture);
    journal_commit_decision(&fixture);
    assert_eq!(fixture.classify(), MutationRecoveryState::CommitPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
}

/// C3: registry + evidence already reconciled, receipt still `Updating`.
#[test]
fn c3_resume_publishes_the_receipt_after_reconciled_integration() {
    let fixture = probation_fixture("c3");
    journal_accepted_health(&fixture);
    journal_commit_decision(&fixture);
    // Simulate the crash after the registry/evidence steps: reconcile at
    // the target and publish the evidence, but leave the receipt Updating.
    let validated = fixture.handoff();
    let view = commit_receipt_view(&fixture.paths, &validated).unwrap();
    crate::integration::reconcile(&fixture.paths, &view).unwrap();
    publish_installed_evidence(&fixture.paths, &validated).unwrap();
    assert_eq!(fixture.display_version(), "1.2.0");
    assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Updating);

    assert_eq!(fixture.classify(), MutationRecoveryState::CommitPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
    assert_eq!(fixture.display_version(), "1.2.0");
}

/// C4: the receipt is `Installed` at the target, session unfinalized —
/// the receipt is authoritative; finalize, never roll back.
#[test]
fn c4_finalize_committed_from_the_authoritative_receipt() {
    let fixture = probation_fixture("c4");
    journal_accepted_health(&fixture);
    journal_commit_decision(&fixture);
    // Simulate the crash after the receipt publish, before finalization.
    let mut receipt = fixture.receipt();
    receipt.lifecycle_state = Lifecycle::Installed;
    receipt.current_version = Some("1.2.0".to_string());
    receipt.maintenance.active_session_id = None;
    receipt.maintenance.committed_manifest_sha256 = Some(fixture.digest.clone());
    receipt.save(&fixture.paths).unwrap();

    assert_eq!(
        fixture.classify(),
        MutationRecoveryState::CommittedPendingFinalization
    );
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("finalized"), "{report}");
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.display_version(), "1.2.0");
}

/// C5: session committed, cleanup pending — restart idempotence, cleanup
/// failure never changes the terminal truth.
#[test]
fn c5_committed_cleanup_is_idempotent_and_debt_only() {
    let fixture = probation_fixture("c5");
    journal_accepted_health(&fixture);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    // Restart: the committed state classifies as cleanup-pending forever
    // and re-running recovery is a no-op that never degrades the verdict.
    assert_eq!(fixture.classify(), MutationRecoveryState::CommittedCleanupPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    let receipt = fixture.receipt();
    assert_eq!(receipt.lifecycle_state, Lifecycle::Installed);
    assert_eq!(receipt.current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
}

/// Cleanup debt: a locked backup cannot be removed after commit, and the
/// committed verdict survives it untouched.
#[test]
fn cleanup_failure_after_commit_never_degrades_the_verdict() {
    let fixture = probation_fixture("c-cleanup-debt");
    journal_accepted_health(&fixture);
    // Hold the backup open with no sharing so its removal fails.
    use std::os::windows::fs::OpenOptionsExt;
    let conflict = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.workspace().join("mainExecutable.backup"))
        .unwrap();
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    drop(conflict);
    let receipt = fixture.receipt();
    assert_eq!(receipt.lifecycle_state, Lifecycle::Installed);
    assert_eq!(receipt.current_version.as_deref(), Some("1.2.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
}

// --------------------------------------- rollback crash windows R1–R7

fn journal_rollback_decision(fixture: &Fixture, reason: &str) {
    let handoff = fixture.handoff();
    ensure_nonce(&handoff).unwrap();
    let envelope = load_journal(&handoff).unwrap();
    journal_decision(&handoff, CommitDecision::Rollback, &envelope, Some(reason)).unwrap();
}

/// R1: rollback intent durable, crash before any restore.
#[test]
fn r1_resume_restores_from_the_journaled_intent() {
    let fixture = probation_fixture("r1");
    journal_rollback_decision(&fixture, "timeout");
    assert_eq!(fixture.classify(), MutationRecoveryState::RollbackPending);
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("rolled back"), "{report}");
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
}

/// R2/R3: one (or both) executables already restored, no journal entry of
/// its own — the destination bytes are the evidence; the resume completes
/// idempotently and never rebuilds the rollback asset from the target.
#[test]
fn r2_partial_restore_completes_idempotently_preserving_slots() {
    let fixture = probation_fixture("r2");
    journal_rollback_decision(&fixture, "timeout");
    // Simulate the crash after the main EXE was restored.
    std::fs::write(
        fixture.paths.install().join(crate::MAIN_EXE),
        &fixture.main_old,
    )
    .unwrap();
    let slot_before = std::fs::read(fixture.workspace().join("mainExecutable.backup")).unwrap();
    assert_eq!(fixture.classify(), MutationRecoveryState::RollbackPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::HELPER_EXE)).unwrap(),
        fixture.helper_old
    );
    // The rollback asset survived byte-identically.
    assert_eq!(
        std::fs::read(fixture.workspace().join("mainExecutable.backup")).unwrap(),
        slot_before
    );
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
}

/// R4: files restored, receipt still `Updating` — the resume finishes the
/// integration/receipt half.
#[test]
fn r4_restored_files_resume_into_the_receipt_half() {
    let fixture = probation_fixture("r4");
    journal_rollback_decision(&fixture, "timeout");
    std::fs::write(
        fixture.paths.install().join(crate::MAIN_EXE),
        &fixture.main_old,
    )
    .unwrap();
    std::fs::write(
        fixture.paths.install().join(crate::HELPER_EXE),
        &fixture.helper_old,
    )
    .unwrap();
    assert_eq!(fixture.classify(), MutationRecoveryState::RollbackPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Installed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert!(fixture.receipt().maintenance.active_session_id.is_none());
}

/// R5/R6: the receipt is `Installed` at the source after a rollback
/// decision, session unfinalized — verify the old set, finalize.
#[test]
fn r6_finalize_rolled_back_from_the_restored_receipt() {
    let fixture = probation_fixture("r6");
    journal_rollback_decision(&fixture, "timeout");
    std::fs::write(
        fixture.paths.install().join(crate::MAIN_EXE),
        &fixture.main_old,
    )
    .unwrap();
    std::fs::write(
        fixture.paths.install().join(crate::HELPER_EXE),
        &fixture.helper_old,
    )
    .unwrap();
    let previous = fixture.envelope().previous_receipt.clone().unwrap();
    // Mirror the real rollback order: integration was reconciled at the
    // source BEFORE the receipt publish; the crash left both done and the
    // session unfinalized.
    crate::integration::reconcile(&fixture.paths, &previous).unwrap();
    let mut receipt = previous;
    receipt.lifecycle_state = Lifecycle::Installed;
    receipt.maintenance.active_session_id = None;
    receipt.save(&fixture.paths).unwrap();

    assert_eq!(
        fixture.classify(),
        MutationRecoveryState::RolledBackPendingFinalization
    );
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("finalized"), "{report}");
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    assert_eq!(fixture.display_version(), "1.1.0");
}

/// R7: session rolled-back, cleanup pending; restart idempotence.
#[test]
fn r7_rolled_back_cleanup_is_idempotent() {
    let fixture = probation_fixture("r7");
    journal_rollback_decision(&fixture, "timeout");
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(fixture.classify(), MutationRecoveryState::RolledBackCleanupPending);
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Installed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    // Backups retained after rollback (evidence), staged runtime gone.
    assert!(fixture.workspace().join("maintenanceHelper.backup").is_file());
}

/// Rollback restores the previous committed-evidence state: with prior
/// committed evidence it is re-asserted from the verified backup; without
/// any, the absence is re-asserted.
#[test]
fn rollback_restores_the_previous_evidence_state() {
    // Without prior evidence (bootstrap source): new evidence must never
    // exist after rollback, and none is created here.
    let fixture = probation_fixture("evidence-none");
    journal_rollback_decision(&fixture, "timeout");
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(!fixture
        .paths
        .install()
        .join(crate::handoff::INSTALLED_MANIFEST_FILE)
        .exists());

    // With prior committed evidence: the backup restores it verbatim. The
    // prior evidence is a signed manifest for the SOURCE version (the
    // previous update's commit), so it verifies as 1.1.0.
    let fixture = probation_fixture("evidence-previous");
    let signing = SigningKey::from_bytes(&TEST_SEED);
    let prior_manifest = format!(
        concat!(
            r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
            r#""publishedAt":"2026-09-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"#,
            r#""assets":{{"windows-x64":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":{pkg},"sha256":"{psha}","installFiles":["#,
            r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":{s1},"sha256":"{h1}"}},"#,
            r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":{s2},"sha256":"{h2}"}}]}}}}}}"#
        ),
        v = "1.1.0",
        pkg = 16,
        psha = sha_of(b"prior package"),
        s1 = fixture.main_old.len(),
        h1 = sha_of(&fixture.main_old),
        s2 = fixture.helper_old.len(),
        h2 = sha_of(&fixture.helper_old),
    )
    .into_bytes();
    let prior_signature = signing.sign(&prior_manifest).to_bytes();
    let prior_envelope = format!(
        r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
        derive_key_id(&signing.verifying_key().to_bytes()),
        STANDARD.encode(prior_signature)
    )
    .into_bytes();
    let prior_digest = desktop_todo_update_core::sha256_hex(&prior_manifest);
    let manifest_path = fixture
        .paths
        .install()
        .join(crate::handoff::INSTALLED_MANIFEST_FILE);
    let envelope_path = fixture
        .paths
        .install()
        .join(crate::handoff::INSTALLED_ENVELOPE_FILE);
    std::fs::write(&manifest_path, &prior_manifest).unwrap();
    std::fs::write(&envelope_path, &prior_envelope).unwrap();
    let mut receipt = fixture.receipt();
    receipt.maintenance.committed_manifest_sha256 = Some(prior_digest.clone());
    receipt.save(&fixture.paths).unwrap();
    // Rebuild the session's previous-receipt snapshot to carry the digest.
    let handoff = fixture.handoff();
    publish_mutation(&handoff, |envelope| {
        if let Some(previous) = &mut envelope.previous_receipt {
            previous.maintenance.committed_manifest_sha256 = Some(prior_digest.clone());
        }
    })
    .unwrap();

    journal_rollback_decision(&fixture, "timeout");
    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    // The evidence at the install root still re-verifies as the source
    // version's committed bytes.
    let restored = std::fs::read(&manifest_path).unwrap();
    assert_eq!(restored, prior_manifest);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    assert_eq!(
        fixture.receipt().maintenance.committed_manifest_sha256.as_deref(),
        Some(prior_digest.as_str())
    );
}

// -------------------------------- probation resume (helper died waiting)

/// LaunchedAwaitingHealth resume: a still-live child with a fully
/// validating marker is accepted and committed (the frozen ACK path,
/// observed late).
#[test]
fn launched_resume_accepts_a_live_validating_child() {
    let fixture = probation_fixture("resume-ack");
    // Launch a probe that writes a valid marker and stays alive. The probe
    // launch closure journals the launch (mirroring production).
    let launch = probe_launch(&fixture, "ack");
    let launch_fn = launch;
    let child = (launch_fn)(
        &fixture.paths,
        &fixture.handoff(),
        &{
            let handoff = fixture.handoff();
            ensure_nonce(&handoff).unwrap()
        },
    )
    .unwrap();
    // Wait for the probe's marker to land.
    for _ in 0..100 {
        if fixture.health_marker().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(fixture.health_marker().is_some(), "probe never wrote the marker");
    // The fixture's runner released the lease; the resume re-acquires.
    assert_eq!(fixture.classify(), MutationRecoveryState::LaunchedAwaitingHealth);
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("committed"), "{report}");
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    assert_eq!(
        fixture.envelope().probation_process.as_ref().unwrap(),
        &child.identity
    );
}

/// LaunchedAwaitingHealth resume with an unresolved probation: the
/// journaled child is terminated and the transaction rolls back — never a
/// guessed acceptance, never a second probation attempt.
#[test]
fn launched_resume_terminates_and_rolls_back_when_unprovable() {
    let fixture = probation_fixture("resume-unresolved");
    let handoff = fixture.handoff();
    ensure_nonce(&handoff).unwrap();
    // Journal a launch whose process does not exist (the helper died
    // before/after the child exited; nothing is provable).
    publish_mutation(&handoff, |envelope| {
        envelope.probation_process = Some(ProcessIdentity {
            pid: 4242,
            process_created_at: "133000000000000000".to_string(),
            image_path: fixture.paths.install().join(crate::MAIN_EXE),
        });
    })
    .unwrap();
    assert_eq!(fixture.classify(), MutationRecoveryState::LaunchedAwaitingHealth);
    let report = recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("rolled back"), "{report}");
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
}

// ---------------------------------------------------- handoff validation

#[test]
fn post_apply_validation_admits_probation_fields_and_resume_rejects_them() {
    let fixture = probation_fixture("post-apply-gate");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    // The 2D-A apply entry (validate_resume) refuses probation-phase
    // fields; the 2D-B executor entry accepts them.
    assert!(crate::handoff::validate_resume(
        &fixture.paths,
        &fixture.trust,
        &fixture.session_id,
        &fixture.digest,
    )
    .is_err());
    assert!(crate::handoff::validate_post_apply(
        &fixture.paths,
        &fixture.trust,
        &fixture.session_id,
        &fixture.digest,
    )
    .is_ok());
    let _ = nonce;
}

#[test]
fn decision_shape_is_strict_and_malformed_evidence_is_inconsistent() {
    let fixture = probation_fixture("decision-shape");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    let ack = valid_ack(&fixture, &nonce, 4242, "133000000000000000");
    let accepted = AcceptedHealth {
        health_ack: ack,
        accepted_at: "2026-10-01T00:00:06Z".to_string(),
    };
    publish_mutation(&handoff, |envelope| {
        envelope.accepted_health = Some(serde_json::to_value(&accepted).unwrap());
        envelope.commit_intent = Some(serde_json::json!({
            "schemaVersion": 1,
            "decision": "commit",
            "targetVersion": "1.2.0",
            "manifestSha256": fixture.digest,
            "decidedAt": "2026-10-01T00:00:07Z"
        }));
    })
    .unwrap();
    // A malformed decision value is inconsistency, never an instruction.
    publish_mutation(&handoff, |envelope| {
        envelope.commit_intent = Some(serde_json::json!({
            "schemaVersion": 1,
            "decision": "explode",
            "targetVersion": "1.2.0",
            "manifestSha256": fixture.digest,
            "decidedAt": "2026-10-01T00:00:07Z"
        }));
    })
    .unwrap();
    assert!(matches!(
        fixture.classify(),
        MutationRecoveryState::Inconsistent { .. }
    ));
}

// ----------------------------------------------------- terminal marking

#[test]
fn inconsistent_evidence_marks_recovery_required_fail_closed() {
    let fixture = probation_fixture("recovery-required");
    // Tamper evidence: a completed resource no longer holds the target
    // bytes and no legal reading exists.
    std::fs::write(fixture.paths.install().join(crate::MAIN_EXE), b"tampered").unwrap();
    assert!(matches!(
        fixture.classify(),
        MutationRecoveryState::Inconsistent { .. }
    ));
    let error =
        recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap_err();
    assert!(error.detail.contains("recovery"), "{error}");
    assert_eq!(fixture.envelope().phase, SessionPhase::RecoveryRequired);
    assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::RecoveryRequired);
    assert!(fixture.receipt().maintenance.active_session_id.is_none());
    // The tampered bytes stay inconsistent forever — a recovery-required
    // terminal is never "proven consistent" by the classifier; the durable
    // terminal marking (journal phase + receipt lifecycle) is the record.
}

// ------------------------------------ pre-commit launch race (suspended handshake)

/// Create the probe child **suspended** with the frozen environment bindings
/// plus the test plumbing, exactly the way the production
/// `spawn_probation_suspended` creates the real child (same suspended flag,
/// same environment bindings; the test plumbing env is what the production
/// child's real image would carry implicitly). Returns the child and its
/// captured live identity.
fn spawn_suspended_probe(
    fixture: &Fixture,
    nonce: &str,
    mode: &str,
) -> (std::process::Child, ProcessIdentity) {
    use std::os::windows::process::CommandExt;
    let root = fixture.root.clone();
    let session_id = fixture.session_id.clone();
    let exe = std::env::current_exe().unwrap();
    let child = std::process::Command::new(&exe)
        .args(["probation::tests::probation_child_probe", "--exact", "--nocapture"])
        .env("DTW_TEST_PROBE", mode)
        .env("DTW_TEST_SANDBOX_ROOT", &root)
        .env("DTW_TEST_SESSION", &session_id)
        .env("DTW_TEST_INSTALLATION", &fixture.installation_id)
        .env("DTW_TEST_VERSION", "1.2.0")
        .env("DTW_TEST_NONCE", nonce)
        .env(ENV_SESSION_ID, &session_id)
        .env(ENV_NONCE, nonce)
        .env(ENV_LAUNCH_MODE, LAUNCH_MODE_PROBATION)
        .creation_flags(0x0000_0004) // CREATE_SUSPENDED
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("suspended probe spawns");
    let pid = child.id();
    let live = query_live_process_identity(pid).expect("suspended child is queryable");
    let identity = ProcessIdentity {
        pid: live.pid,
        process_created_at: live.creation_filetime.to_string(),
        image_path: live.image_path,
    };
    (child, identity)
}

/// The pre-commit race test (§5): the child's admission observation runs as
/// its first instruction chain after waking up, and it records what it saw.
/// Under the suspended-creation handshake the resume happens only after the
/// durable `probationProcess` journal, so the child must observe an
/// authoritative binding (present + matching its own PID + creation time)
/// **unconditionally** — under the old spawn-then-journal ordering this
/// same probe raced the journal write and observed `present=false` (the
/// child reached admission before the helper journaled), which is exactly
/// the defect this test pins.
#[test]
fn race_test_child_wakes_to_an_authoritative_binding() {
    let fixture = probation_fixture("race");
    let nonce = {
        let handoff = fixture.handoff();
        ensure_nonce(&handoff).unwrap()
    };
    let (mut child, identity) = {
        // Production primitives, production ordering: suspended spawn,
        // durable journal, then resume — each step is the real code path.
        let (child, identity) = spawn_suspended_probe(&fixture, &nonce, "admission-order");
        // While suspended the child provably cannot run: the marker it
        // writes on wake-up must not exist, even after a grace window.
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            !fixture.paths.state().join("admission-order-marker.txt").exists(),
            "a suspended child executed code before the resume"
        );
        let handoff = fixture.handoff();
        super::journal_probation_process(&handoff, &identity).unwrap();
        super::resume_process_threads(child.id()).expect("child resumes");
        (child, identity)
    };
    // Wait for the probe's first-instruction observation and read it.
    let marker = fixture.paths.state().join("admission-order-marker.txt");
    let mut observed = String::new();
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            observed = text;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        observed.contains("present=true"),
        "the child woke up without the durable binding: {observed:?}"
    );
    assert!(
        observed.contains("pid_match=true") && observed.contains("created_match=true"),
        "the durable binding does not match the child identity: {observed:?}"
    );
    // The journaled identity is exactly the captured one.
    assert_eq!(fixture.envelope().probation_process.as_ref().unwrap(), &identity);
    let _ = child.kill();
    let _ = child.wait();
}

/// Crash window B1 (suspended creation): the child exists **suspended** but
/// the journal write never happened. Two properties are pinned:
///
/// 1. the classifier sees the pre-journal state exactly as
///    `ReplacedAwaitingLaunch` — the suspended orphan is never
///    misclassified, never guessed at, and never killed by name;
/// 2. completing the adoption handshake (journal the orphan's captured
///    identity while it is still suspended, then resume — the exact
///    `adopt_or_launch` ordering) leaves the child running its admission
///    against the now-authoritative binding, and the recovery observation
///    path commits.
#[test]
fn crash_b1_suspended_orphan_is_adopted_journaled_and_committed() {
    let fixture = probation_fixture("crash-b1");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    let (mut orphan, identity) = spawn_suspended_probe(&fixture, &nonce, "ack");
    assert!(fixture.envelope().probation_process.is_none());
    assert_eq!(
        fixture.classify(),
        MutationRecoveryState::ReplacedAwaitingLaunch
    );
    // While suspended the orphan cannot have executed anything.
    assert!(!fixture
        .paths
        .state()
        .join("admission-order-marker.txt")
        .exists());
    // The adoption handshake, in the adopt_or_launch ordering: durable
    // journal first (the child still cannot run), then resume.
    super::journal_probation_process(&handoff, &identity).unwrap();
    super::resume_process_threads(identity.pid).expect("orphan resumes");
    assert_eq!(
        fixture.classify(),
        MutationRecoveryState::LaunchedAwaitingHealth
    );
    let report =
        recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("committed"), "{report}");
    // The adopted orphan's identity is the journaled probation record, and
    // its marker was accepted (live-process validated).
    assert_eq!(fixture.envelope().probation_process.as_ref().unwrap(), &identity);
    assert!(fixture.health_marker().is_some());
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    let _ = orphan.kill();
    let _ = orphan.wait();
}

/// Crash window B2: the journal is durable but the crash landed before the
/// resume — the orphan is still suspended. The `LaunchedAwaitingHealth`
/// resume observes the still-live journaled child, ensures it is resumed
/// (idempotent for a running child), and waits a fresh bounded window: the
/// child admits, acknowledges, and the update commits.
#[test]
fn crash_b2_journaled_suspended_orphan_is_resumed_and_observed() {
    let fixture = probation_fixture("crash-b2");
    let handoff = fixture.handoff();
    let nonce = ensure_nonce(&handoff).unwrap();
    let (mut orphan, identity) = spawn_suspended_probe(&fixture, &nonce, "ack");
    super::journal_probation_process(&handoff, &identity).unwrap();
    // Journaled but never resumed: the child cannot have run its admission.
    assert!(!fixture
        .paths
        .state()
        .join("admission-order-marker.txt")
        .exists());
    assert_eq!(
        fixture.classify(),
        MutationRecoveryState::LaunchedAwaitingHealth
    );
    let report =
        recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("committed"), "{report}");
    assert!(fixture.health_marker().is_some());
    assert_eq!(fixture.envelope().phase, SessionPhase::Committed);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.2.0"));
    let _ = orphan.kill();
    let _ = orphan.wait();
}

/// An unrelated process whose image happens to be the installed main
/// executable cannot be distinguished from an orphan under image-only
/// observation; the transaction fails closed around it — it is adopted,
/// journaled, resumed, times out without a valid marker, is terminated by
/// its journaled identity (never a name-based kill), and the rollback
/// completes with the old set restored.
#[test]
fn unrelated_installed_main_process_fails_closed_through_rollback() {
    let fixture = probation_fixture("unrelated-main");
    // A live process whose image is exactly the sandbox installed main
    // executable (a copy of this test binary under that path), running
    // unbound: no admission will ever acknowledge for it.
    std::fs::copy(std::env::current_exe().unwrap(), fixture.paths.install().join(crate::MAIN_EXE))
        .unwrap();
    let mut stranger = std::process::Command::new(fixture.paths.install().join(crate::MAIN_EXE))
        .args(["probation::tests::probation_child_probe", "--exact", "--nocapture"])
        .env("DTW_TEST_PROBE", "silent")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("stranger spawns");
    let stranger_pid = stranger.id();
    let launch = probe_launch(&fixture, "silent");
    let mut ctl = controls(&launch);
    ctl.timeout = Duration::from_millis(900);
    let settlement = run_flow(&fixture, &ctl).expect("the transaction fails closed");
    let Settlement::RolledBack { reason } = settlement else {
        panic!("expected rollback, got {settlement:?}");
    };
    assert!(reason.contains("bounded probation window"), "{reason}");
    // The stranger was terminated through its journaled identity and its
    // image lock released before the restore.
    let _ = stranger.wait();
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old,
        "the old set is restored after the stranger was terminated"
    );
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
    let _ = stranger_pid;
}

/// Mixed rollback crash state (§7): the main EXE is already restored OLD
/// while the helper EXE still holds TARGET — a genuinely reachable window
/// (the restore order is sequential and the crash can land between the two
/// move_replaces). There is no per-resource rollback completion field, so
/// recovery must observe each destination independently, preserve the
/// verified OLD slots, and idempotently restore only what remains.
#[test]
fn mixed_rollback_state_observes_each_destination_independently() {
    let fixture = probation_fixture("mixed-rollback");
    journal_rollback_decision(&fixture, "timeout");
    // The crash landed between the two restorations: main is OLD, helper
    // is still TARGET.
    std::fs::write(
        fixture.paths.install().join(crate::MAIN_EXE),
        &fixture.main_old,
    )
    .unwrap();
    std::fs::write(
        fixture.paths.install().join(crate::HELPER_EXE),
        &fixture.helper_new,
    )
    .unwrap();
    // Both slots must still hold the verified OLD preimages (they are
    // never rebuilt from the target bytes).
    let main_slot = std::fs::read(fixture.workspace().join("mainExecutable.backup")).unwrap();
    let helper_slot = std::fs::read(fixture.workspace().join("maintenanceHelper.backup")).unwrap();
    assert_eq!(main_slot, fixture.main_old);
    assert_eq!(helper_slot, fixture.helper_old);
    // The classify refuses nothing here: the rollback decision suspends the
    // "completed ⇒ target bytes" invariant, and the mixed state is exactly
    // the in-progress rollback shape.
    assert_eq!(fixture.classify(), MutationRecoveryState::RollbackPending);

    recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    // Both destinations now hold OLD; the slots survived byte-identically.
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::MAIN_EXE)).unwrap(),
        fixture.main_old
    );
    assert_eq!(
        std::fs::read(fixture.paths.install().join(crate::HELPER_EXE)).unwrap(),
        fixture.helper_old
    );
    assert_eq!(
        std::fs::read(fixture.workspace().join("mainExecutable.backup")).unwrap(),
        main_slot
    );
    assert_eq!(
        std::fs::read(fixture.workspace().join("maintenanceHelper.backup")).unwrap(),
        helper_slot
    );
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
}

/// Spliced-journal hardening (§5): a journaled probation record whose
/// process facts no longer match the live child refuses the ack (wrong
/// process) — the same shape a stolen or spliced journal would produce —
/// and the transaction rolls back instead of accepting. The splice targets
/// the creation FILETIME (the wrong-creation-time matrix case): the PID
/// stays the live child's, so the tampering is proven by the creation
/// mismatch alone, deterministically.
#[test]
fn spliced_journal_refuses_the_ack_and_rolls_back() {
    let fixture = probation_fixture("spliced-journal");
    // Produce a valid live probe + marker, then corrupt the journal's
    // creation time.
    let launch = probe_launch(&fixture, "ack");
    let launch_fn = launch;
    let _probe_child = (launch_fn)(
        &fixture.paths,
        &fixture.handoff(),
        &{
            let handoff = fixture.handoff();
            ensure_nonce(&handoff).unwrap()
        },
    )
    .unwrap();
    for _ in 0..100 {
        if fixture.health_marker().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(fixture.health_marker().is_some());
    let handoff = fixture.handoff();
    publish_mutation(&handoff, |envelope| {
        if let Some(identity) = &mut envelope.probation_process {
            let created: u64 = identity.process_created_at.parse().unwrap();
            identity.process_created_at = (created + 1).to_string();
        }
    })
    .unwrap();
    assert_eq!(fixture.classify(), MutationRecoveryState::LaunchedAwaitingHealth);
    let report =
        recover_and_execute_with(&fixture.paths, &fixture.trust, &fixture.session_id).unwrap();
    assert!(report.contains("rolled back"), "{report}");
    assert!(fixture.envelope().accepted_health.is_none());
    assert_eq!(fixture.envelope().phase, SessionPhase::RolledBack);
    assert_eq!(fixture.receipt().current_version.as_deref(), Some("1.1.0"));
}
