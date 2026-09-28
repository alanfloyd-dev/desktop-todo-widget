//! The Phase 2D-B probation half: health-nonce minting, the probation
//! launch, `PostUpdateProbation` health acceptance, and the commit/rollback
//! decision — the frozen protocol v1 "HealthAck" machinery plus the
//! receipt/journal write-ordering lattice for commit and rollback.
//!
//! Frozen boundaries implemented here (protocol v1 and application-
//! lifecycle §8–§10; nothing here invents protocol fields):
//!
//! - **Probation lease choreography.** The transaction holds the gate and
//!   the exclusive application lease through the replacements; the
//!   probation-ready record (the durable session `healthNonce` and the
//!   previous-evidence backup) is persisted **before** the exclusive lease
//!   is released, and no runtime mutation happens after that until a
//!   terminal verdict. The admitted child takes the ordinary shared lease;
//!   the ACK path never reacquires the exclusive lease; the failure path
//!   terminates the exact child, waits for it to be gone and the lease
//!   released, and only then reacquires the exclusive lease to roll back.
//! - **The nonce is single-use, durable, and never on a command line.** It
//!   is minted from the OS CSPRNG (256 bits), stored in the durable session
//!   journal, passed to the child only through the child's own environment
//!   block, validated against the durable value on every marker, and
//!   consumed once by the durable `acceptedHealth` record. It is never
//!   logged and never reproduced in error text.
//! - **`commitIntent` sub-shape (frozen by this phase, its first
//!   writer).** The envelope field name is frozen protocol; this phase
//!   fixes the typed value: `{schemaVersion, decision, targetVersion,
//!   manifestSha256, decidedAt, reason?}` where `decision` is `commit` or
//!   `rollback`. It is the durable decision boundary for both terminal
//!   directions — a crash gap on either side has exactly one legal reading
//!   through it.
//! - **`acceptedHealth` sub-shape (frozen by this phase, its first
//!   writer).** `{healthAck: <the exact frozen HealthAck document>,
//!   acceptedAt}`. Commit is impossible before this record is durable:
//!   write order is validate ack → journal acceptedHealth → read back →
//!   only then the decision.
//! - **Commit ordering (frozen).** (1) journal `commitIntent{commit}`;
//!   (2) fresh-verify the installed target set; (3) reconcile Windows
//!   integration at the target version; (4) publish the installed signed
//!   evidence; (5) receipt → `Installed` at the target (`currentVersion`,
//!   runtime facts, committed-evidence digest, `activeSessionId` cleared in
//!   one atomic publish — the receipt always follows verified state);
//!   (6) session finalized `committed`; (7) bounded cleanup only after the
//!   durable committed truth, with failures recorded as cleanup debt that
//!   never degrades the verdict.
//! - **Rollback ordering (frozen).** (1) journal `commitIntent{rollback}`
//!   before any side effect; (2) verify every rollback slot against the
//!   journaled preimage and restore exactly the two managed executables
//!   (never derived from the target bytes; a missing or mismatching asset
//!   fails closed); (3) reconcile integration at the source version;
//!   (4) restore the previous committed-evidence state; (5) receipt →
//!   `Installed` at the previous version from the journaled previous
//!   receipt; (6) session finalized `rolled-back`; (7) bounded cleanup.
//! - **One recovery entrypoint.** `--recover --session-id <UUID>` executes
//!   the frozen-safe action the read-only classifier names: resume apply,
//!   resume/observe probation, resume commit, resume rollback, finalize a
//!   terminal receipt, or mark `recovery-required` on inconsistency. It
//!   never guesses an ambiguous state.
//!
//! The probation timeout is implementation policy, not a protocol
//! invariant: finite, measured monotonically from process creation, 60 s
//! default, never session-controlled.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use desktop_todo_update_core::TrustStore;
use serde::{Deserialize, Serialize};

use crate::apply::{
    self, backup_slot_path, copy_verified_to, hash_stream, io_err, load_journal,
    open_checked, publish_mutation, session_workspace, wait_exclusive_lease,
    MutationError, MutationRecoveryState,
};
use crate::handoff::{
    self, query_live_process_identity, read_bounded, validate_post_apply, ValidatedHandoff,
};
use crate::lock::{AppLease, Gate};
use crate::paths::{self, Paths};
use crate::receipt::{validate_uuid, Lifecycle, Receipt};
use crate::resources::Resource;
use crate::update_session::{
    load_update_session, ProcessIdentity, SessionPhase, UpdateSessionEnvelope, SESSION_FILE,
};
use crate::{apply::assert_no_reparse, update_session::SessionErrorRecord};
use crate::{Error, ErrorKind, HELPER_EXE, MAIN_EXE};

/// The frozen child environment bindings (protocol v1 "HealthAck"): the
/// child receives these, and a fixed-purpose launch mode, only in its own
/// environment block — never on a command line, never forwarded.
pub const ENV_SESSION_ID: &str = "DTW_MAINTENANCE_SESSION_ID";
pub const ENV_NONCE: &str = "DTW_MAINTENANCE_NONCE";
/// The fixed-purpose launch mode marker. The protocol fixes that a
/// fixed-purpose launch mode travels in the environment block; the variable
/// name and its single fixed value are the implementation's naming of it.
pub const ENV_LAUNCH_MODE: &str = "DTW_MAINTENANCE_LAUNCH_MODE";
pub const LAUNCH_MODE_PROBATION: &str = "post-update-probation";

/// The durable health marker inside the session directory (application-
/// lifecycle §4 session layout: "... health").
pub const HEALTH_FILE: &str = "health.json";
/// HealthAck size bound (protocol v1 "Encoding and validation").
pub const HEALTH_ACK_MAX_BYTES: usize = 4 * 1024;
/// Private workspace names for the previous committed-evidence backup taken
/// before the exclusive lease is released. Not protocol fields: session-
/// workspace files of this transaction, restored (or their absence
/// re-asserted) by rollback.
pub const PREVIOUS_MANIFEST_BACKUP: &str = "installed-manifest.json.previous";
pub const PREVIOUS_ENVELOPE_BACKUP: &str = "installed-manifest.json.sig.previous";

/// Probation window (implementation policy, not a protocol invariant):
/// finite, measured monotonically from successful process creation. The
/// protocol fixes the invariant, not the number; QA may tune it before
/// protocol 1 ships.
pub const PROBATION_TIMEOUT: Duration = Duration::from_secs(60);
/// Production poll cadence for the durable health marker.
const PROBATION_ATTEMPTS: u32 = PROBATION_TIMEOUT.as_millis() as u32 / 250;
const PROBATION_INTERVAL: Duration = Duration::from_millis(250);
/// Bounded wait when reacquiring the exclusive lease on the rollback path
/// (the child was terminated first, so its lease is already released or
/// about to be).
const ROLLBACK_LEASE_ATTEMPTS: u32 = 40; // 40 × 250 ms = 10 s

// ------------------------------------------------------- frozen sub-shapes

/// The frozen HealthAck wire document (protocol v1 "HealthAck", schema 1,
/// closed). The child writes it after core initialization; the helper
/// validates it against the durable session facts. `deny_unknown_fields`
/// also rejects duplicate object members, per the uniform protocol policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HealthAckDocument {
    pub schema_version: u32,
    pub session_id: String,
    pub installation_id: String,
    pub version: String,
    /// Canonical standard-alphabet base64 of exactly 32 nonce bytes.
    pub nonce: String,
    pub pid: u32,
    /// Windows process creation FILETIME as a decimal string.
    pub process_created_at: String,
    /// UTC RFC 3339 timestamp. Metadata, never proof.
    pub initialized_at: String,
}

/// The durable `acceptedHealth` value — typed sub-shape frozen by this
/// phase, its first writer. Carries the exact accepted marker plus the
/// acceptance fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AcceptedHealth {
    #[serde(rename = "healthAck")]
    pub health_ack: HealthAckDocument,
    pub accepted_at: String,
}

/// The terminal decision. `commitIntent` is the envelope's frozen decision
/// boundary; this phase fixes its typed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitDecision {
    Commit,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CommitIntent {
    pub schema_version: u32,
    pub decision: CommitDecision,
    pub target_version: String,
    pub manifest_sha256: String,
    pub decided_at: String,
    /// Bounded, privacy-filtered reason (rollback only). Never includes a
    /// nonce, user content, or raw paths beyond the compiled install root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

const DECISION_SCHEMA_VERSION: u32 = 1;

/// Parse the durable `commitIntent` value. `Ok(None)` for absence; a
/// present-but-malformed value is an error the classifier reports as
/// inconsistency — never interpreted leniently.
pub fn parse_commit_intent(value: &serde_json::Value) -> Result<CommitIntent, String> {
    let intent: CommitIntent = serde_json::from_value(value.clone())
        .map_err(|e| e.to_string())?;
    if intent.schema_version != DECISION_SCHEMA_VERSION {
        return Err(format!("unsupported decision schema {}", intent.schema_version));
    }
    Ok(intent)
}

/// Parse and bind the durable `acceptedHealth` value against the journal's
/// frozen facts.
fn parse_accepted_health(
    envelope: &UpdateSessionEnvelope,
    value: &serde_json::Value,
) -> Result<AcceptedHealth, MutationError> {
    let accepted: AcceptedHealth = serde_json::from_value(value.clone())
        .map_err(|e| MutationError::RecoveryInconsistent {
            detail: format!("durable acceptedHealth is malformed: {e}"),
        })?;
    let ack = &accepted.health_ack;
    if ack.session_id != envelope.session_id
        || ack.installation_id != envelope.installation_id
        || ack.version != envelope.to_version
    {
        return Err(MutationError::RecoveryInconsistent {
            detail: "durable acceptedHealth bindings disagree with the journal".to_string(),
        });
    }
    let nonce = envelope
        .health_nonce
        .as_deref()
        .ok_or_else(|| MutationError::RecoveryInconsistent {
            detail: "durable acceptedHealth without any journaled nonce".to_string(),
        })?;
    if ack.nonce != nonce {
        return Err(MutationError::RecoveryInconsistent {
            detail: "durable acceptedHealth nonce disagrees with the journal".to_string(),
        });
    }
    Ok(accepted)
}

/// A canonical health nonce: standard-alphabet base64 of exactly 32 bytes
/// whose re-encoding reproduces the text exactly.
fn canonical_nonce(text: &str) -> Result<[u8; 32], MutationError> {
    nonce_bytes(text).map_err(|()| MutationError::HealthAckMalformed {
        detail: "the nonce is not canonical base64 of exactly 32 bytes".to_string(),
    })
}

/// Public nonce canonicality check (the main application's probation
/// admission validates the environment binding with it): `Err(())` for any
/// non-canonical form. The value itself never travels in an error.
pub fn canonical_nonce_bytes(text: &str) -> Result<[u8; 32], ()> {
    nonce_bytes(text)
}

fn nonce_bytes(text: &str) -> Result<[u8; 32], ()> {
    let bytes = BASE64.decode(text).map_err(|_| ())?;
    let array: [u8; 32] = bytes.try_into().map_err(|_| ())?;
    if BASE64.encode(array) != text {
        return Err(());
    }
    Ok(array)
}

/// Stream the size and SHA-256 of an already opened regular file — the
/// installed-target fact check the main application's probation admission
/// performs before admitting the child.
pub fn hash_file_facts(file: &mut std::fs::File) -> (u64, String) {
    crate::apply::hash_stream(file).unwrap_or((0, String::new()))
}

/// Mint the session health nonce from the OS CSPRNG: 256 random bits,
/// canonical base64. Never a hand-rolled PRNG, never derived from the
/// session id, version, or time.
fn mint_nonce() -> Result<String, MutationError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| MutationError::NonceGeneration {
        detail: e.to_string(),
    })?;
    Ok(BASE64.encode(bytes))
}

/// Ensure the durable `healthNonce` exists for this session. Idempotent: a
/// restart after the nonce was journaled keeps the existing value (the
/// nonce binds the whole session, so a rotation could only invalidate an
/// already-launched child).
fn ensure_nonce(validated: &ValidatedHandoff) -> Result<String, MutationError> {
    let envelope = load_journal(validated)?;
    if let Some(existing) = &envelope.health_nonce {
        canonical_nonce(existing)?;
        return Ok(existing.clone());
    }
    let nonce = mint_nonce()?;
    publish_mutation(validated, |envelope| {
        envelope.health_nonce = Some(nonce.clone());
    })?;
    Ok(nonce)
}

// ------------------------------------------------------- evidence backup

/// Whether the installation had committed signed evidence before this
/// transaction's commit.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EvidenceBackup {
    /// No committed evidence existed (bootstrap-only installation):
    /// rollback re-asserts the absence.
    Absent,
    /// The previous committed evidence was copied into the session
    /// workspace, cryptographically verified against the journaled previous
    /// receipt before the exclusive lease was released.
    Present,
}

/// Verify that raw manifest/envelope bytes are the committed evidence of
/// `version` with digest `digest`, through this binary's own compiled trust
/// store.
fn verify_committed_evidence(
    trust: &TrustStore,
    manifest_bytes: &[u8],
    envelope_bytes: &[u8],
    version: &str,
    digest: &str,
) -> Result<(), MutationError> {
    let target = desktop_todo_update_core::verify_and_parse(trust, envelope_bytes, manifest_bytes)
        .map_err(|e| MutationError::RecoveryInconsistent {
            detail: format!("committed evidence fails re-verification: {e}"),
        })?;
    if desktop_todo_update_core::sha256_hex(manifest_bytes) != digest {
        return Err(MutationError::RecoveryInconsistent {
            detail: "committed evidence digest disagrees with the receipt".to_string(),
        });
    }
    if target.manifest().version().to_string() != version {
        return Err(MutationError::RecoveryInconsistent {
            detail: "committed evidence target disagrees with the receipt version".to_string(),
        });
    }
    Ok(())
}

/// Backup the installation's committed signed evidence (if any) into the
/// session workspace, verifying it cryptographically first. Runs before the
/// exclusive lease is released; it is a copy into maintenance's own
/// workspace, never a runtime mutation. Idempotent on resume: an existing
/// verified backup is kept.
fn backup_previous_evidence(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
) -> Result<EvidenceBackup, MutationError> {
    let envelope = load_journal(validated)?;
    let previous = envelope.previous_receipt.clone().ok_or_else(|| {
        MutationError::RecoveryInconsistent {
            detail: "the journaled previous receipt is missing".to_string(),
        }
    })?;
    let manifest_path = paths.install().join(crate::handoff::INSTALLED_MANIFEST_FILE);
    let envelope_path = paths.install().join(crate::handoff::INSTALLED_ENVELOPE_FILE);
    let digest = previous
        .maintenance
        .committed_manifest_sha256
        .clone()
        .filter(|d| !d.is_empty());
    if digest.is_none() {
        if manifest_path.exists() || envelope_path.exists() {
            return Err(MutationError::RecoveryInconsistent {
                detail: "committed evidence exists without any receipt record of it".to_string(),
            });
        }
        return Ok(EvidenceBackup::Absent);
    }
    if !manifest_path.is_file() || !envelope_path.is_file() {
        return Err(MutationError::RecoveryInconsistent {
            detail: "the receipt records committed evidence that is not installed".to_string(),
        });
    }
    let manifest_bytes = read_bounded(&manifest_path, "committed manifest")
        .map_err(|e| MutationError::RecoveryInconsistent {
            detail: e.to_string(),
        })?;
    let envelope_bytes = read_bounded(&envelope_path, "committed envelope")
        .map_err(|e| MutationError::RecoveryInconsistent {
            detail: e.to_string(),
        })?;
    let version = previous
        .current_version
        .clone()
        .ok_or_else(|| MutationError::RecoveryInconsistent {
            detail: "the previous receipt carries no version".to_string(),
        })?;
    verify_committed_evidence(trust, &manifest_bytes, &envelope_bytes, &version, &digest.unwrap())?;

    let workspace = session_workspace(paths, validated.session_id());
    paths::create_dir(&workspace).map_err(|e| io_err("evidence workspace", e))?;
    for (source, name) in [
        (&manifest_path, PREVIOUS_MANIFEST_BACKUP),
        (&envelope_path, PREVIOUS_ENVELOPE_BACKUP),
    ] {
        let destination = workspace.join(name);
        if destination.is_file() {
            // Idempotent resume: keep an existing backup only when it still
            // verifies; otherwise refresh it (it is this transaction's own
            // asset, unlike the managed rollback slots which are never
            // rewritten).
            let mut file = open_checked(&destination)?;
            let facts = hash_stream(&mut file)?;
            let mut origin = open_checked(source)?;
            let origin_facts = hash_stream(&mut origin)?;
            if facts == origin_facts {
                continue;
            }
            paths::remove_file(&destination).map_err(|e| io_err("evidence backup refresh", e))?;
        }
        let measured = {
            let mut origin = open_checked(source)?;
            hash_stream(&mut origin)?
        };
        copy_verified_to(source, &destination, measured, name)
            .map_err(|e| io_err("evidence backup", e))?;
    }
    Ok(EvidenceBackup::Present)
}

/// Restore the previous committed-evidence state during rollback: re-assert
/// the verified backup (idempotent) or re-assert the absence.
fn restore_evidence(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
) -> Result<(), MutationError> {
    let envelope = load_journal(validated)?;
    let previous = envelope.previous_receipt.clone().ok_or_else(|| {
        MutationError::RecoveryInconsistent {
            detail: "the journaled previous receipt is missing".to_string(),
        }
    })?;
    let manifest_path = paths.install().join(crate::handoff::INSTALLED_MANIFEST_FILE);
    let envelope_path = paths.install().join(crate::handoff::INSTALLED_ENVELOPE_FILE);
    let digest = previous
        .maintenance
        .committed_manifest_sha256
        .clone()
        .filter(|d| !d.is_empty());
    let Some(digest) = digest else {
        // The installation had no committed evidence: re-assert the
        // absence. A failure here leaves receipt/evidence disagreeing, so
        // it is a rollback failure, not a warning.
        for path in [&manifest_path, &envelope_path] {
            if path.exists() {
                paths::remove_file(path)
                    .map_err(|e| MutationError::EvidenceRollback { detail: e.to_string() })?;
            }
        }
        return Ok(());
    };
    let workspace = session_workspace(paths, validated.session_id());
    let version = previous
        .current_version
        .clone()
        .ok_or_else(|| MutationError::RecoveryInconsistent {
            detail: "the previous receipt carries no version".to_string(),
        })?;
    for (name, destination) in [
        (PREVIOUS_MANIFEST_BACKUP, &manifest_path),
        (PREVIOUS_ENVELOPE_BACKUP, &envelope_path),
    ] {
        let backup = workspace.join(name);
        if !backup.is_file() {
            return Err(MutationError::EvidenceRollback {
                detail: format!("the previous-evidence backup {name} is missing"),
            });
        }
        let measured = {
            let mut file = open_checked(&backup)?;
            hash_stream(&mut file)?
        };
        copy_verified_to(&backup, destination, measured, name)
            .map_err(|e| MutationError::EvidenceRollback {
                detail: e.to_string(),
            })?;
    }
    let manifest_bytes =
        read_bounded(&manifest_path, "restored committed manifest").map_err(|e| {
            MutationError::EvidenceRollback {
                detail: e.to_string(),
            }
        })?;
    let envelope_bytes =
        read_bounded(&envelope_path, "restored committed envelope").map_err(|e| {
            MutationError::EvidenceRollback {
                detail: e.to_string(),
            }
        })?;
    verify_committed_evidence(trust, &manifest_bytes, &envelope_bytes, &version, &digest)
        .map_err(|e| MutationError::EvidenceRollback {
            detail: e.to_string(),
        })?;
    Ok(())
}

// ------------------------------------------------------- probation launch

/// A launched probation child: the retained OS handle (for bounded waiting
/// and exact-child termination) plus its journaled identity facts.
pub struct LaunchedChild {
    /// `None` only in injected test launches that simulate outcomes without
    /// a real process.
    child: Option<std::process::Child>,
    pub identity: ProcessIdentity,
}

impl LaunchedChild {
    /// Whether the child process is still running. A retained handle asks
    /// the OS directly; an adopted child (no retained handle across a
    /// restart) is asked through its journaled identity, which detects PID
    /// reuse via the creation FILETIME.
    fn is_still_running(&mut self) -> Result<bool, MutationError> {
        match &mut self.child {
            Some(child) => child
                .try_wait()
                .map(|status| status.is_none())
                .map_err(|e| MutationError::Io {
                    detail: format!("probation child wait: {e}"),
                }),
            // No retained handle (adopted/journaled child across a restart):
            // liveness requires the FULL journaled identity — PID plus
            // creation FILETIME plus image. A PID reused by an unrelated
            // process reports not-alive; it is never resumed, never waited
            // on, and never terminated through that record.
            None => match query_live_process_identity(self.identity.pid) {
                Ok(live) => {
                    let journaled_created: u64 = self
                        .identity
                        .process_created_at
                        .parse()
                        .map_err(|_| MutationError::HealthAckWrongProcess {
                            detail: "the journaled creation time is not numeric".to_string(),
                        })?;
                    Ok(live.creation_filetime == journaled_created
                        && paths::equal(&live.image_path, &self.identity.image_path))
                }
                Err(handoff::HandoffError::CallerExited { .. }) => Ok(false),
                Err(other) => Err(MutationError::Io {
                    detail: format!("probation child liveness: {other}"),
                }),
            },
        }
    }
}

/// Create the exact probation child **suspended** (`CREATE_SUSPENDED`):
/// the kernel object, address space, environment block (carrying the frozen
/// session/nonce bindings) and identity exist, but not one instruction of
/// the child has run. This is the frozen-compatible launch handshake that
/// makes the admission invariant structural: the durable `probationProcess`
/// binding is journaled while the child is provably unable to execute its
/// admission, and the resume below is the release of a binding that is
/// already authoritative. The retained handle keeps termination exact.
fn spawn_probation_suspended(
    paths: &Paths,
    validated: &ValidatedHandoff,
    nonce: &str,
) -> Result<std::process::Child, MutationError> {
    use std::os::windows::process::CommandExt;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    let exe = paths.install().join(MAIN_EXE);
    assert_no_reparse(&exe)?;
    if !exe.is_file() {
        return Err(MutationError::InstalledFileMissing {
            resource: MAIN_EXE.to_string(),
        });
    }
    let mut command = std::process::Command::new(&exe);
    command.env(ENV_SESSION_ID, validated.session_id());
    command.env(ENV_NONCE, nonce);
    command.env(ENV_LAUNCH_MODE, LAUNCH_MODE_PROBATION);
    command.creation_flags(CREATE_SUSPENDED);
    command
        .spawn()
        .map_err(|e| MutationError::Launch {
            detail: format!("{}: {e}", exe.display()),
        })
}

/// Resume every thread of the exact child process. A suspended child owns
/// exactly one thread (it has never run, so it cannot have created more);
/// resuming is idempotent for an already-running process (a prior suspend
/// count of 0 is reported, not an error). Used both for the fresh launch
/// and for the crash-recovery adoption of an orphan suspended child.
fn resume_process_threads(pid: u32) -> Result<(), MutationError> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).map_err(|e| {
            MutationError::Launch {
                detail: format!("thread snapshot for {pid}: {e}"),
            }
        })?;
        let result = (|| -> Result<(), MutationError> {
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut next = Thread32First(snapshot, &mut entry);
            let mut resumed = 0u32;
            while next.is_ok() {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                        .map_err(|e| MutationError::Launch {
                            detail: format!(
                                "open thread {} of {pid}: {e}",
                                entry.th32ThreadID
                            ),
                        })?;
                    let previous = ResumeThread(thread);
                    let _ = CloseHandle(thread);
                    if previous == u32::MAX {
                        return Err(MutationError::Launch {
                            detail: format!(
                                "resume thread {} of {pid} failed",
                                entry.th32ThreadID
                            ),
                        });
                    }
                    resumed += 1;
                }
                next = Thread32Next(snapshot, &mut entry);
            }
            if resumed == 0 {
                return Err(MutationError::Launch {
                    detail: format!("no thread of {pid} could be resumed"),
                });
            }
            Ok(())
        })();
        let _ = CloseHandle(snapshot);
        result
    }
}

/// Terminate a suspended or running child through the retained handle. The
/// launch sequence never leaks a child: any failure after `spawn` (identity
/// capture, journal, resume) kills the exact process before propagating, so
/// no suspended orphan can ever hold the installed image locked.
fn kill_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// The production probation launch handshake (frozen-compatible strategy A):
///
/// 1. create the exact target child **suspended** (no instruction has run);
/// 2. capture its PID + creation FILETIME + canonical image;
/// 3. journal the durable `probationProcess` binding — the child is still
///    provably unable to execute its admission;
/// 4. resume the exact child. From the child's first instruction onward the
///    authoritative binding already exists.
///
/// Every step after the spawn kills the exact child on failure: the launch
/// sequence can never leak a suspended process nor leave it running without
/// a journaled binding.
fn launch_probation_child(
    paths: &Paths,
    validated: &ValidatedHandoff,
    nonce: &str,
) -> Result<LaunchedChild, MutationError> {
    let exe = paths.install().join(MAIN_EXE);
    let mut child = spawn_probation_suspended(paths, validated, nonce)?;
    let pid = child.id();
    let live = match query_live_process_identity(pid) {
        Ok(live) => live,
        Err(error) => {
            kill_child(&mut child);
            return Err(MutationError::Launch {
                detail: format!("launched child identity unrecordable: {error}"),
            });
        }
    };
    if !paths::equal(&live.image_path, &exe) {
        kill_child(&mut child);
        return Err(MutationError::Launch {
            detail: format!(
                "launched child image {:?} does not match the canonical path",
                live.image_path
            ),
        });
    }
    let identity = ProcessIdentity {
        pid: live.pid,
        process_created_at: live.creation_filetime.to_string(),
        image_path: exe,
    };
    // Durable BEFORE resume — the structural admission invariant.
    if let Err(error) = journal_probation_process(validated, &identity) {
        kill_child(&mut child);
        return Err(error);
    }
    if let Err(error) = resume_process_threads(pid) {
        kill_child(&mut child);
        return Err(error);
    }
    Ok(LaunchedChild {
        identity,
        child: Some(child),
    })
}

/// Journal the frozen `probationProcess` record immediately after process
/// creation — the `LaunchedAwaitingHealth` window is as small as the OS
/// allows.
fn journal_probation_process(
    validated: &ValidatedHandoff,
    probation: &ProcessIdentity,
) -> Result<(), MutationError> {
    publish_mutation(validated, |envelope| {
        envelope.probation_process = Some(probation.clone());
    })?;
    Ok(())
}

// ------------------------------------------------------- health validation

/// Validate a raw health marker against the durable session facts and the
/// live child. Every binding is checked; the nonce value itself is never
/// reproduced in an error.
fn validate_health_ack(
    _paths: &Paths,
    validated: &ValidatedHandoff,
    nonce: &str,
    bytes: &[u8],
) -> Result<HealthAckDocument, MutationError> {
    if bytes.len() > HEALTH_ACK_MAX_BYTES {
        return Err(MutationError::HealthAckMalformed {
            detail: "health marker exceeds its size bound".to_string(),
        });
    }
    let ack: HealthAckDocument = serde_json::from_slice(bytes).map_err(|e| {
        MutationError::HealthAckMalformed {
            detail: e.to_string(),
        }
    })?;
    if ack.schema_version != 1 {
        return Err(MutationError::HealthAckMalformed {
            detail: format!("unsupported health marker schema {}", ack.schema_version),
        });
    }
    let envelope = validated.envelope();
    if ack.session_id != validated.session_id()
        || ack.installation_id != envelope.installation_id
        || ack.version != envelope.to_version
    {
        return Err(MutationError::HealthAckWrongSession);
    }
    canonical_nonce(&ack.nonce)?;
    if ack.nonce != nonce {
        return Err(MutationError::HealthAckWrongNonce);
    }
    if chrono::DateTime::parse_from_rfc3339(&ack.initialized_at).is_err() {
        return Err(MutationError::HealthAckMalformed {
            detail: "initializedAt is not an RFC 3339 timestamp".to_string(),
        });
    }
    // The launch record is read from the CURRENT journal, never the
    // validation-time snapshot: the PostApply handoff is validated before
    // the nonce/launch exist, so the snapshot never carries it.
    let journaled = load_journal(validated)?
        .probation_process
        .ok_or_else(|| MutationError::HealthAckWrongProcess {
            detail: "no probation launch is journaled".to_string(),
        })?;
    if ack.pid != journaled.pid || ack.process_created_at != journaled.process_created_at {
        return Err(MutationError::HealthAckWrongProcess {
            detail: "marker process facts do not match the journaled probation child".to_string(),
        });
    }
    // The live process must still exist with exactly that identity: PID
    // reuse produces a different creation time, an unrelated process a
    // different image.
    let live = query_live_process_identity(ack.pid).map_err(|e| match e {
        handoff::HandoffError::CallerExited { pid } => MutationError::HealthAckWrongProcess {
            detail: format!("the probation child {pid} is no longer running"),
        },
        other => MutationError::HealthAckWrongProcess {
            detail: format!("the probation child is unverifiable: {other}"),
        },
    })?;
    let journaled_created: u64 = journaled
        .process_created_at
        .parse()
        .map_err(|_| MutationError::HealthAckWrongProcess {
            detail: "the journaled creation time is not numeric".to_string(),
        })?;
    if live.creation_filetime != journaled_created
        || !paths::equal(&live.image_path, &journaled.image_path)
    {
        return Err(MutationError::HealthAckWrongProcess {
            detail: "the live process does not match the journaled probation identity".to_string(),
        });
    }
    Ok(ack)
}

/// Accept a validated marker: journal the durable `acceptedHealth` (write
/// new generation → atomic publish → read-back) before any decision. The
/// nonce is consumed by this record: a later marker is a replay refusal.
fn accept_health(
    validated: &ValidatedHandoff,
    ack: &HealthAckDocument,
) -> Result<(), MutationError> {
    let envelope = load_journal(validated)?;
    if envelope.accepted_health.is_some() {
        return Err(MutationError::HealthAckReplay);
    }
    let accepted = AcceptedHealth {
        health_ack: ack.clone(),
        accepted_at: apply::now_utc(),
    };
    let value = serde_json::to_value(&accepted).map_err(|e| MutationError::JournalWrite {
        detail: e.to_string(),
    })?;
    publish_mutation(validated, |envelope| {
        envelope.accepted_health = Some(value.clone());
    })?;
    // Read-back binding: the durable record must parse back to exactly the
    // accepted fact.
    let durable = load_journal(validated)?;
    let recorded = parse_accepted_health(&durable, durable.accepted_health.as_ref().ok_or(
        MutationError::JournalWrite {
            detail: "acceptedHealth did not publish".to_string(),
        },
    )?)?;
    if recorded.health_ack != *ack {
        return Err(MutationError::JournalWrite {
            detail: "published acceptedHealth does not match the accepted marker".to_string(),
        });
    }
    Ok(())
}

/// Read the durable health marker, if published.
fn read_health_marker(validated: &ValidatedHandoff) -> Result<Option<Vec<u8>>, MutationError> {
    let path = validated.session_dir().join(HEALTH_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    assert_no_reparse(&path)?;
    let file = open_checked(&path)?;
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take((HEALTH_ACK_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| MutationError::Io {
            detail: format!("health marker read: {e}"),
        })?;
    Ok(Some(bytes))
}

// ------------------------------------------------------- the wait + verdict

/// Why a probation ended without an accepted marker. Every variant is a
/// frozen rollback trigger; nothing else rolls back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProbationFailure {
    LaunchFailed(MutationError),
    ChildExitedBeforeAck { pid: u32 },
    Timeout,
    InvalidHealth(MutationError),
}

impl ProbationFailure {
    fn reason(&self) -> String {
        match self {
            Self::LaunchFailed(error) => format!("probation launch failed: {error}"),
            Self::ChildExitedBeforeAck { pid } => {
                format!("the probation child {pid} exited before any HealthAck was accepted")
            }
            Self::Timeout => {
                "no accepted HealthAck arrived within the bounded probation window".to_string()
            }
            Self::InvalidHealth(error) => format!("invalid health state: {error}"),
        }
    }
}

/// Injectable probation controls (production: the real spawn and the
/// compiled timeout; deterministic tests: a probe child and tiny bounds).
pub(crate) struct ProbationControls<'a> {
    pub launch: &'a dyn Fn(&Paths, &ValidatedHandoff, &str) -> Result<LaunchedChild, MutationError>,
    /// Marker poll cadence and total attempt count.
    pub attempts: u32,
    pub interval: Duration,
    /// The probation window, measured monotonically from launch.
    pub timeout: Duration,
}

/// Wait — bounded, monotonic from the launch — for the exact child's
/// durable health marker, validating each observation. Returns the accepted
/// marker or the frozen rollback trigger.
fn wait_for_health(
    paths: &Paths,
    validated: &ValidatedHandoff,
    child: &mut LaunchedChild,
    nonce: &str,
    controls: &ProbationControls<'_>,
) -> Result<HealthAckDocument, ProbationFailure> {
    let started = Instant::now();
    for _ in 0..controls.attempts {
        if !child
            .is_still_running()
            .map_err(ProbationFailure::InvalidHealth)?
        {
            return Err(ProbationFailure::ChildExitedBeforeAck { pid: child.identity.pid });
        }
        if started.elapsed() >= controls.timeout {
            return Err(ProbationFailure::Timeout);
        }
        if let Some(bytes) = read_health_marker(validated)
            .map_err(ProbationFailure::InvalidHealth)?
        {
            let ack = validate_health_ack(paths, validated, nonce, &bytes)
                .map_err(ProbationFailure::InvalidHealth)?;
            return Ok(ack);
        }
        std::thread::sleep(controls.interval);
    }
    Err(ProbationFailure::Timeout)
}

// ------------------------------------------------------- commit

/// Map a journal identity to the compiled managed resource.
fn resource_for_identity(identity: &str) -> Result<Resource, MutationError> {
    let compiled =
        desktop_todo_update_core::InstallIdentity::from_manifest_text(identity).ok_or_else(
            || MutationError::RecoveryInconsistent {
                detail: format!("unknown identity {identity}"),
            },
        )?;
    Resource::from_filename(desktop_todo_update_core::identity_filename(compiled))
        .ok_or_else(|| MutationError::RecoveryInconsistent {
            detail: format!("identity {identity} is not a managed resource"),
        })
}

/// Fresh-open both installed managed executables and require the signed
/// target bytes — the commit-side verification (B in the frozen commit
/// ordering).
fn verify_installed_targets(
    paths: &Paths,
    validated: &ValidatedHandoff,
) -> Result<(), MutationError> {
    let envelope = load_journal(validated)?;
    for entry in &envelope.resources {
        let resource = resource_for_identity(&entry.identity)?;
        let destination = paths.install().join(resource.filename());
        let mut file = open_checked(&destination)?;
        let facts = hash_stream(&mut file)?;
        if facts != (entry.new_size, entry.new_sha256.clone()) {
            return Err(MutationError::ReplacementVerification {
                resource: resource.filename().to_string(),
                expected: entry.new_sha256.clone(),
                actual: facts.1,
            });
        }
    }
    Ok(())
}

/// Build the post-commit receipt view from the current (Updating) receipt
/// and the validated target facts. The caller publishes it only after the
/// registry and evidence steps verified.
fn commit_receipt_view(
    paths: &Paths,
    validated: &ValidatedHandoff,
) -> Result<Receipt, MutationError> {
    let mut receipt = Receipt::load(paths)
        .map_err(|e| MutationError::ReceiptCommit {
            detail: format!("receipt unusable: {e}"),
        })?
        .ok_or_else(|| MutationError::ReceiptCommit {
            detail: "no installation receipt".to_string(),
        })?;
    let envelope = validated.envelope();
    for record in &mut receipt.runtime_resources {
        let signed = envelope
            .resources
            .iter()
            .find(|entry| resource_for_identity(&entry.identity) == Ok(record.identity))
            .ok_or_else(|| MutationError::ReceiptCommit {
                detail: format!("no journal fact for {:?}", record.identity),
            })?;
        record.version = envelope.to_version.clone();
        record.size = signed.new_size;
        record.sha256 = signed.new_sha256.clone();
    }
    receipt.current_version = Some(envelope.to_version.clone());
    receipt.maintenance.committed_manifest_sha256 = Some(envelope.manifest_sha256.clone());
    Ok(receipt)
}

/// The bounded post-terminal cleanup. Failures are cleanup debt: recorded,
/// retried by a later maintenance invocation, and never a reason to degrade
/// the terminal verdict.
fn bounded_cleanup(paths: &Paths, validated: &ValidatedHandoff, clean_backups: bool) -> Vec<String> {
    let mut debts = Vec::new();
    let session_dir = validated.session_dir().to_path_buf();
    // Staged package + staged runtime + the durable health marker belong to
    // the finished transaction's ephemeral state; the session envelope and
    // the signed evidence stay as terminal records.
    for name in [crate::handoff::PACKAGE_FILE] {
        let path = session_dir.join(name);
        if path.exists() {
            if let Err(error) = paths::remove_file(&path) {
                debts.push(format!("{name}: {error}"));
            }
        }
    }
    let staged = session_dir.join(crate::handoff::STAGED_DIR);
    if staged.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&staged) {
            debts.push(format!("staged: {error}"));
        }
    }
    if clean_backups {
        let workspace = session_workspace(paths, validated.session_id());
        if workspace.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&workspace) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        if let Err(error) = paths::remove_file(&entry.path()) {
                            debts.push(format!(
                                "{}: {error}",
                                entry.file_name().to_string_lossy()
                            ));
                        }
                    }
                }
            }
            if let Err(error) = paths::remove_empty_dir(&workspace) {
                if !error.detail.contains("not empty") {
                    debts.push(format!("workspace: {error}"));
                }
            }
        }
    }
    remove_stale_runners(paths, &mut debts);
    debts
}

/// Remove stale session-runner copies (ephemeral state owned by
/// maintenance): only exact-shape UUID directories under the compiled
/// runners root whose image is not this running process. The current
/// runner's own image stays for a later invocation to remove, exactly as
/// the frozen deferred-cleanup policy prescribes.
fn remove_stale_runners(paths: &Paths, debts: &mut Vec<String>) {
    let runners = paths.state().join("runners");
    let Ok(entries) = std::fs::read_dir(&runners) else {
        return;
    };
    let current = std::env::current_exe().ok();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if validate_uuid(&name).is_err() {
            continue;
        }
        let image = entry.path().join(HELPER_EXE);
        let is_self = current
            .as_ref()
            .is_some_and(|current| paths::equal(current, &image));
        if is_self {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            debts.push(format!("runner {name}: {error}"));
        }
    }
}

/// Execute the commit transaction (frozen ordering). The caller has ensured
/// the durable `acceptedHealth`. No exclusive lease is needed: the committed
/// receipt closes admission by itself and no runtime file is touched.
pub(crate) fn execute_commit(
    paths: &Paths,
    validated: &ValidatedHandoff,
) -> Result<Vec<String>, MutationError> {
    let envelope = load_journal(validated)?;
    // 1. The durable decision: journal `commitIntent{commit}` first.
    journal_decision(
        validated,
        CommitDecision::Commit,
        &envelope,
        None,
    )?;
    // 2. Fresh-verify the installed target set.
    verify_installed_targets(paths, validated)?;
    // 3. Windows integration at the target version (readback-verified by
    //    the shared reconciliation). The receipt follows verified state, so
    //    this precedes the receipt publish.
    let target_view = commit_receipt_view(paths, validated)?;
    crate::integration::reconcile(paths, &target_view)
        .map_err(|e| MutationError::RegistryCommit { detail: e.to_string() })?;
    // 4. Publish the installed signed evidence — the exact persisted bytes.
    publish_installed_evidence(paths, validated)?;
    // 5. Receipt → Installed at the target: currentVersion, runtime facts,
    //    committed-evidence digest, lifecycle leaving Updating and
    //    activeSessionId cleared in one atomic publish.
    let mut committed = target_view;
    committed.lifecycle_state = Lifecycle::Installed;
    committed.maintenance.active_session_id = None;
    committed.maintenance.last_completed_session_id = Some(validated.session_id().to_string());
    committed
        .save(paths)
        .map_err(|e| MutationError::ReceiptCommit { detail: e.to_string() })?;
    // 6. Session finalized `committed` (journal published as evidence).
    publish_mutation(validated, |envelope| {
        envelope.phase = SessionPhase::Committed;
    })?;
    // 7. Bounded cleanup, debt-only failures.
    Ok(bounded_cleanup(paths, validated, true))
}

/// Publish the installed signed evidence from the session's persisted raw
/// bytes (idempotent atomic writes).
fn publish_installed_evidence(
    paths: &Paths,
    validated: &ValidatedHandoff,
) -> Result<(), MutationError> {
    let dir = validated.session_dir();
    let manifest_bytes = read_bounded(&dir.join(crate::handoff::MANIFEST_FILE), "persisted manifest")
        .map_err(|e| MutationError::EvidenceCommit { detail: e.to_string() })?;
    let envelope_bytes =
        read_bounded(&dir.join(crate::handoff::ENVELOPE_FILE), "persisted envelope")
            .map_err(|e| MutationError::EvidenceCommit { detail: e.to_string() })?;
    paths::atomic_write(
        &paths.install().join(crate::handoff::INSTALLED_MANIFEST_FILE),
        &manifest_bytes,
    )
    .map_err(|e| MutationError::EvidenceCommit { detail: e.to_string() })?;
    paths::atomic_write(
        &paths.install().join(crate::handoff::INSTALLED_ENVELOPE_FILE),
        &envelope_bytes,
    )
    .map_err(|e| MutationError::EvidenceCommit { detail: e.to_string() })?;
    Ok(())
}

/// Journal the typed decision boundary. Idempotent: an existing decision
/// must match the requested direction and the journal's frozen facts, or
/// the journal conflicts.
fn journal_decision(
    validated: &ValidatedHandoff,
    decision: CommitDecision,
    envelope: &UpdateSessionEnvelope,
    reason: Option<&str>,
) -> Result<CommitIntent, MutationError> {
    let intent = CommitIntent {
        schema_version: DECISION_SCHEMA_VERSION,
        decision,
        target_version: envelope.to_version.clone(),
        manifest_sha256: envelope.manifest_sha256.clone(),
        decided_at: apply::now_utc(),
        reason: reason
            .map(|r| r.chars().take(256).collect::<String>())
            .filter(|r| !r.is_empty()),
    };
    if let Some(existing_value) = &envelope.commit_intent {
        let existing = parse_commit_intent(existing_value).map_err(|detail| {
            MutationError::JournalConflict {
                field: format!("commitIntent is malformed: {detail}"),
            }
        })?;
        if existing.decision != intent.decision
            || existing.target_version != intent.target_version
            || existing.manifest_sha256 != intent.manifest_sha256
        {
            return Err(MutationError::JournalConflict {
                field: "commitIntent".to_string(),
            });
        }
        return Ok(existing);
    }
    let value =
        serde_json::to_value(&intent).map_err(|e| MutationError::JournalWrite {
            detail: e.to_string(),
        })?;
    publish_mutation(validated, |envelope| {
        envelope.commit_intent = Some(value.clone());
    })?;
    Ok(intent)
}

// ------------------------------------------------------- rollback

/// Terminate a journaled probation child after a restart (no retained
/// handle): re-derive the handle from the PID and verify the exact identity
/// — creation FILETIME and canonical image — before terminating. An already
/// exited child is success.
fn terminate_journaled_child(_paths: &Paths, identity: &ProcessIdentity) -> Result<(), MutationError> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_TERMINATE,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    let created: u64 = identity
        .process_created_at
        .parse()
        .map_err(|_| MutationError::HealthAckWrongProcess {
            detail: "the journaled creation time is not numeric".to_string(),
        })?;
    let live = match query_live_process_identity(identity.pid) {
        Ok(live) => live,
        Err(handoff::HandoffError::CallerExited { .. }) => return Ok(()),
        Err(other) => {
            return Err(MutationError::HealthAckWrongProcess {
                detail: format!("the journaled probation child is unverifiable: {other}"),
            })
        }
    };
    if live.creation_filetime != created || !paths::equal(&live.image_path, &identity.image_path) {
        return Err(MutationError::HealthAckWrongProcess {
            detail: "the live process does not match the journaled probation identity".to_string(),
        });
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            identity.pid,
        )
        .map_err(|e| MutationError::Io {
            detail: format!("open probation child {}: {e}", identity.pid),
        })?
    };
    let result = (|| {
        unsafe {
            TerminateProcess(handle, 1).map_err(|e| MutationError::Io {
                detail: format!("terminate probation child {}: {e}", identity.pid),
            })?;
            if WaitForSingleObject(handle, 10_000) != windows::Win32::Foundation::WAIT_OBJECT_0 {
                return Err(MutationError::Io {
                    detail: format!(
                        "the terminated probation child {} did not exit in time",
                        identity.pid
                    ),
                });
            }
        }
        Ok(())
    })();
    unsafe {
        let _ = CloseHandle(handle);
    }
    result
}

/// Execute the rollback transaction (frozen ordering). The caller has
/// ensured the exclusive application lease and (where a child might still
/// run) terminated the exact child first.
pub(crate) fn execute_rollback(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
    reason: &str,
) -> Result<Vec<String>, MutationError> {
    let _ = paths;
    let envelope = load_journal(validated)?;
    // Feasibility first: every rollback slot must hold the journaled
    // preimage before anything is touched. A missing or mismatching asset
    // means the old set is not provable — fail closed as inconsistent, never
    // best-effort.
    let mut plans = Vec::new();
    for entry in &envelope.resources {
        let resource = resource_for_identity(&entry.identity)?;
        let old = entry.old_sha256.as_ref().ok_or_else(|| {
            MutationError::RecoveryInconsistent {
                detail: format!("{} has no journaled preimage to restore", resource.filename()),
            }
        })?;
        let slot = backup_slot_path(paths, validated.session_id(), entry.identity.as_str());
        if !slot.is_file() {
            return Err(MutationError::RollbackAssetMissing {
                resource: resource.filename().to_string(),
            });
        }
        let plan = RollbackPlan {
            destination: paths.install().join(resource.filename()),
            slot,
            expected: (entry.old_size.unwrap_or(0), old.clone()),
            filename: resource.filename().to_string(),
        };
        verify_rollback_asset_by_plan(&plan)?;
        plans.push(plan);
    }
    // 1. The durable rollback decision (idempotent).
    journal_decision(validated, CommitDecision::Rollback, &envelope, Some(reason))?;
    // Evidence feasibility (previous backup must verify, or the absence
    // claim must hold) before the first side effect.
    restore_evidence_precheck(paths, trust, validated)?;
    // 2. Restore exactly the two managed executables from the verified
    //    slots, fresh-opening each result.
    for plan in &plans {
        let workspace = session_workspace(paths, validated.session_id())
            .join(format!("{}.rollback", plan.filename));
        assert_no_reparse(&plan.destination)?;
        copy_verified_to(&plan.slot, &workspace, plan.expected.clone(), &plan.filename).map_err(
            |e| MutationError::RollbackRestore {
                resource: plan.filename.clone(),
                detail: e.to_string(),
            },
        )?;
        paths::move_replace(&workspace, &plan.destination).map_err(|e| {
            MutationError::RollbackRestore {
                resource: plan.filename.clone(),
                detail: e.to_string(),
            }
        })?;
        let mut restored = open_checked(&plan.destination)?;
        let facts = hash_stream(&mut restored)?;
        if facts != plan.expected {
            return Err(MutationError::RollbackVerification {
                resource: plan.filename.clone(),
                expected: plan.expected.1.clone(),
                actual: facts.1,
            });
        }
    }
    // 3. Integration at the source version (the journaled previous
    //    receipt's desired state). The receipt follows verified state.
    let previous = envelope.previous_receipt.clone().ok_or_else(|| {
        MutationError::RecoveryInconsistent {
            detail: "the journaled previous receipt is missing".to_string(),
        }
    })?;
    crate::integration::reconcile(paths, &previous)
        .map_err(|e| MutationError::RegistryRollback { detail: e.to_string() })?;
    // 4. Restore the previous committed-evidence state.
    restore_evidence(paths, trust, validated)?;
    // 5. Receipt → Installed at the previous version, from the journaled
    //    snapshot — never reconstructed from memory or the target.
    let mut restored = previous;
    restored.lifecycle_state = Lifecycle::Installed;
    restored.maintenance.active_session_id = None;
    restored.maintenance.last_completed_session_id = Some(validated.session_id().to_string());
    restored
        .save(paths)
        .map_err(|e| MutationError::ReceiptCommit { detail: e.to_string() })?;
    // 6. Session finalized `rolled-back`.
    publish_mutation(validated, |envelope| {
        envelope.phase = SessionPhase::RolledBack;
    })?;
    // 7. Bounded cleanup: target/transient artifacts and stale runners; the
    //    verified OLD backup slots are retained as rollback evidence.
    Ok(bounded_cleanup(paths, validated, false))
}

struct RollbackPlan {
    destination: PathBuf,
    slot: PathBuf,
    expected: (u64, String),
    filename: String,
}

fn verify_rollback_asset_by_plan(plan: &RollbackPlan) -> Result<(), MutationError> {
    let mut slot = open_checked(&plan.slot)?;
    let facts = hash_stream(&mut slot)?;
    if facts != plan.expected {
        return Err(MutationError::RollbackAssetMismatch {
            resource: plan.filename.clone(),
            expected: plan.expected.1.clone(),
            actual: facts.1,
        });
    }
    Ok(())
}

/// Rollback feasibility precheck for the committed-evidence state: whatever
/// the previous receipt claims must be provable before any file is
/// restored.
fn restore_evidence_precheck(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
) -> Result<(), MutationError> {
    let envelope = load_journal(validated)?;
    let previous = envelope.previous_receipt.clone().ok_or_else(|| {
        MutationError::RecoveryInconsistent {
            detail: "the journaled previous receipt is missing".to_string(),
        }
    })?;
    let digest = previous
        .maintenance
        .committed_manifest_sha256
        .clone()
        .filter(|d| !d.is_empty());
    if digest.is_none() {
        return Ok(()); // absence will be re-asserted; removal failures fail later, before the receipt.
    }
    let workspace = session_workspace(paths, validated.session_id());
    let version = previous
        .current_version
        .clone()
        .ok_or_else(|| MutationError::RecoveryInconsistent {
            detail: "the previous receipt carries no version".to_string(),
        })?;
    // Self-healing resume: a rollback decision implies the evidence backup
    // ran before the launch (frozen order), but the resume path must not
    // depend on which crash window happened. If the backups are missing,
    // re-derive them from the still-source-version evidence at the install
    // root — full cryptographic verification applies either way, so a
    // foreign (target-version) evidence file still fails closed.
    for name in [PREVIOUS_MANIFEST_BACKUP, PREVIOUS_ENVELOPE_BACKUP] {
        if !workspace.join(name).is_file() {
            backup_previous_evidence(paths, trust, validated)?;
            break;
        }
    }
    for name in [PREVIOUS_MANIFEST_BACKUP, PREVIOUS_ENVELOPE_BACKUP] {
        let backup = workspace.join(name);
        if !backup.is_file() {
            return Err(MutationError::EvidenceRollback {
                detail: format!("the previous-evidence backup {name} is missing"),
            });
        }
    }
    let manifest_bytes = read_bounded(&workspace.join(PREVIOUS_MANIFEST_BACKUP), "previous evidence")
        .map_err(|e| MutationError::EvidenceRollback {
            detail: e.to_string(),
        })?;
    let envelope_bytes =
        read_bounded(&workspace.join(PREVIOUS_ENVELOPE_BACKUP), "previous evidence")
            .map_err(|e| MutationError::EvidenceRollback {
                detail: e.to_string(),
            })?;
    verify_committed_evidence(trust, &manifest_bytes, &envelope_bytes, &version, &digest.unwrap())
        .map_err(|e| MutationError::EvidenceRollback {
            detail: e.to_string(),
        })?;
    Ok(())
}

// ------------------------------------------------------- terminal marking

/// Mark the frozen `recovery-required` terminal: the journal records the
/// bounded error and the phase; the receipt leaves `Updating` for
/// `RecoveryRequired`. Normal launch stays blocked; a new session (or manual
/// repair) is required.
pub(crate) fn mark_recovery_required(
    paths: &Paths,
    validated: &ValidatedHandoff,
    detail: &str,
) -> Result<(), MutationError> {
    publish_mutation(validated, |envelope| {
        envelope.phase = SessionPhase::RecoveryRequired;
        envelope.last_error = Some(SessionErrorRecord {
            operation: "update".to_string(),
            resource: None,
            win32_code: None,
            message: detail.chars().take(512).collect(),
        });
    })?;
    let mut receipt = Receipt::load(paths)
        .map_err(|e| MutationError::ReceiptCommit {
            detail: format!("receipt unusable: {e}"),
        })?
        .ok_or_else(|| MutationError::ReceiptCommit {
            detail: "no installation receipt".to_string(),
        })?;
    if receipt.lifecycle_state == Lifecycle::Updating {
        receipt.lifecycle_state = Lifecycle::RecoveryRequired;
        receipt.maintenance.active_session_id = None;
        receipt
            .save(paths)
            .map_err(|e| MutationError::ReceiptCommit { detail: e.to_string() })?;
    }
    Ok(())
}

// ------------------------------------------------------- the probation flow

/// The terminal settlement of one update transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settlement {
    Committed,
    RolledBack { reason: String },
}

/// The Phase 2D-B continuation from `ReplacedAwaitingLaunch`: probation
/// ready (nonce + evidence backup, durable) → exclusive lease release →
/// launch → bounded health wait → commit or rollback → bounded cleanup.
/// `lease` is the exclusive lease held since the replacements; it is
/// dropped only after the probation-ready record is durable (frozen
/// choreography).
pub(crate) fn run_probation(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
    lease: AppLease,
    controls: &ProbationControls<'_>,
) -> Result<Settlement, MutationError> {
    // 1. Probation-ready: durable nonce + previous-evidence backup.
    let nonce = ensure_nonce(validated)?;
    backup_previous_evidence(paths, trust, validated)?;
    // 2. The exclusive lease is released only now (frozen step 1): the
    //    helper retains the gate and performs no runtime mutation until a
    //    terminal verdict.
    drop(lease);
    // 3. Launch (or adopt the suspended orphan from the crash window
    //    between the suspended spawn and the probation journal — see the
    //    adoption资格 argument on `adopt_or_launch`). The journal write is
    //    part of the launch handshake and happens before the child can run.
    match adopt_or_launch(paths, validated, &nonce, controls) {
        Ok(mut child) => {
            // 4. Bounded wait for the exact child's marker.
            match wait_for_health(paths, validated, &mut child, &nonce, controls) {
                Ok(ack) => {
                    accept_health(validated, &ack)?;
                    execute_commit(paths, validated)?;
                    Ok(Settlement::Committed)
                }
                Err(failure) => rollback_after_failure(paths, trust, validated, failure),
            }
        }
        Err(error) => rollback_after_failure(
            paths,
            trust,
            validated,
            ProbationFailure::LaunchFailed(error),
        ),
    }
}

/// Adopt the orphan suspended child from the crash window between the
/// suspended spawn and the journaled `probationProcess`, or launch a fresh
/// one. Under the suspended-creation handshake the window has exactly one
/// shape: **journaled ⟺ resumed** — the resume happens only after the
/// durable journal write, so a `ReplacedAwaitingLaunch` state with a live
/// main-image process can only be the helper-created orphan that has never
/// executed a single instruction (ordinary launches are refused by the
/// `Updating` receipt, and a Development copy has a different image). The
/// adoption completes the handshake: capture its identity, journal it
/// durably, then resume — after which the orphan runs its full admission
/// against the now-authoritative binding like any freshly launched child.
/// Its ack must still carry the durable nonce, so adoption widens nothing.
fn adopt_or_launch(
    paths: &Paths,
    validated: &ValidatedHandoff,
    nonce: &str,
    controls: &ProbationControls<'_>,
) -> Result<LaunchedChild, MutationError> {
    if let Some(identity) = find_live_installed_main(paths) {
        // Journal first (the child is still suspended and cannot run its
        // admission), then resume. Any failure terminates the exact orphan
        // through its identity — never a leaked process, never a suspended
        // image lock left behind for the rollback. (`nonce` belongs to the
        // fresh-launch branch below; the adopted orphan carries its own
        // copy in the environment block the helper gave it at creation.)
        if let Err(error) = journal_probation_process(validated, &identity) {
            let _ = terminate_journaled_child(paths, &identity);
            return Err(error);
        }
        if let Err(error) = resume_process_threads(identity.pid) {
            let _ = terminate_journaled_child(paths, &identity);
            return Err(error);
        }
        return Ok(LaunchedChild {
            child: None,
            identity,
        });
    }
    (controls.launch)(paths, validated, nonce)
}

/// Find a live process whose image is exactly the canonical installed main
/// executable. Image-path matched only — never a name-only kill surface.
fn find_live_installed_main(paths: &Paths) -> Option<ProcessIdentity> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let expected = paths.install().join(MAIN_EXE);
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let result = (|| {
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut next = Process32FirstW(snapshot, &mut entry);
            while next.is_ok() {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                if name.eq_ignore_ascii_case(MAIN_EXE) {
                    if let Ok(handle) =
                        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID)
                    {
                        let mut image = vec![0u16; 32768];
                        let mut len = image.len() as u32;
                        let queried = QueryFullProcessImageNameW(
                            handle,
                            PROCESS_NAME_WIN32,
                            PWSTR(image.as_mut_ptr()),
                            &mut len,
                        );
                        let _ = CloseHandle(handle);
                        if queried.is_ok() {
                            image.truncate(len as usize);
                            let image_path =
                                PathBuf::from(String::from_utf16_lossy(&image));
                            if paths::equal(&image_path, &expected) {
                                if let Ok(live) =
                                    query_live_process_identity(entry.th32ProcessID)
                                {
                                    return Some(ProcessIdentity {
                                        pid: live.pid,
                                        process_created_at: live.creation_filetime.to_string(),
                                        image_path: expected,
                                    });
                                }
                            }
                        }
                    }
                }
                next = Process32NextW(snapshot, &mut entry);
            }
            None
        })();
        let _ = CloseHandle(snapshot);
        result
    }
}

/// The frozen failure path (lifecycle §10 step 5): terminate the exact
/// probation child, verify it is gone and the shared lease is released,
/// reacquire the exclusive lease, and roll back. The helper never requests
/// the exclusive lease while the child may still hold it. Termination goes
/// through the journaled `probationProcess` identity — which exists for
/// every path that reached the wait — so retained-handle and adopted-child
/// cases behave identically, and an already-exited child is success.
fn rollback_after_failure(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
    failure: ProbationFailure,
) -> Result<Settlement, MutationError> {
    let reason = failure.reason();
    let envelope = load_journal(validated)?;
    if let Some(identity) = &envelope.probation_process {
        terminate_journaled_child(paths, identity)?;
    }
    // The child is gone (or never started): its shared lease is released or
    // releasing. Bounded wait, then the exclusive lease for the rollback.
    let _lease = wait_exclusive_lease(paths, ROLLBACK_LEASE_ATTEMPTS, PROBATION_INTERVAL)?;
    execute_rollback(paths, trust, validated, &reason)?;
    Ok(Settlement::RolledBack { reason })
}

/// The production continuation from a completed replacement phase
/// (`ReplacedAwaitingLaunch`, exclusive lease held): run the probation flow
/// with production controls.
pub(crate) fn settle_after_apply(
    paths: &Paths,
    trust: &TrustStore,
    validated: ValidatedHandoff,
    lease: AppLease,
) -> Result<Settlement, MutationError> {
    let controls = ProbationControls {
        launch: &launch_probation_child,
        attempts: PROBATION_ATTEMPTS,
        interval: PROBATION_INTERVAL,
        timeout: PROBATION_TIMEOUT,
    };
    run_probation(paths, trust, &validated, lease, &controls)
}

// ------------------------------------------------------- recovery execution

/// The executed `--recover` entry: classify read-only, then perform exactly
/// the frozen-safe action the classification names. Ambiguous evidence is
/// marked `recovery-required`, never guessed through. Returns the human
/// report text.
pub fn recover_and_execute(paths: &Paths, session_id: &str) -> Result<String, Error> {
    let trust = desktop_todo_update_core::production_trust_store();
    recover_and_execute_with(paths, &trust, session_id)
}

/// [`recover_and_execute`] with an explicit trust store — the seam
/// deterministic tests use (the production compiled store fails closed on
/// test-fixture keys by design).
pub fn recover_and_execute_with(
    paths: &Paths,
    trust: &TrustStore,
    session_id: &str,
) -> Result<String, Error> {
    validate_uuid(session_id).map_err(|e| {
        Error::new(ErrorKind::InvalidInstallation, format!("--session-id: {e}"))
    })?;
    let _gate = Gate::acquire(paths).map_err(Error::from)?;
    let classification =
        apply::classify_update_recovery(paths, trust, session_id).map_err(Error::from)?;
    let state = classification.state.clone();
    let base = classification.report_text();
    match state {
        MutationRecoveryState::NoActiveUpdate => Ok(format!(
            "{base}\nNo active update transaction exists; nothing to recover."
        )),
        MutationRecoveryState::HandoffPrepared => Ok(format!(
            "{base}\nAuthority was never transferred (the receipt is still Installed); \
             normal launch is unaffected. The session may be resumed by its owner or \
             discarded by bounded cleanup."
        )),
        MutationRecoveryState::RecoveryRequired => Ok(format!(
            "{base}\nThis session is in the recovery-required terminal state; \
             a new session or manual repair is required."
        )),
        MutationRecoveryState::CommittedCleanupPending => {
            // The terminal truth is durable; only bounded cleanup may
            // remain. No PostApply validation is needed (the receipt is no
            // longer Updating).
            let debts = cleanup_terminal_session(paths, session_id, true);
            Ok(format!("{base}\nCleanup completed. Debt: {debts:?}"))
        }
        MutationRecoveryState::RolledBackCleanupPending => {
            let debts = cleanup_terminal_session(paths, session_id, false);
            Ok(format!("{base}\nCleanup completed. Debt: {debts:?}"))
        }
        MutationRecoveryState::UpdatingNoMutation
        | MutationRecoveryState::BackupPartial
        | MutationRecoveryState::ReplacePartial
        | MutationRecoveryState::ReplacedAwaitingLaunch => {
            // Resume the apply (idempotent) and continue into probation.
            let validated = validated_for_recovery(paths, trust, session_id)?;
            let lease = wait_exclusive_lease(paths, crate::apply::LEASE_ATTEMPTS, crate::apply::RETRY_INTERVAL)
                .map_err(Error::from)?;
            crate::lifecycle::require_no_legacy_processes(paths).map_err(|e| {
                Error::new(ErrorKind::MainProcessStillRunning, e.to_string())
            })?;
            resume_apply(paths, &validated)?;
            let controls = ProbationControls {
                launch: &launch_probation_child,
                attempts: PROBATION_ATTEMPTS,
                interval: PROBATION_INTERVAL,
                timeout: PROBATION_TIMEOUT,
            };
            match run_probation(paths, trust, &validated, lease, &controls)? {
                Settlement::Committed => Ok(format!("{base}\nUpdate committed.")),
                Settlement::RolledBack { reason } => {
                    Ok(format!("{base}\nUpdate rolled back: {reason}"))
                }
            }
        }
        MutationRecoveryState::LaunchedAwaitingHealth => {
            let validated = validated_for_recovery(paths, trust, session_id)?;
            let lease = wait_exclusive_lease(paths, crate::apply::LEASE_ATTEMPTS, crate::apply::RETRY_INTERVAL)
                .map_err(Error::from)?;
            let controls = ProbationControls {
                launch: &launch_probation_child,
                attempts: PROBATION_ATTEMPTS,
                interval: PROBATION_INTERVAL,
                timeout: PROBATION_TIMEOUT,
            };
            resume_probation_observation(paths, trust, &validated, lease, &controls)
                .map(|settlement| match settlement {
                    Settlement::Committed => format!("{base}\nUpdate committed."),
                    Settlement::RolledBack { reason } => {
                        format!("{base}\nUpdate rolled back: {reason}")
                    }
                })
        }
        MutationRecoveryState::CommitPending => {
            let validated = validated_for_recovery(paths, trust, session_id)?;
            let debts = execute_commit(paths, &validated).map_err(Error::from)?;
            Ok(format!("{base}\nUpdate committed. Debt: {debts:?}"))
        }
        MutationRecoveryState::RollbackPending => {
            let validated = validated_for_recovery(paths, trust, session_id)?;
            // A journaled child may still be alive and holding the shared
            // lease; terminate it by its journaled identity first.
            let envelope = load_journal(&validated)?;
            if let Some(identity) = &envelope.probation_process {
                terminate_journaled_child(paths, identity)?;
            }
            let _lease = wait_exclusive_lease(paths, ROLLBACK_LEASE_ATTEMPTS, PROBATION_INTERVAL)
                .map_err(Error::from)?;
            let debts = execute_rollback(
                paths,
                &trust,
                &validated,
                "recovery resumed a journaled rollback",
            )
            .map_err(Error::from)?;
            Ok(format!(
                "{base}\nUpdate rolled back: resumed the journaled rollback decision. Debt: {debts:?}"
            ))
        }
        MutationRecoveryState::CommittedPendingFinalization => {
            let debts = finalize_committed(paths, trust, session_id).map_err(Error::from)?;
            Ok(format!(
                "{base}\nCommitted transaction finalized. Debt: {debts:?}"
            ))
        }
        MutationRecoveryState::RolledBackPendingFinalization => {
            let debts = finalize_rolled_back(paths, trust, session_id).map_err(Error::from)?;
            Ok(format!(
                "{base}\nRolled-back transaction finalized. Debt: {debts:?}"
            ))
        }
        MutationRecoveryState::Inconsistent { detail } => {
            // Fail closed: mark the frozen recovery-required terminal so no
            // later launch guesses.
            if let Ok(validated) = validated_for_recovery(paths, trust, session_id) {
                let _ = mark_recovery_required(paths, &validated, &detail);
                return Err(Error::new(
                    ErrorKind::InvalidInstallation,
                    format!(
                        "{base}\nThe installation requires recovery: {detail}\n\
                         The session was marked recovery-required; a new update session \
                         (or manual repair) is required."
                    ),
                ));
            }
            Err(Error::new(
                ErrorKind::InvalidInstallation,
                format!(
                    "{base}\nThe installation requires recovery: {detail}\n\
                     Run the maintenance helper manually or reinstall from the trusted package."
                ),
            ))
        }
    }
}

/// PostApply validation for the recovery executor: the verified digest comes
/// from the helper's own journaled record (written before any destructive
/// work), never from a CLI argument.
fn validated_for_recovery(
    paths: &Paths,
    trust: &TrustStore,
    session_id: &str,
) -> Result<ValidatedHandoff, Error> {
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    let journal_bytes = read_bounded(&dir.join(crate::handoff::HELPER_JOURNAL_FILE), "helper journal")
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Digest {
        verified_manifest_sha256: String,
    }
    let digest: Digest = serde_json::from_slice(&journal_bytes).map_err(|e| {
        Error::new(
            ErrorKind::UpdateHandoffRejected,
            format!("helper journal malformed: {e}"),
        )
    })?;
    validate_post_apply(paths, trust, session_id, &digest.verified_manifest_sha256)
        .map_err(Error::from)
}

/// Resume an interrupted replacement phase: idempotent per-resource apply
/// over the re-validated handoff (the exclusive lease is already held by
/// the caller).
fn resume_apply(paths: &Paths, validated: &ValidatedHandoff) -> Result<(), Error> {
    apply::apply_replacements(paths, validated).map_err(Error::from)
}

/// Resume probation from the `LaunchedAwaitingHealth` state (the helper
/// died while waiting): a durable `acceptedHealth` goes straight to commit;
/// a fully validating marker whose child is still alive with the journaled
/// identity is accepted and committed; anything else terminates the
/// journaled child and rolls back. This is the frozen lattice's
/// "revalidate ... fresh controlled probation if safe; otherwise rollback"
/// resolved to the always-safe branch: no second probation attempt is
/// started from a resume.
/// Resume probation from the `LaunchedAwaitingHealth` state (the helper
/// died while the probation was unresolved): the frozen lattice resolves
/// this as "revalidate ... perform fresh controlled probation if safe;
/// otherwise rollback". A durable `acceptedHealth` goes straight to commit;
/// a still-live journaled child is **observed** — the helper ensures it is
/// resumed (covering the crash window between the journal and the resume,
/// where the child is a suspended orphan) and waits a fresh bounded window
/// for its marker; this is the continuation of the same probation attempt,
/// never a second launch, so the nonce is not rotated and no new child is
/// created. A dead child, an invalid marker, or a timed-out observation
/// terminates the exact journaled child and rolls back.
fn resume_probation_observation(
    paths: &Paths,
    trust: &TrustStore,
    validated: &ValidatedHandoff,
    lease: AppLease,
    controls: &ProbationControls<'_>,
) -> Result<Settlement, Error> {
    let envelope = load_journal(validated).map_err(Error::from)?;
    if envelope.accepted_health.is_some() {
        drop(lease);
        return execute_commit(paths, validated)
            .map(|_| Settlement::Committed)
            .map_err(Error::from);
    }
    let nonce = envelope
        .health_nonce
        .clone()
        .ok_or_else(|| Error::new(ErrorKind::UpdateApplyFailed, "no journaled nonce"))?;
    let journaled = envelope
        .probation_process
        .clone()
        .ok_or_else(|| Error::new(ErrorKind::UpdateApplyFailed, "no journaled probation child"))?;
    // A marker that is already durable and fully validating (including the
    // live-process check) is the frozen ACK path, observed late.
    let mut child = LaunchedChild {
        child: None,
        identity: journaled.clone(),
    };
    let accepted = match read_health_marker(validated).map_err(Error::from)? {
        Some(bytes) => match validate_health_ack(paths, validated, &nonce, &bytes) {
            Ok(ack) => {
                accept_health(validated, &ack).map_err(Error::from)?;
                true
            }
            Err(_) => false,
        },
        None => false,
    };
    let alive = child.is_still_running().map_err(Error::from)?;
    if accepted {
        // The frozen ACK path, observed late: health is already durably
        // accepted — commit directly (no exclusive lease needed).
        drop(lease);
        execute_commit(paths, validated).map_err(Error::from)?;
        return Ok(Settlement::Committed);
    }
    if alive {
        // The journaled child is alive: continue its probation. Under the
        // suspended-creation handshake the journal may predate the resume
        // (helper crashed in between), so ensure the child is running
        // before waiting — idempotent for an already-running child.
        drop(lease);
        resume_process_threads(journaled.pid).map_err(Error::from)?;
        return match wait_for_health(paths, validated, &mut child, &nonce, controls) {
            Ok(ack) => {
                accept_health(validated, &ack).map_err(Error::from)?;
                execute_commit(paths, validated).map_err(Error::from)?;
                Ok(Settlement::Committed)
            }
            Err(failure) => {
                let _lease = wait_exclusive_lease(
                    paths,
                    ROLLBACK_LEASE_ATTEMPTS,
                    PROBATION_INTERVAL,
                )
                .map_err(Error::from)?;
                execute_rollback(paths, trust, validated, &failure.reason())
                    .map(|_| Settlement::RolledBack { reason: failure.reason() })
                    .map_err(Error::from)
            }
        };
    }
    // Unresolved probation (dead child or invalid marker): terminate the
    // exact journaled child, wait for the lease, roll back.
    drop(lease);
    if terminate_journaled_child(paths, &journaled).is_ok() {
        // fallthrough to the bounded lease wait below
    }
    let _lease = wait_exclusive_lease(paths, ROLLBACK_LEASE_ATTEMPTS, PROBATION_INTERVAL)
        .map_err(Error::from)?;
    execute_rollback(
        paths,
        trust,
        validated,
        "the probation could not be proven after the helper was interrupted",
    )
    .map(|_| Settlement::RolledBack {
        reason: "the probation could not be proven after the helper was interrupted".to_string(),
    })
    .map_err(Error::from)
}

/// Finalize the `CommittedPendingFinalization` lattice state: the receipt is
/// authoritative (the installation is committed); verify the installed set
/// and integration, finalize the session `committed`, clean up.
fn finalize_committed(paths: &Paths, trust: &TrustStore, session_id: &str) -> Result<Vec<String>, Error> {
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    let manifest_bytes = read_bounded(&dir.join(crate::handoff::MANIFEST_FILE), "persisted manifest")
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    let envelope_bytes =
        read_bounded(&dir.join(crate::handoff::ENVELOPE_FILE), "persisted envelope")
            .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    let target = desktop_todo_update_core::verify_and_parse(trust, &envelope_bytes, &manifest_bytes)
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    // Installed set + integration verification/ensure at the target.
    let mut receipt = Receipt::load(paths)
        .map_err(Error::from)?
        .ok_or_else(|| Error::new(ErrorKind::InvalidInstallation, "no installation receipt"))?;
    for record in &mut receipt.runtime_resources {
        let signed = target
            .manifest()
            .asset()
            .install_files()
            .iter()
            .find(|file| {
                Resource::from_filename(desktop_todo_update_core::identity_filename(
                    desktop_todo_update_core::InstallIdentity::from_manifest_text(file.identity())
                        .unwrap_or(desktop_todo_update_core::InstallIdentity::MainExecutable),
                )) == Some(record.identity)
            })
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidInstallation,
                    format!("no signed fact for {:?}", record.identity),
                )
            })?;
        let path = paths.install().join(record.identity.filename());
        let mut file = open_checked(&path).map_err(Error::from)?;
        let facts = hash_stream(&mut file).map_err(Error::from)?;
        if facts != (signed.size(), signed.sha256_hex().to_string()) {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                format!(
                    "installed {} does not hold the signed target bytes",
                    record.identity.filename()
                ),
            ));
        }
        record.version = target.manifest().version().to_string();
        record.size = signed.size();
        record.sha256 = signed.sha256_hex().to_string();
    }
    receipt.current_version = Some(target.manifest().version().to_string());
    receipt.maintenance.committed_manifest_sha256 =
        Some(desktop_todo_update_core::sha256_hex(&manifest_bytes));
    crate::integration::reconcile(paths, &receipt).map_err(Error::from)?;
    // Journal finalization: phase → committed (with the digest re-verified
    // above, never merged speculatively).
    let mut envelope = load_update_session(&dir)
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    envelope.phase = SessionPhase::Committed;
    envelope.generation = envelope.generation.checked_add(1).ok_or_else(|| {
        Error::new(ErrorKind::UpdateApplyFailed, "journal generation overflow")
    })?;
    envelope.updated_at = apply::now_utc();
    let bytes = serde_json::to_vec_pretty(&envelope)
        .map_err(|e| Error::new(ErrorKind::UpdateApplyFailed, e.to_string()))?;
    paths::atomic_write(&dir.join(SESSION_FILE), &bytes).map_err(Error::from)?;
    let published = load_update_session(&dir)
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    if published.phase != SessionPhase::Committed {
        return Err(Error::new(
            ErrorKind::UpdateApplyFailed,
            "finalized session did not read back as committed",
        ));
    }
    // Bounded cleanup without a ValidatedHandoff: the terminal facts name
    // the session directory directly.
    Ok(cleanup_terminal_session(paths, session_id, true))
}

/// Finalize the `RolledBackPendingFinalization` lattice state: verify the
/// restored old set, finalize the session `rolled-back`, clean up.
fn finalize_rolled_back(paths: &Paths, trust: &TrustStore, session_id: &str) -> Result<Vec<String>, Error> {
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    let mut envelope = load_update_session(&dir)
        .map_err(|e| Error::new(ErrorKind::UpdateHandoffRejected, e.to_string()))?;
    let previous = envelope
        .previous_receipt
        .clone()
        .ok_or_else(|| Error::new(ErrorKind::InvalidInstallation, "no journaled previous receipt"))?;
    // Verify the restored old set against the previous receipt's facts.
    for record in &previous.runtime_resources {
        let path = paths.install().join(record.identity.filename());
        let mut file = open_checked(&path).map_err(Error::from)?;
        let facts = hash_stream(&mut file).map_err(Error::from)?;
        if facts != (record.size, record.sha256.clone()) {
            return Err(Error::new(
                ErrorKind::InvalidInstallation,
                format!(
                    "restored {} does not hold the journaled previous bytes",
                    record.identity.filename()
                ),
            ));
        }
    }
    let _ = trust;
    envelope.phase = SessionPhase::RolledBack;
    envelope.generation = envelope.generation.checked_add(1).ok_or_else(|| {
        Error::new(ErrorKind::UpdateApplyFailed, "journal generation overflow")
    })?;
    envelope.updated_at = apply::now_utc();
    let bytes = serde_json::to_vec_pretty(&envelope)
        .map_err(|e| Error::new(ErrorKind::UpdateApplyFailed, e.to_string()))?;
    paths::atomic_write(&dir.join(SESSION_FILE), &bytes).map_err(Error::from)?;
    Ok(cleanup_terminal_session(paths, session_id, false))
}

/// Terminal cleanup for the finalize paths (no `ValidatedHandoff`): the
/// session directory is resolved from the canonical sessions root plus the
/// validated session id.
fn cleanup_terminal_session(paths: &Paths, session_id: &str, clean_backups: bool) -> Vec<String> {
    let mut debts = Vec::new();
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    for name in [crate::handoff::PACKAGE_FILE] {
        let path = dir.join(name);
        if path.exists() {
            if let Err(error) = paths::remove_file(&path) {
                debts.push(format!("{name}: {error}"));
            }
        }
    }
    let staged = dir.join(crate::handoff::STAGED_DIR);
    if staged.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&staged) {
            debts.push(format!("staged: {error}"));
        }
    }
    if clean_backups {
        let workspace = session_workspace(paths, session_id);
        if workspace.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&workspace) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        if let Err(error) = paths::remove_file(&entry.path()) {
                            debts.push(format!(
                                "{}: {error}",
                                entry.file_name().to_string_lossy()
                            ));
                        }
                    }
                }
            }
            let _ = paths::remove_empty_dir(&workspace);
        }
    }
    remove_stale_runners(paths, &mut debts);
    debts
}

// ------------------------------------------------------- child-side API

/// Build this process's HealthAck document (the probation child side): own
/// PID, own creation FILETIME, the validated session bindings.
pub fn build_current_health_ack(
    session_id: &str,
    installation_id: &str,
    target_version: &str,
    nonce: &str,
) -> Result<HealthAckDocument, Error> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let created = unsafe {
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
        ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64
    };
    Ok(HealthAckDocument {
        schema_version: 1,
        session_id: session_id.to_string(),
        installation_id: installation_id.to_string(),
        version: target_version.to_string(),
        nonce: nonce.to_string(),
        pid: std::process::id(),
        process_created_at: created.to_string(),
        initialized_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

/// The child writes the durable health marker: bounded `health.json.tmp`,
/// full flush, atomic rename to `health.json` inside the session directory
/// (compiled-derived from the canonical roots plus the session id — the
/// environment values authorize nothing).
pub fn write_health_ack(paths: &Paths, ack: &HealthAckDocument) -> Result<(), Error> {
    let bytes = serde_json::to_vec(ack)
        .map_err(|e| Error::new(ErrorKind::Io, e.to_string()))?;
    if bytes.len() > HEALTH_ACK_MAX_BYTES {
        return Err(Error::new(ErrorKind::Io, "health marker exceeds its bound"));
    }
    let path = paths
        .state()
        .join("updates")
        .join("sessions")
        .join(&ack.session_id)
        .join(HEALTH_FILE);
    paths::atomic_write(&path, &bytes)
}

#[cfg(test)]
mod tests;
