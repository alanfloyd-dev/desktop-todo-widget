//! The Phase 2D runtime mutation transaction: from a [`ValidatedHandoff`]
//! through the lease, the durable `Updating` transition, the backup/replace
//! transaction over exactly the two managed executables, and the launch of
//! the new runtime — stopping exactly before HealthAck acceptance.
//!
//! Frozen boundaries this module implements (protocol v1 and application-
//! lifecycle §8–§10; nothing here invents protocol fields):
//!
//! - **Mutation authority.** Every entry takes a [`ValidatedHandoff`] — the
//!   unforgeable output of the handoff validation — or re-derives one. Raw
//!   session JSON, session paths, booleans and provider metadata are never
//!   mutation inputs.
//! - **Journal leads, receipt follows.** The durable journal is the frozen
//!   `UpdateSession` envelope: per-resource `intent`, old facts
//!   (`oldPresent`/`oldSha256`/`oldSize`) and `completed` advance field by
//!   field, `generation` is monotonic, every write is write-new-generation →
//!   atomic publish → read-back. The receipt only follows verified state:
//!   handoff = (1) session `handed-off` + previous-state snapshots, then
//!   (2) receipt → `Updating` + `activeSessionId`; the receipt stays
//!   `Updating` throughout the apply.
//! - **Single writer.** The Phase 1 lease primitives are reused as-is: the
//!   maintenance [`crate::lock::Gate`] closes admission, the exclusive
//!   application lease excludes live app instances (released only after the
//!   replacements are durable, before the probation launch). No second lock
//!   mechanism exists.
//! - **Helper self-replacement (frozen strategy).** The installed helper
//!   never replaces its own image while executing from it: it copies itself
//!   to a private session runner under the compiled runners root, verifies
//!   the bytes, spawns the runner with the same frozen argv and transfers
//!   ownership through an explicit ready handshake (the runner takes the
//!   gate, then signals). The runner — still the old protocol
//!   implementation — executes the transaction, which frees the installed
//!   helper image for an atomic same-volume move; at every instant the
//!   installed helper is exactly the old or the new verified bytes.
//! - **Backup before replace.** Per resource: durable intent → backup the
//!   old runtime into the fixed frozen slot
//!   (`.maintenance/<UUID>/<identity>.backup`) → verify the backup fact →
//!   journal the old facts → only then replace from the staged bytes and
//!   fresh-verify the destination against the signed `installFiles` facts.
//! - **The runtime mutation set is exactly the two managed executables.**
//!   Support files, the receipt's version facts, the registry and user data
//!   are never touched by this phase; `currentVersion` stays the last
//!   committed version until commit.
//!
//! This phase stops before HealthAck acceptance: no nonce is minted, no
//! probation decision exists, nothing is committed, rolled back or cleaned
//! up. The read-only [`classify_update_recovery`] output gives the next
//! phase the exact journal facts it needs.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use desktop_todo_update_core::{
    identity_filename, sha256_hex, verify_and_parse, InstallIdentity, TrustStore, Version,
};
use serde::Serialize;

use crate::handoff::{
    self, query_live_process_identity, validate_caller_identity, HandoffError, ValidatedHandoff,
};
use crate::lock::{self, Gate};
use crate::paths::{self, Paths};
use crate::receipt::{validate_uuid, Lifecycle, Receipt};
use crate::resources::Resource;
use crate::update_session::{
    load_update_session, ResourceAction, SessionPhase, UpdateSessionEnvelope,
    SESSION_FILE,
};
use crate::{Error, ErrorKind, HELPER_EXE};

/// The frozen per-session backup/workspace directory under the install root
/// (the grammar of `update_session::backup_slot`): same-volume copies and
/// backups live here, never arbitrary paths.
pub const SESSION_WORKSPACE_DIR: &str = r".maintenance";

/// Production wait bounds (implementation policy, not protocol invariants):
/// the runner-ready handshake and the app-exit lease wait.
pub(crate) const RUNNER_READY_ATTEMPTS: u32 = 40; // 40 × 250 ms = 10 s
pub(crate) const LEASE_ATTEMPTS: u32 = 120; // 120 × 250 ms = 30 s
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Why the runtime mutation transaction refused or failed. A closed taxonomy
/// of its own — never collapsed into a generic failure and never reusing a
/// lower-layer error kind unless the failure really originates there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationError {
    /// The transaction was not entered from the installed helper image (the
    /// frozen runner strategy is the only supported self-replacement path;
    /// executing a mutation from any other image is refused).
    HelperImageMismatch { path: PathBuf },
    /// The session runner could not be staged or spawned.
    HelperSpawn { detail: String },
    /// The runner never signaled the ownership handshake.
    RunnerNotReady,
    /// The recorded caller process no longer exists.
    CallerExited { pid: u32 },
    /// The live caller does not match the recorded identity.
    CallerIdentityMismatch { detail: String },
    /// Another live owner holds the exclusive application lease.
    LeaseBusy { detail: String },
    /// The receipt could not make the frozen `Updating` transition.
    LifecycleTransition { detail: String },
    /// The frozen source baseline no longer matches the actual installation.
    SourceVersionChanged { recorded: String, current: String },
    /// The local installation cannot support the transaction.
    InstallationInvalid { detail: String },
    /// A managed runtime file expected at the install root is absent.
    InstalledFileMissing { resource: String },
    /// The installed bytes disagree with the receipt's recorded preimage —
    /// a repair conflict, never permission to overwrite unknown content.
    InstalledHashMismatch { resource: String, expected: String, actual: String },
    /// The backup copy could not be created.
    BackupFailed { resource: String, detail: String },
    /// The backup copy does not verify against the measured preimage.
    BackupVerification { resource: String, expected: String, actual: String },
    /// The replacement could not be performed.
    ReplacementFailed { resource: String, detail: String },
    /// The replaced destination does not verify against the signed facts.
    ReplacementVerification { resource: String, expected: String, actual: String },
    /// A path passes through (or is) a reparse point / hard link.
    UnsafePath { path: PathBuf },
    /// A durable journal field disagrees with the validated facts.
    JournalConflict { field: String },
    /// A durable journal write failed (including the intent and completion
    /// writes for backup and replacement).
    JournalWrite { detail: String },
    /// The new runtime could not be launched or its identity recorded.
    Launch { detail: String },
    /// The session health nonce could not be minted from the OS CSPRNG.
    NonceGeneration { detail: String },
    /// The probation child terminated before any HealthAck was accepted.
    ChildExitedBeforeAck { pid: u32 },
    /// No accepted HealthAck arrived within the bounded probation window.
    ProbationTimeout,
    /// A health marker was present but failed strict closed parsing or its
    /// size bound.
    HealthAckMalformed { detail: String },
    /// A health marker names a different session.
    HealthAckWrongSession,
    /// A health marker carries a nonce that is not the durable session
    /// nonce (the value itself is never reproduced in diagnostics).
    HealthAckWrongNonce,
    /// A health marker's process facts do not match the journaled
    /// probation child (PID, creation FILETIME, or image), or the process
    /// is no longer live with that identity.
    HealthAckWrongProcess { detail: String },
    /// A health marker was offered after health was already accepted — the
    /// nonce is consumed once; replay fails closed.
    HealthAckReplay,
    /// The commit could not publish the target receipt.
    ReceiptCommit { detail: String },
    /// The commit could not reconcile Windows integration at the target
    /// version.
    RegistryCommit { detail: String },
    /// The commit could not publish the installed signed evidence.
    EvidenceCommit { detail: String },
    /// A rollback backup slot is missing while the old set must be
    /// restored.
    RollbackAssetMissing { resource: String },
    /// A rollback backup slot does not hold the journaled preimage.
    RollbackAssetMismatch { resource: String, expected: String, actual: String },
    /// A rollback restoration could not be performed.
    RollbackRestore { resource: String, detail: String },
    /// A restored old runtime file does not verify against the journaled
    /// preimage.
    RollbackVerification { resource: String, expected: String, actual: String },
    /// Rollback could not reconcile Windows integration at the source
    /// version.
    RegistryRollback { detail: String },
    /// Rollback could not restore the previous (or absent) committed
    /// signed evidence.
    EvidenceRollback { detail: String },
    /// Best-effort cleanup after a terminal state failed. Never degrades
    /// the terminal verdict; recorded as cleanup debt.
    Cleanup { detail: String },
    /// Recovery evidence conflicts or cannot be proven.
    RecoveryInconsistent { detail: String },
    /// A lower-layer handoff validation refusal.
    Validation(HandoffError),
    Io { detail: String },
}

impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HelperImageMismatch { path } => write!(
                f,
                "the update transaction must be entered from the installed helper, not {path:?}"
            ),
            Self::HelperSpawn { detail } => write!(f, "session runner spawn failed: {detail}"),
            Self::RunnerNotReady => write!(
                f,
                "the session runner never signaled ownership; the transaction stays resumable"
            ),
            Self::CallerExited { pid } => write!(
                f,
                "the recorded caller process {pid} has exited; the handoff origin cannot be bound"
            ),
            Self::CallerIdentityMismatch { detail } => {
                write!(f, "live caller identity mismatch: {detail}")
            }
            Self::LeaseBusy { detail } => write!(f, "exclusive application lease busy: {detail}"),
            Self::LifecycleTransition { detail } => {
                write!(f, "Updating lifecycle transition failed: {detail}")
            }
            Self::SourceVersionChanged { recorded, current } => write!(
                f,
                "session source baseline {recorded} != actual installed {current}"
            ),
            Self::InstallationInvalid { detail } => {
                write!(f, "installation cannot support the transaction: {detail}")
            }
            Self::InstalledFileMissing { resource } => {
                write!(f, "installed managed file for {resource} is absent")
            }
            Self::InstalledHashMismatch { resource, expected, actual } => {
                write!(
                    f,
                    "installed {resource} hash {actual} != recorded preimage {expected}; repair conflict"
                )
            }
            Self::BackupFailed { resource, detail } => {
                write!(f, "backup of {resource} failed: {detail}")
            }
            Self::BackupVerification { resource, expected, actual } => {
                write!(f, "backup of {resource} hash {actual} != preimage {expected}")
            }
            Self::ReplacementFailed { resource, detail } => {
                write!(f, "replacement of {resource} failed: {detail}")
            }
            Self::ReplacementVerification { resource, expected, actual } => {
                write!(f, "replaced {resource} hash {actual} != signed target {expected}")
            }
            Self::UnsafePath { path } => write!(
                f,
                "path passes through a reparse point or hard link: {}",
                path.display()
            ),
            Self::JournalConflict { field } => write!(
                f,
                "durable journal field {field} disagrees with the validated facts; refusing"
            ),
            Self::JournalWrite { detail } => write!(f, "journal write failure: {detail}"),
            Self::Launch { detail } => write!(f, "new runtime launch failed: {detail}"),
            Self::NonceGeneration { detail } => {
                write!(f, "health nonce generation failed: {detail}")
            }
            Self::ChildExitedBeforeAck { pid } => {
                write!(f, "probation child {pid} exited before any HealthAck was accepted")
            }
            Self::ProbationTimeout => write!(
                f,
                "no accepted HealthAck arrived within the bounded probation window"
            ),
            Self::HealthAckMalformed { detail } => {
                write!(f, "health marker rejected: {detail}")
            }
            Self::HealthAckWrongSession => {
                write!(f, "health marker names a different session; refused")
            }
            Self::HealthAckWrongNonce => {
                write!(f, "health marker nonce does not match the durable session nonce; refused")
            }
            Self::HealthAckWrongProcess { detail } => {
                write!(f, "health marker process identity mismatch: {detail}")
            }
            Self::HealthAckReplay => write!(
                f,
                "health was already accepted for this session; the nonce is single-use"
            ),
            Self::ReceiptCommit { detail } => {
                write!(f, "commit could not publish the target receipt: {detail}")
            }
            Self::RegistryCommit { detail } => write!(
                f,
                "commit could not reconcile Windows integration at the target version: {detail}"
            ),
            Self::EvidenceCommit { detail } => write!(
                f,
                "commit could not publish the installed signed evidence: {detail}"
            ),
            Self::RollbackAssetMissing { resource } => {
                write!(f, "rollback asset for {resource} is missing; the old set is not provable")
            }
            Self::RollbackAssetMismatch { resource, expected, actual } => write!(
                f,
                "rollback asset for {resource} holds {actual}, journaled preimage is {expected}"
            ),
            Self::RollbackRestore { resource, detail } => {
                write!(f, "rollback restoration of {resource} failed: {detail}")
            }
            Self::RollbackVerification { resource, expected, actual } => write!(
                f,
                "restored {resource} hash {actual} != journaled preimage {expected}"
            ),
            Self::RegistryRollback { detail } => write!(
                f,
                "rollback could not reconcile Windows integration at the source version: {detail}"
            ),
            Self::EvidenceRollback { detail } => write!(
                f,
                "rollback could not restore the previous committed evidence state: {detail}"
            ),
            Self::Cleanup { detail } => {
                write!(f, "post-terminal cleanup debt: {detail}")
            }
            Self::RecoveryInconsistent { detail } => {
                write!(f, "recovery evidence is inconsistent: {detail}")
            }
            Self::Validation(error) => write!(f, "{error}"),
            Self::Io { detail } => write!(f, "transaction IO failure: {detail}"),
        }
    }
}

impl From<HandoffError> for MutationError {
    fn from(error: HandoffError) -> Self {
        match error {
            HandoffError::CallerExited { pid } => Self::CallerExited { pid },
            HandoffError::CallerIdentityMismatch { detail } => Self::CallerIdentityMismatch { detail },
            HandoffError::SourceVersionChanged { recorded, current } => {
                Self::SourceVersionChanged { recorded, current }
            }
            HandoffError::UnsafePath { path } => Self::UnsafePath { path },
            other => Self::Validation(other),
        }
    }
}

impl From<MutationError> for Error {
    fn from(error: MutationError) -> Self {
        Error::new(ErrorKind::UpdateApplyFailed, error.to_string())
    }
}

pub(crate) fn io_err(context: &str, error: impl std::fmt::Display) -> MutationError {
    MutationError::Io {
        detail: format!("{context}: {error}"),
    }
}

pub(crate) fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub(crate) fn hash_stream(file: &mut std::fs::File) -> Result<(u64, String), MutationError> {
    use sha2::Digest;
    use std::io::Read;
    let mut hasher = sha2::Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| io_err("hash read", e))?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    // Hex of the digest itself — `sha256_hex` would hash the digest again.
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn fingerprint(path: &Path) -> Result<(u64, String), MutationError> {
    let mut file = open_checked(path)?;
    hash_stream(&mut file)
}

/// `paths::open_regular` already rejects reparse points and multi-link files;
/// surface its refusal as the typed unsafe-path error.
pub(crate) fn open_checked(path: &Path) -> Result<std::fs::File, MutationError> {
    paths::open_regular(path).map_err(|e| match e.kind {
        ErrorKind::UnsafePath => MutationError::UnsafePath {
            path: path.to_path_buf(),
        },
        other => MutationError::Io {
            detail: format!("{other:?}: {}", e.detail),
        },
    })
}

pub(crate) fn assert_no_reparse(path: &Path) -> Result<(), MutationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x0400 != 0 {
                return Err(MutationError::UnsafePath { path: path.to_path_buf() });
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io_err("inspect", err)),
    }
}

/// Map a signed manifest identity to the compiled resource. Only the two
/// runtime identities can ever produce a plan.
fn resource_from_identity(identity: &str) -> Option<Resource> {
    let compiled = InstallIdentity::from_manifest_text(identity)?;
    Resource::from_filename(identity_filename(compiled))
}

/// The frozen backup/workspace directory for this session under the install
/// root: `<installRoot>\.maintenance\<sessionId>\`.
pub(crate) fn session_workspace(paths: &Paths, session_id: &str) -> PathBuf {
    paths.install().join(SESSION_WORKSPACE_DIR).join(session_id)
}

/// The fixed backup slot path for one resource — the frozen grammar
/// `.maintenance/<UUID>/<identity>.backup`, deterministically derived, never
/// a dynamic or provider-supplied filename.
pub(crate) fn backup_slot_path(paths: &Paths, session_id: &str, identity: &str) -> PathBuf {
    session_workspace(paths, session_id).join(format!("{identity}.backup"))
}

/// Every frozen fact of the durable journal must still agree with the
/// validated handoff. The session lives in user-writable space: an edited
/// journal can only make the transaction refuse, never widen it.
fn frozen_agree(validated: &ValidatedHandoff, current: &UpdateSessionEnvelope) -> bool {
    let v = validated.envelope();
    v.session_id == current.session_id
        && v.installation_id == current.installation_id
        && v.app_id == current.app_id
        && v.from_version == current.from_version
        && v.to_version == current.to_version
        && v.manifest_sha256 == current.manifest_sha256
        && v.package_sha256 == current.package_sha256
        && v.package_size == current.package_size
        && paths::equal(&v.install_root, &current.install_root)
        && paths::equal(&v.staging_root, &current.staging_root)
        && v.parent_process == current.parent_process
        && v.resources.len() == current.resources.len()
        && v.resources.iter().zip(current.resources.iter()).all(|(a, b)| {
            a.identity == b.identity
                && a.new_sha256 == b.new_sha256
                && a.new_size == b.new_size
                && a.backup_slot == b.backup_slot
        })
}

/// Load the durable journal and re-verify every frozen fact against the
/// validated handoff.
pub(crate) fn load_journal(validated: &ValidatedHandoff) -> Result<UpdateSessionEnvelope, MutationError> {
    let envelope = load_update_session(validated.session_dir()).map_err(|e| {
        MutationError::JournalConflict {
            field: format!("journal unreadable: {e}"),
        }
    })?;
    if !frozen_agree(validated, &envelope) {
        return Err(MutationError::JournalConflict {
            field: "frozen facts".to_string(),
        });
    }
    Ok(envelope)
}

/// Journal one mutation step: write-new-generation → atomic publish →
/// read-back verification. The frozen facts are re-verified on the durable
/// result; generation is monotonic by construction.
pub(crate) fn publish_mutation(
    validated: &ValidatedHandoff,
    mutate: impl FnOnce(&mut UpdateSessionEnvelope),
) -> Result<UpdateSessionEnvelope, MutationError> {
    let file = validated.session_dir().join(SESSION_FILE);
    assert_no_reparse(&file)?;
    let mut envelope = load_journal(validated)?;
    let generation = envelope.generation;
    mutate(&mut envelope);
    envelope.generation = generation
        .checked_add(1)
        .ok_or_else(|| MutationError::JournalConflict {
            field: "generation".to_string(),
        })?;
    envelope.updated_at = now_utc();
    let bytes = serde_json::to_vec_pretty(&envelope).map_err(|e| MutationError::JournalWrite {
        detail: e.to_string(),
    })?;
    paths::atomic_write(&file, &bytes).map_err(|e| MutationError::JournalWrite {
        detail: e.to_string(),
    })?;
    let published = load_journal(validated)?;
    if published.generation != generation + 1 {
        return Err(MutationError::JournalWrite {
            detail: "published journal generation did not advance".to_string(),
        });
    }
    Ok(published)
}

/// Durable handoff intent: session phase `staged` → `handed-off` with the
/// previous receipt and integration snapshots for the rollback half. The
/// journal leads; the receipt transition happens only after this is durable.
/// Idempotent at the crash window between the two writes.
fn journal_handoff_intent(paths: &Paths, validated: &ValidatedHandoff) -> Result<(), MutationError> {
    let envelope = load_journal(validated)?;
    match envelope.phase {
        SessionPhase::Staged => {
            // The previous-state snapshots come from the validated receipt —
            // the state this transaction is allowed to restore.
            let receipt = Receipt::load(paths)
                .map_err(|e| MutationError::InstallationInvalid {
                    detail: format!("receipt unusable for snapshots: {e}"),
                })?
                .ok_or_else(|| MutationError::InstallationInvalid {
                    detail: "no installation receipt".to_string(),
                })?;
            publish_mutation(validated, |envelope| {
                envelope.phase = SessionPhase::HandedOff;
                envelope.previous_receipt = Some(receipt.clone());
                envelope.previous_integration = Some(receipt.integration_resources.clone());
            })?;
            Ok(())
        }
        SessionPhase::HandedOff => Ok(()),
        other => Err(MutationError::JournalConflict {
            field: format!("phase {other:?}"),
        }),
    }
}

/// Durable `Updating` transition: receipt → `Updating` + `activeSessionId`,
/// after the journal intent is durable. `currentVersion` stays the last
/// committed version — Updating is not success. Idempotent.
fn transition_receipt_to_updating(
    paths: &Paths,
    validated: &ValidatedHandoff,
) -> Result<Receipt, MutationError> {
    let mut receipt = Receipt::load(paths)
        .map_err(|e| MutationError::InstallationInvalid {
            detail: format!("receipt unusable: {e}"),
        })?
        .ok_or_else(|| MutationError::InstallationInvalid {
            detail: "no installation receipt".to_string(),
        })?;
    if receipt.lifecycle_state == Lifecycle::Updating
        && receipt.maintenance.active_session_id.as_deref() == Some(validated.session_id())
    {
        return Ok(receipt);
    }
    if receipt.lifecycle_state != Lifecycle::Installed {
        return Err(MutationError::LifecycleTransition {
            detail: format!("receipt lifecycle is {:?}", receipt.lifecycle_state),
        });
    }
    if receipt.maintenance.active_session_id.is_some() {
        return Err(MutationError::LifecycleTransition {
            detail: "receipt already carries an active session".to_string(),
        });
    }
    receipt.lifecycle_state = Lifecycle::Updating;
    receipt.maintenance.active_session_id = Some(validated.session_id().to_string());
    receipt
        .save(paths)
        .map_err(|e| MutationError::LifecycleTransition {
            detail: e.to_string(),
        })?;
    Ok(receipt)
}

/// The runtime mutation plan for one resource, derived before the first
/// mutation.
struct ResourcePlan {
    identity: String,
    filename: &'static str,
    destination: PathBuf,
    /// The fixed frozen backup slot for this resource.
    backup_slot: PathBuf,
    staged_path: PathBuf,
    expected_old: (u64, String),
    expected_new: (u64, String),
}

fn staged_path(validated: &ValidatedHandoff, filename: &str) -> PathBuf {
    validated
        .session_dir()
        .join(crate::handoff::STAGED_DIR)
        .join(filename)
}

/// Precheck both resources against their durable facts before any mutation.
/// A resource the journal already marked completed must currently hold the
/// signed target bytes; everything else must hold the recorded preimage.
fn precheck_resources(
    plans: &[ResourcePlan],
    journal: &UpdateSessionEnvelope,
) -> Result<(), MutationError> {
    for plan in plans {
        let entry = journal
            .resources
            .iter()
            .find(|entry| entry.identity == plan.identity)
            .ok_or_else(|| MutationError::JournalConflict {
                field: format!("resources[{}]", plan.filename),
            })?;
        let mut installed = open_checked(&plan.destination)?;
        let (size, hash) = hash_stream(&mut installed)?;
        let installed_old = (size, hash.clone()) == plan.expected_old;
        let installed_new = (size, hash.clone()) == plan.expected_new;
        if entry.completed == Some(true) {
            if !installed_new {
                return Err(MutationError::ReplacementVerification {
                    resource: plan.filename.to_string(),
                    expected: plan.expected_new.1.clone(),
                    actual: hash,
                });
            }
        } else if !installed_old && installed_new {
            // Crash-D shape: the replacement side effect landed but the
            // completion is not journaled yet. Reconciliation is legal only
            // when the rollback asset verifiably holds the journaled
            // preimage — otherwise the old set cannot be proven and the
            // transaction refuses (recovery-required semantics), never
            // guesses.
            if entry.old_sha256.is_none() {
                return Err(MutationError::RecoveryInconsistent {
                    detail: format!(
                        "{} holds the target bytes before any backup facts were journaled",
                        plan.filename
                    ),
                });
            }
            verify_rollback_asset(plan, entry.old_sha256.as_deref())?;
        } else if !installed_old {
            return Err(MutationError::InstalledHashMismatch {
                resource: plan.filename.to_string(),
                expected: plan.expected_old.1.clone(),
                actual: hash,
            });
        }
    }
    Ok(())
}

/// Fresh-open the fixed backup slot and require it to hash exactly to the
/// journaled preimage. Never deletes, rewrites or "repairs" the slot.
fn verify_rollback_asset(
    plan: &ResourcePlan,
    expected_old: Option<&str>,
) -> Result<(u64, String), MutationError> {
    let Some(expected) = expected_old else {
        return Err(MutationError::RecoveryInconsistent {
            detail: format!("{} has no journaled preimage to verify the rollback asset against", plan.filename),
        });
    };
    if !plan.backup_slot.is_file() {
        return Err(MutationError::RecoveryInconsistent {
            detail: format!(
                "rollback asset for {} is missing while the destination already holds the target bytes",
                plan.filename
            ),
        });
    }
    let mut slot = open_checked(&plan.backup_slot)?;
    let facts = hash_stream(&mut slot)?;
    if facts.1 != expected {
        return Err(MutationError::RecoveryInconsistent {
            detail: format!(
                "rollback asset for {} does not hold the journaled preimage (found {})",
                plan.filename, facts.1
            ),
        });
    }
    Ok(facts)
}

/// Copy `source` to `destination` through a same-directory `.installing`
/// temporary, sync, hash-verify, then atomically move into place. Reuses the
/// Phase 1 primitives (`open_regular`, `move_replace`); creates no second
/// copy implementation.
pub(crate) fn copy_verified_to(
    source: &Path,
    destination: &Path,
    expected: (u64, String),
    resource: &str,
) -> Result<(), MutationError> {
    paths::pin_parents(destination).map_err(|e| io_err("pin parents", e))?;
    let mut input = open_checked(source)?;
    let temp = destination.with_extension(format!("{}.installing", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), MutationError> {
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| io_err(resource, e))?;
        std::io::copy(&mut input, &mut output).map_err(|e| io_err(resource, e))?;
        output.sync_all().map_err(|e| io_err(resource, e))?;
        drop(output);
        let mut copied = open_checked(&temp)?;
        let actual = hash_stream(&mut copied)?;
        drop(copied);
        if actual.1 != expected.1 || actual.0 != expected.0 {
            return Err(MutationError::BackupVerification {
                resource: resource.to_string(),
                expected: expected.1,
                actual: actual.1,
            });
        }
        paths::move_replace(&temp, destination).map_err(|e| io_err(resource, e))
    })();
    if result.is_err() {
        let _ = paths::remove_file(&temp);
    }
    result
}

/// Backup one resource: verify the installed preimage, copy it into the
/// fixed frozen slot, and fresh-verify the copy — never touching the
/// destination. Returns the verified old facts.
fn backup_resource(paths: &Paths, plan: &ResourcePlan, session_id: &str) -> Result<(u64, String), MutationError> {
    let workspace = session_workspace(paths, session_id);
    paths::create_dir(&workspace).map_err(|e| io_err("workspace", e))?;
    let slot = backup_slot_path(paths, session_id, plan.identity.as_str());
    assert_no_reparse(&slot)?;

    // Measure the preimage through a fresh, checked open — this measurement
    // is the old fact.
    let mut installed = open_checked(&plan.destination)?;
    let old = hash_stream(&mut installed)?;
    drop(installed);
    if old != plan.expected_old {
        return Err(MutationError::InstalledHashMismatch {
            resource: plan.filename.to_string(),
            expected: plan.expected_old.1.clone(),
            actual: old.1,
        });
    }

    // A slot from an interrupted run is only accepted when it verifiably
    // holds the preimage; anything else is removed and redone (it is this
    // transaction's own asset).
    if slot.is_file() {
        let mut existing = open_checked(&slot)?;
        let existing_facts = hash_stream(&mut existing)?;
        drop(existing);
        if existing_facts == old {
            return Ok(old);
        }
        paths::remove_file(&slot).map_err(|e| MutationError::BackupFailed {
            resource: plan.filename.to_string(),
            detail: e.to_string(),
        })?;
    }
    copy_verified_to(&plan.destination, &slot, old.clone(), plan.filename).map_err(|e| match e {
        MutationError::BackupVerification { expected, actual, .. } => {
            MutationError::BackupVerification {
                resource: plan.filename.to_string(),
                expected,
                actual,
            }
        }
        other => MutationError::BackupFailed {
            resource: plan.filename.to_string(),
            detail: other.to_string(),
        },
    })?;
    // Fresh-open verification of the durable backup fact.
    let mut verified = open_checked(&slot)?;
    let backup = hash_stream(&mut verified)?;
    if backup != old {
        return Err(MutationError::BackupVerification {
            resource: plan.filename.to_string(),
            expected: old.1,
            actual: backup.1,
        });
    }
    Ok(old)
}

/// Replace one resource: copy the staged bytes into the same-volume
/// workspace, verify the copy, atomically move over the destination, then
/// fresh-open and hash-verify the result against the signed facts. Success is
/// never inferred from the rename itself.
fn replace_resource(paths: &Paths, plan: &ResourcePlan, session_id: &str) -> Result<(), MutationError> {
    let workspace = session_workspace(paths, session_id).join(format!("{}.staged", plan.identity));
    assert_no_reparse(&workspace)?;
    copy_verified_to(&plan.staged_path, &workspace, plan.expected_new.clone(), plan.filename).map_err(
        |e| match e {
            MutationError::BackupVerification { expected, actual, .. } => {
                MutationError::ReplacementVerification {
                    resource: plan.filename.to_string(),
                    expected,
                    actual,
                }
            }
            other => MutationError::ReplacementFailed {
                resource: plan.filename.to_string(),
                detail: other.to_string(),
            },
        },
    )?;
    // Identity re-check immediately before the mutation (TOCTOU discipline).
    assert_no_reparse(&plan.destination)?;
    paths::move_replace(&workspace, &plan.destination).map_err(|e| MutationError::ReplacementFailed {
        resource: plan.filename.to_string(),
        detail: e.to_string(),
    })?;
    // Fresh-open destination: size and hash must equal the signed facts.
    let mut replaced = open_checked(&plan.destination)?;
    let actual = hash_stream(&mut replaced)?;
    if actual != plan.expected_new {
        return Err(MutationError::ReplacementVerification {
            resource: plan.filename.to_string(),
            expected: plan.expected_new.1.clone(),
            actual: actual.1,
        });
    }
    Ok(())
}

/// Apply one resource under the frozen ordering: intent → backup → verify →
/// journal old facts → replace → fresh verify → journal completion. Every
/// step is idempotent, so a resumed apply continues exactly where the
/// journal says it stopped.
fn apply_resource(
    paths: &Paths,
    validated: &ValidatedHandoff,
    plan: &ResourcePlan,
    session_id: &str,
) -> Result<(), MutationError> {
    let journal = load_journal(validated)?;
    let entry = journal
        .resources
        .iter()
        .find(|entry| entry.identity == plan.identity)
        .ok_or_else(|| MutationError::JournalConflict {
            field: format!("resources[{}]", plan.filename),
        })?;
    let needs_intent = entry.intent.is_none();
    let needs_backup = entry.old_sha256.is_none();
    let needs_replace = entry.completed != Some(true);
    // 1. Durable intent before any side effect.
    if needs_intent {
        publish_mutation(validated, |envelope| {
            if let Some(entry) = envelope
                .resources
                .iter_mut()
                .find(|entry| entry.identity == plan.identity)
            {
                entry.intent = Some(ResourceAction::Replace);
            }
        })?;
    }
    // 2–4. Backup, verify, journal the old facts.
    if needs_backup {
        let (old_size, old_hash) = backup_resource(paths, plan, session_id)?;
        publish_mutation(validated, |envelope| {
            if let Some(entry) = envelope
                .resources
                .iter_mut()
                .find(|entry| entry.identity == plan.identity)
            {
                entry.old_present = true;
                entry.old_size = Some(old_size);
                entry.old_sha256 = Some(old_hash);
            }
        })?;
    }
    // 5–6. Replace and journal completion only after fresh verification.
    if needs_replace {
        // Crash-D reconciliation: when the replacement side effect already
        // landed (the destination verifiably holds the signed target bytes)
        // and the fixed rollback slot verifiably holds the journaled
        // preimage, the crash happened between the move and the completion
        // publish. Reconcile the journal instead of redoing the side
        // effect — the OLD backup slot is preserved byte-identical, never
        // overwritten, never re-derived from the target bytes.
        let mut reconciled = false;
        if entry.old_sha256.is_some() {
            let mut installed = open_checked(&plan.destination)?;
            let facts = hash_stream(&mut installed)?;
            if facts == plan.expected_new {
                verify_rollback_asset(plan, entry.old_sha256.as_deref())?;
                reconciled = true;
            }
        }
        if !reconciled {
            replace_resource(paths, plan, session_id)?;
        }
        publish_mutation(validated, |envelope| {
            if let Some(entry) = envelope
                .resources
                .iter_mut()
                .find(|entry| entry.identity == plan.identity)
            {
                entry.completed = Some(true);
            }
        })?;
    }
    Ok(())
}

/// Run every runtime mutation: derive the plan, precheck both resources
/// against their durable facts, then the ordered per-resource apply. The
/// receipt stays `Updating` throughout; nothing else is touched.
pub(crate) fn apply_replacements(paths: &Paths, validated: &ValidatedHandoff) -> Result<(), MutationError> {
    let session_id = validated.session_id().to_string();
    let receipt = Receipt::load(paths)
        .map_err(|e| MutationError::InstallationInvalid {
            detail: format!("receipt unusable: {e}"),
        })?
        .ok_or_else(|| MutationError::InstallationInvalid {
            detail: "no installation receipt".to_string(),
        })?;
    let journal = load_journal(validated)?;
    let mut plans = Vec::new();
    for resource in [Resource::MainExecutable, Resource::MaintenanceHelper] {
        let staged = validated
            .staged()
            .iter()
            .find(|entry| resource_from_identity(entry.identity.as_str()) == Some(resource))
            .ok_or_else(|| MutationError::InstallationInvalid {
                detail: format!("validated handoff carries no staged entry for {resource:?}"),
            })?;
        let journal_entry = journal
            .resources
            .iter()
            .find(|entry| resource_from_identity(entry.identity.as_str()) == Some(resource))
            .ok_or_else(|| MutationError::JournalConflict {
                field: format!("resources[{}]", resource.filename()),
            })?;
        let destination = paths.install().join(resource.filename());
        assert_no_reparse(&destination)?;
        if !destination.is_file() {
            return Err(MutationError::InstalledFileMissing {
                resource: resource.filename().to_string(),
            });
        }
        let record = receipt
            .runtime_resources
            .iter()
            .find(|entry| entry.identity == resource)
            .ok_or_else(|| MutationError::InstallationInvalid {
                detail: format!("receipt carries no runtime record for {resource:?}"),
            })?;
        // The staged source must still be exactly the signed bytes.
        let mut staged_file = open_checked(&staged_path(validated, staged.filename.as_str()))?;
        let staged_facts = hash_stream(&mut staged_file)?;
        if staged_facts != (staged.size, staged.sha256_hex.clone()) {
            return Err(MutationError::ReplacementVerification {
                resource: resource.filename().to_string(),
                expected: staged.sha256_hex.clone(),
                actual: staged_facts.1,
            });
        }
        let _ = journal_entry;
        plans.push(ResourcePlan {
            identity: staged.identity.clone(),
            filename: resource.filename(),
            destination,
            backup_slot: backup_slot_path(paths, &session_id, staged.identity.as_str()),
            staged_path: staged_path(validated, staged.filename.as_str()),
            expected_old: (record.size, record.sha256.clone()),
            expected_new: (staged.size, staged.sha256_hex.clone()),
        });
    }
    precheck_resources(&plans, &journal)?;
    for plan in &plans {
        apply_resource(paths, validated, plan, &session_id)?;
    }
    Ok(())
}

/// Wait (bounded) for the exclusive application lease: the spawning app is
/// expected to exit and release its shared lease. Handle-based, so a crashed
/// owner releases it immediately — file existence is never consulted.
pub(crate) fn wait_exclusive_lease(
    paths: &Paths,
    attempts: u32,
    interval: Duration,
) -> Result<lock::AppLease, MutationError> {
    for _ in 0..attempts {
        match lock::exclusive_app(paths) {
            Ok(lease) => return Ok(lease),
            Err(_) => std::thread::sleep(interval),
        }
    }
    Err(MutationError::LeaseBusy {
        detail: "the application lease never released".to_string(),
    })
}

/// The runner-side ready marker: written after the runner holds the gate, so
/// the delegating parent only exits once ownership actually transferred.
fn ready_marker(runner_dir: &Path) -> PathBuf {
    runner_dir.join("runner-ready.json")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyMarker {
    pid: u32,
}

/// Execute the transaction on the runner side. Preconditions (checked by the
/// production entry before calling): this process runs from a verified
/// session-runner copy. The gate is taken first (an abandoned mutex from the
/// delegating parent is a legal acquire), the ready marker signals ownership,
/// and the full Resume validation re-derives every fact from disk before the
/// lease wait and the first mutation.
///
/// **The replacement phase's success endpoint is the durable
/// `ReplacedAwaitingLaunch` state**: both executables replaced and verified,
/// receipt `Updating`, backups retained — and, deliberately, the exclusive
/// lease still held by this process. The returned validated handoff and
/// lease are exactly what the Phase 2D-B probation flow continues from: it
/// journals the probation-ready record, releases the lease, launches the
/// child and owns the commit/rollback decision. Nothing here commits,
/// accepts health, rolls back or cleans up.
pub fn continue_update_transaction(
    paths: &Paths,
    trust: &TrustStore,
    runner_dir: &Path,
    session_id: &str,
    expected_manifest_sha256: &str,
) -> Result<(ValidatedHandoff, lock::AppLease), MutationError> {
    validate_uuid(session_id).map_err(|e| {
        MutationError::Validation(HandoffError::CliArgument {
            detail: e.to_string(),
        })
    })?;
    let _gate = Gate::acquire(paths).map_err(|e| MutationError::LeaseBusy {
        detail: e.to_string(),
    })?;
    paths::atomic_write(
        &ready_marker(runner_dir),
        serde_json::to_vec(&ReadyMarker {
            pid: std::process::id(),
        })
        .map_err(|e| MutationError::JournalWrite {
            detail: e.to_string(),
        })?
        .as_slice(),
    )
    .map_err(|e| MutationError::JournalWrite {
        detail: e.to_string(),
    })?;
    let validated = handoff::validate_resume(paths, trust, session_id, expected_manifest_sha256)?;
    let lease = wait_exclusive_lease(paths, LEASE_ATTEMPTS, RETRY_INTERVAL)?;
    crate::lifecycle::require_no_legacy_processes(paths).map_err(|e| MutationError::LeaseBusy {
        detail: e.to_string(),
    })?;
    apply_replacements(paths, &validated)?;
    Ok((validated, lease))
}

/// Stage the session runner: copy this (installed helper) image into a fresh
/// private runner directory under the compiled runners root, hash-verify the
/// copy, and record its expected digest. Returns the runner executable and
/// its ready-marker path.
fn stage_runner_copy(paths: &Paths) -> Result<(PathBuf, PathBuf), MutationError> {
    let current = std::env::current_exe().map_err(|e| MutationError::HelperSpawn {
        detail: e.to_string(),
    })?;
    let runners = paths.state().join("runners");
    let destination = runners.join(uuid::Uuid::new_v4().to_string());
    paths::create_dir(&destination).map_err(|e| MutationError::HelperSpawn {
        detail: format!("runner directory: {e}"),
    })?;
    let target = destination.join(HELPER_EXE);
    let (size, hash) = fingerprint(&current)?;
    {
        let mut input = open_checked(&current)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| MutationError::HelperSpawn {
                detail: e.to_string(),
            })?;
        std::io::copy(&mut input, &mut output).map_err(|e| MutationError::HelperSpawn {
            detail: e.to_string(),
        })?;
        output.sync_all().map_err(|e| MutationError::HelperSpawn {
            detail: e.to_string(),
        })?;
    }
    let copied = fingerprint(&target)?;
    if copied != (size, hash.clone()) {
        return Err(MutationError::HelperSpawn {
            detail: "runner copy mismatch".to_string(),
        });
    }
    paths::atomic_write(&destination.join("runner.sha256"), hash.as_bytes())
        .map_err(|e| MutationError::HelperSpawn {
            detail: e.to_string(),
        })?;
    Ok((target, ready_marker(&destination)))
}

/// Spawn the staged runner with the exact frozen argv (absolute path, no
/// shell, no PATH lookup, no extra arguments) and wait — bounded — for the
/// ownership handshake. `spawn_runner` and the wait bound are injectable for
/// deterministic tests; the production implementation is
/// [`spawn_runner_process`] with [`RUNNER_READY_ATTEMPTS`].
fn hand_off_to_runner(
    runner_exe: &Path,
    marker: &Path,
    session_id: &str,
    digest: &str,
    ready_attempts: u32,
    spawn_runner: &dyn Fn(&Path, &[String]) -> Result<(), MutationError>,
) -> Result<(), MutationError> {
    let argv = [
        "--update".to_string(),
        "--session-id".to_string(),
        session_id.to_string(),
        "--expected-manifest-sha256".to_string(),
        digest.to_string(),
    ];
    spawn_runner(runner_exe, &argv)?;
    for _ in 0..ready_attempts {
        if marker.is_file() {
            return Ok(());
        }
        std::thread::sleep(RETRY_INTERVAL);
    }
    Err(MutationError::RunnerNotReady)
}

/// The production runner spawn: `Command::new(exact path)` with the exact
/// argv and no window. Never a shell, never PATH lookup.
pub fn spawn_runner_process(runner_exe: &Path, argv: &[String]) -> Result<(), MutationError> {
    use std::os::windows::process::CommandExt;
    Command::new(runner_exe)
        .args(argv)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .map(|_child| ())
        .map_err(|e| MutationError::HelperSpawn {
            detail: e.to_string(),
        })
}

/// The first half of the transaction, executed by the installed helper:
/// live caller binding, full Initial validation (which journals the verified
/// digest), the maintenance gate, the durable handoff intent and the
/// `Updating` transition. The gate is released before the runner is spawned
/// (the runner must be able to take it); between the two, the `Updating`
/// receipt already refuses every other entrance.
///
/// `running_image`, `live_caller` and `installed_source` are the caller's
/// observed facts — the production entry derives them from the OS
/// ([`update_entry`]); they are parameters so the semantics are
/// deterministically testable. No mutation has happened when this returns:
/// the runner owns the rest.
pub fn begin_update_transaction(
    paths: &Paths,
    trust: &TrustStore,
    running_image: &Path,
    live_caller: &handoff::LiveProcessIdentity,
    installed_source: &Version,
    session_id: &str,
    expected_manifest_sha256: &str,
) -> Result<(), MutationError> {
    // The transaction is only ever entered from the installed helper image;
    // a copy invoked from anywhere else is refused before anything is read.
    if !paths::equal(running_image, &paths.install().join(HELPER_EXE)) {
        return Err(MutationError::HelperImageMismatch {
            path: running_image.to_path_buf(),
        });
    }
    // Bind the live caller before anything else: the recorded parent process
    // must match PID + creation time + canonical image exactly.
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    let envelope = load_update_session(&dir).map_err(|e| MutationError::JournalWrite {
        detail: format!("session unreadable: {e}"),
    })?;
    validate_caller_identity(paths, &envelope.parent_process, live_caller)?;
    let validated = handoff::validate_initial_with_source(
        paths,
        trust,
        session_id,
        expected_manifest_sha256,
        installed_source,
        live_caller,
    )?;
    let _gate = Gate::acquire(paths).map_err(|e| MutationError::LeaseBusy {
        detail: e.to_string(),
    })?;
    journal_handoff_intent(paths, &validated)?;
    transition_receipt_to_updating(paths, &validated)?;
    drop(_gate);
    Ok(())
}

/// The production Initial-facts derivation: the installed-version authority
/// (receipt, PE image, committed evidence chain) from the shared library.
pub fn derive_installed_source_checked(
    paths: &Paths,
    trust: &TrustStore,
) -> Result<Version, MutationError> {
    handoff::derive_installed_source(paths, trust)
        .map_err(|e| MutationError::InstallationInvalid {
            detail: e.to_string(),
        })
}

/// Detect and verify session-runner provenance: this image executes from a
/// runner directory under the compiled runners root and matches its recorded
/// digest. Returns the runner directory when proven, `None` for an ordinary
/// (installed-helper) invocation.
pub fn detect_runner_provenance(paths: &Paths) -> Result<Option<PathBuf>, MutationError> {
    let current = std::env::current_exe().map_err(|e| MutationError::Io {
        detail: e.to_string(),
    })?;
    let parent = current.parent().ok_or_else(|| MutationError::Io {
        detail: "running image has no parent directory".to_string(),
    })?;
    let runners = paths.state().join("runners");
    let is_runner = parent
        .parent()
        .is_some_and(|grand| paths::equal(grand, &runners));
    if !is_runner {
        return Ok(None);
    }
    let id = parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MutationError::InstallationInvalid {
            detail: "invalid runner identity".to_string(),
        })?;
    validate_uuid(id).map_err(|e| MutationError::InstallationInvalid {
        detail: e.to_string(),
    })?;
    let expected = std::fs::read_to_string(parent.join("runner.sha256"))
        .map_err(|e| MutationError::InstallationInvalid {
            detail: format!("runner digest unreadable: {e}"),
        })?;
    let (_, actual) = fingerprint(&current)?;
    if actual != expected.trim() {
        return Err(MutationError::InstallationInvalid {
            detail: "runner copy changed".to_string(),
        });
    }
    Ok(Some(parent.to_path_buf()))
}

/// The full production `--update` entry: detects whether this process is the
/// installed helper or a verified session runner and executes the
/// corresponding half. The runner half completes the replacement phase (the
/// durable `ReplacedAwaitingLaunch` state) and then continues straight into
/// the Phase 2D-B probation flow — nonce, launch, HealthAck, and the
/// commit/rollback decision — which owns the transaction's terminal verdict.
/// On a rollback verdict the caller reports through the returned outcome so
/// the failure is surfaced, never relabeled update success.
pub fn update_entry(paths: &Paths, session_id: &str, expected_manifest_sha256: &str) -> Result<(), Error> {
    let trust = desktop_todo_update_core::production_trust_store();
    if let Some(runner_dir) = detect_runner_provenance(paths).map_err(Error::from)? {
        let (validated, lease) =
            continue_update_transaction(paths, &trust, &runner_dir, session_id, expected_manifest_sha256)
                .map_err(Error::from)?;
        crate::probation::settle_after_apply(paths, &trust, validated, lease)
            .map_err(Error::from)?;
        Ok(())
    } else {
        // Installed-helper half: derive the Initial facts from the OS —
        // the running image, the live caller identity, the installed source
        // version — then run the transaction through the durable `Updating`
        // transition and delegate to the runner.
        let running_image = std::env::current_exe().map_err(|e| {
            Error::new(ErrorKind::UpdateApplyFailed, e.to_string())
        })?;
        if !paths::equal(&running_image, &paths.install().join(HELPER_EXE)) {
            return Err(MutationError::HelperImageMismatch {
                path: running_image,
            }
            .into());
        }
        let dir = paths
            .state()
            .join("updates")
            .join("sessions")
            .join(session_id);
        let envelope =
            load_update_session(&dir).map_err(|e| MutationError::JournalWrite {
                detail: format!("session unreadable: {e}"),
            })?;
        let live = query_live_process_identity(envelope.parent_process.pid).map_err(Error::from)?;
        let source = derive_installed_source_checked(paths, &trust).map_err(Error::from)?;
        begin_update_transaction(
            paths,
            &trust,
            &running_image,
            &live,
            &source,
            session_id,
            expected_manifest_sha256,
        )
        .map_err(Error::from)?;
        let (runner_exe, marker) = stage_runner_copy(paths).map_err(Error::from)?;
        hand_off_to_runner(
            &runner_exe,
            &marker,
            session_id,
            expected_manifest_sha256,
            RUNNER_READY_ATTEMPTS,
            &spawn_runner_process,
        )
        .map_err(Error::from)?;
        Ok(())
    }
}

// -------------------------------------------------------- recovery classification

/// Read-only recovery fact for one managed resource, as observed on disk and
/// in the durable journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRecoveryFact {
    pub identity: String,
    pub intent_recorded: bool,
    /// The old facts are durably journaled.
    pub backup_recorded: bool,
    pub backup_slot_present: bool,
    /// `Some(true)` only when the slot exists and verifiably holds the
    /// journaled preimage.
    pub backup_slot_matches_old: Option<bool>,
    pub replace_completed: bool,
    pub installed_matches_old: Option<bool>,
    pub installed_matches_new: Option<bool>,
}

/// The classified position of an interrupted or completed update
/// transaction. These are recovery-observation states over the frozen
/// journal fields — never new protocol states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationRecoveryState {
    /// No update session exists for this installation.
    NoActiveUpdate,
    /// The session is staged/handed-off but the receipt is still `Installed`
    /// with zero mutation: authority was never transferred (crash between
    /// the journal intent and the receipt transition, or earlier).
    HandoffPrepared,
    /// The receipt is `Updating` and no resource intent exists.
    UpdatingNoMutation,
    /// Some resource sits between durable intent and journaled backup facts.
    BackupPartial,
    /// Some resource sits between journaled backup facts and a completed,
    /// verified replacement.
    ReplacePartial,
    /// Every resource completed and verified; the new runtime was not
    /// launched (or its launch was not journaled).
    ReplacedAwaitingLaunch,
    /// The new runtime was launched and recorded; health is pending.
    LaunchedAwaitingHealth,
    /// Health was accepted (or a commit decision is journaled) and the
    /// receipt is still `Updating`: the commit transaction resumes here —
    /// never a re-run of the replacement (Phase 2D-B).
    CommitPending,
    /// A rollback decision is journaled and the receipt is still `Updating`:
    /// the rollback transaction resumes here. Per-resource restoration
    /// progress is visible in the resource facts (destination old vs new).
    RollbackPending,
    /// The receipt is `Installed` at the target version but the session is
    /// not finalized: the receipt is authoritative (frozen lattice) — verify
    /// the installed set and integration, then finalize `committed`.
    CommittedPendingFinalization,
    /// The receipt is `Installed` at the previous version after a rollback
    /// decision but the session is not finalized: verify the restored old
    /// set, then finalize `rolled-back`.
    RolledBackPendingFinalization,
    /// The session reached its `committed` terminal with the receipt at the
    /// target: only bounded cleanup may remain.
    CommittedCleanupPending,
    /// The session reached its `rolled-back` terminal with the receipt at
    /// the previous version: only bounded cleanup may remain.
    RolledBackCleanupPending,
    /// The session reached the `recovery-required` terminal: no resume; a
    /// new session (or manual repair) is required.
    RecoveryRequired,
    /// Evidence conflicts or cannot be proven: recovery-required semantics —
    /// no guessed destructive action.
    Inconsistent { detail: String },
}

/// The full read-only recovery classification of one session.
#[derive(Debug, Clone)]
pub struct RecoveryClassification {
    pub session_id: String,
    pub receipt_lifecycle: Option<Lifecycle>,
    pub session_phase: Option<SessionPhase>,
    pub state: MutationRecoveryState,
    pub resources: Vec<ResourceRecoveryFact>,
}

impl RecoveryClassification {
    /// Human-readable report for the helper's native output (diagnostics
    /// only; never authority).
    pub fn report_text(&self) -> String {
        let phase = self
            .session_phase
            .as_ref()
            .map(|phase| format!("{phase:?}"))
            .unwrap_or_else(|| "<missing>".to_string());
        let lifecycle = self
            .receipt_lifecycle
            .as_ref()
            .map(|state| format!("{state:?}"))
            .unwrap_or_else(|| "<missing>".to_string());
        let state = match &self.state {
            MutationRecoveryState::Inconsistent { detail } => format!("Inconsistent ({detail})"),
            other => format!("{other:?}"),
        };
        let mut text = format!(
            "Update session {}\nReceipt lifecycle: {lifecycle}\nSession phase: {phase}\nClassified state: {state}\n",
            self.session_id
        );
        for fact in &self.resources {
            text.push_str(&format!(
                "- {}: intent={} backupRecorded={} backupSlot={} completed={} installedOld={:?} installedNew={:?}\n",
                fact.identity,
                fact.intent_recorded,
                fact.backup_recorded,
                fact.backup_slot_present,
                fact.replace_completed,
                fact.installed_matches_old,
                fact.installed_matches_new,
            ));
        }
        text
    }
}

/// Classify one update session, read-only: receipt + strict session parse +
/// independent signature re-verification over the persisted bytes + per-
/// resource file inspection. Never mutates anything; evidence conflicts are
/// classified `Inconsistent`, never guessed through.
pub fn classify_update_recovery(
    paths: &Paths,
    trust: &TrustStore,
    session_id: &str,
) -> Result<RecoveryClassification, MutationError> {
    if validate_uuid(session_id).is_err() {
        return Err(MutationError::RecoveryInconsistent {
            detail: "session id is not a canonical UUID".to_string(),
        });
    }
    let dir = paths.state().join("updates").join("sessions").join(session_id);
    let receipt = Receipt::load(paths).ok().flatten();
    let envelope = load_update_session(&dir).ok();
    let (Some(receipt), Some(envelope)) = (&receipt, envelope) else {
        // A missing/unreadable journal has two legal readings: no update at
        // all (Installed/absent receipt — the normal case), or — per the
        // frozen crash lattice — RecoveryRequired semantics when the receipt
        // is `Updating` and the journal is gone.
        if let Some(receipt) = &receipt {
            if receipt.lifecycle_state == Lifecycle::Updating {
                return Ok(RecoveryClassification {
                    session_id: session_id.to_string(),
                    receipt_lifecycle: Some(receipt.lifecycle_state),
                    session_phase: None,
                    state: MutationRecoveryState::Inconsistent {
                        detail: "receipt is Updating but the journal is missing or unreadable"
                            .to_string(),
                    },
                    resources: Vec::new(),
                });
            }
        }
        return Ok(RecoveryClassification {
            session_id: session_id.to_string(),
            receipt_lifecycle: receipt.as_ref().map(|r| r.lifecycle_state),
            session_phase: None,
            state: MutationRecoveryState::NoActiveUpdate,
            resources: Vec::new(),
        });
    };

    // Independent validity: the persisted bytes must re-verify through the
    // shared core and agree with the journal's frozen facts.
    let mut notes: Vec<String> = Vec::new();
    let manifest =
        handoff::read_bounded(&dir.join(crate::handoff::MANIFEST_FILE), "persisted manifest");
    let envelope_bytes =
        handoff::read_bounded(&dir.join(crate::handoff::ENVELOPE_FILE), "persisted envelope");
    match (manifest, envelope_bytes) {
        (Ok(manifest), Ok(bytes)) => match verify_and_parse(trust, &bytes, &manifest) {
            Ok(target) => {
                if sha256_hex(&manifest) != envelope.manifest_sha256 {
                    notes.push("persisted manifest digest disagrees with the journal".to_string());
                }
                if target.manifest().version().to_string() != envelope.to_version {
                    notes.push("verified target version disagrees with the journal".to_string());
                }
                for entry in &envelope.resources {
                    let signed = target
                        .manifest()
                        .asset()
                        .install_files()
                        .iter()
                        .find(|file| file.identity() == entry.identity);
                    match signed {
                        Some(file) if file.sha256_hex() == entry.new_sha256 && file.size() == entry.new_size => {}
                        Some(_) => notes.push(format!(
                            "journal target facts for {} disagree with the signed manifest",
                            entry.identity
                        )),
                        None => notes.push(format!(
                            "journal names identity {} outside the signed manifest",
                            entry.identity
                        )),
                    }
                }
            }
            Err(error) => notes.push(format!("persisted bytes fail re-verification: {error}")),
        },
        (manifest, bytes) => notes.push(format!(
            "persisted evidence unreadable (manifest: {}, envelope: {})",
            manifest.is_err(),
            bytes.is_err()
        )),
    };

    // Per-resource observation: journal facts + actual installed/backup bytes.
    let mut facts = Vec::new();
    for entry in &envelope.resources {
        let identity = entry.identity.as_str();
        let filename = match resource_from_identity(identity) {
            Some(resource) => resource.filename(),
            None => {
                return Err(MutationError::RecoveryInconsistent {
                    detail: format!("journal names unknown identity {identity:?}"),
                })
            }
        };
        let destination = paths.install().join(filename);
        // The preimage authority is the validated receipt record; the
        // journal's old facts (absent before the backup journal write) are
        // the fallback reference.
        let preimage = receipt
            .runtime_resources
            .iter()
            .find(|record| resource_from_identity(identity) == Some(record.identity))
            .map(|record| record.sha256.clone())
            .or_else(|| entry.old_sha256.clone());
        let installed = if destination.is_file() {
            match open_checked(&destination) {
                Ok(mut file) => hash_stream(&mut file).ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        let slot = backup_slot_path(paths, session_id, identity);
        let slot_hash = if slot.is_file() {
            match open_checked(&slot) {
                Ok(mut file) => hash_stream(&mut file).ok(),
                Err(_) => None,
            }
        } else {
            None
        };
        facts.push(ResourceRecoveryFact {
            identity: identity.to_string(),
            intent_recorded: entry.intent.is_some(),
            backup_recorded: entry.old_sha256.is_some() && entry.old_size.is_some(),
            backup_slot_present: slot.is_file(),
            backup_slot_matches_old: slot_hash
                .as_ref()
                .map(|found| Some(found.1.clone()) == preimage),
            replace_completed: entry.completed == Some(true),
            installed_matches_old: installed
                .as_ref()
                .map(|found| Some(found.1.clone()) == preimage),
            installed_matches_new: installed
                .map(|found| found == (entry.new_size, entry.new_sha256.clone())),
        });
    }

    let all_completed = facts.iter().all(|fact| fact.replace_completed);
    let any_intent = facts.iter().any(|fact| fact.intent_recorded);

    // The journaled decision boundary (Phase 2D-B): a typed commit/rollback
    // decision. A present but malformed value cannot be interpreted — the
    // journal is evidence, and unreadable evidence is inconsistency, never
    // permission to guess.
    let decision: Option<Result<crate::probation::CommitIntent, String>> = envelope
        .commit_intent
        .as_ref()
        .map(|value| crate::probation::parse_commit_intent(value));
    let rollback_in_progress = matches!(
        &decision,
        Some(Ok(crate::probation::CommitIntent {
            decision: crate::probation::CommitDecision::Rollback,
            ..
        }))
    );
    let decisions: Vec<&crate::probation::CommitIntent> = decision
        .iter()
        .filter_map(|parsed| parsed.as_ref().ok())
        .collect();

    // Phase-ordering invariants over the probation fields: the nonce is
    // minted only after every replacement completed, health is accepted only
    // after a nonce exists, a decision is journaled only after a nonce
    // exists, and the launch record only after a nonce exists. (A rollback
    // decision needs no accepted health — timeouts and launch failures roll
    // back without any ack — but it still lives in the post-replacement
    // phase.) While a rollback is in progress the "completed ⇒ target bytes"
    // invariant is deliberately suspended: the transaction is un-replacing
    // the managed set, so a completed resource legitimately holds its OLD
    // bytes again.
    let tampered_completed = !rollback_in_progress
        && facts
            .iter()
            .any(|fact| fact.replace_completed && fact.installed_matches_new == Some(false));
    let ordering_notes = [
        (!all_completed && (envelope.health_nonce.is_some() || envelope.accepted_health.is_some() || envelope.commit_intent.is_some()))
            .then(|| "probation/commit fields present before the replacements completed".to_string()),
        (envelope.accepted_health.is_some() && envelope.health_nonce.is_none())
            .then(|| "health accepted without any journaled nonce".to_string()),
        (envelope.commit_intent.is_some() && envelope.health_nonce.is_none())
            .then(|| "a decision was journaled without any journaled nonce".to_string()),
        (envelope.probation_process.is_some() && envelope.health_nonce.is_none())
            .then(|| "a probation launch was journaled without any journaled nonce".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<String>>();

    let state = if let Some(Err(detail)) = &decision {
        MutationRecoveryState::Inconsistent {
            detail: format!("journaled decision is malformed: {detail}"),
        }
    } else if let Some(first) = notes.first().or_else(|| ordering_notes.first()) {
        MutationRecoveryState::Inconsistent {
            detail: first.clone(),
        }
    } else if tampered_completed {
        MutationRecoveryState::Inconsistent {
            detail: "a completed resource no longer holds the signed target bytes".to_string(),
        }
    } else {
        match receipt.lifecycle_state {
            Lifecycle::Installed => {
                let commit_shape = matches!(
                    decisions.first(),
                    Some(crate::probation::CommitIntent {
                        decision: crate::probation::CommitDecision::Commit,
                        ..
                    })
                ) && receipt.current_version.as_deref() == Some(envelope.to_version.as_str());
                let rollback_shape = matches!(
                    decisions.first(),
                    Some(crate::probation::CommitIntent {
                        decision: crate::probation::CommitDecision::Rollback,
                        ..
                    })
                ) && receipt.current_version.as_deref() == Some(envelope.from_version.as_str());
                if envelope.phase == SessionPhase::Committed
                    && receipt.current_version.as_deref() == Some(envelope.to_version.as_str())
                {
                    MutationRecoveryState::CommittedCleanupPending
                } else if envelope.phase == SessionPhase::RolledBack
                    && receipt.current_version.as_deref() == Some(envelope.from_version.as_str())
                {
                    MutationRecoveryState::RolledBackCleanupPending
                } else if envelope.phase == SessionPhase::RecoveryRequired {
                    MutationRecoveryState::RecoveryRequired
                } else if envelope.phase == SessionPhase::HandedOff
                    && !any_pending_mutation(&facts)
                    && (commit_shape || rollback_shape)
                {
                    if commit_shape {
                        MutationRecoveryState::CommittedPendingFinalization
                    } else {
                        MutationRecoveryState::RolledBackPendingFinalization
                    }
                } else if !any_intent
                    && envelope.probation_process.is_none()
                    && envelope.health_nonce.is_none()
                    && envelope.accepted_health.is_none()
                    && envelope.commit_intent.is_none()
                {
                    MutationRecoveryState::HandoffPrepared
                } else {
                    MutationRecoveryState::Inconsistent {
                        detail: "mutation facts exist without the Updating receipt".to_string(),
                    }
                }
            }
            Lifecycle::Updating => {
                if envelope.phase == SessionPhase::RecoveryRequired {
                    // The recovery-required terminal was journaled while the
                    // receipt write may not have landed; the journal leads.
                    MutationRecoveryState::RecoveryRequired
                } else if envelope.phase != SessionPhase::HandedOff {
                    MutationRecoveryState::Inconsistent {
                        detail: format!(
                            "Updating receipt with session phase {:?}",
                            envelope.phase
                        ),
                    }
                } else if let Some(intent) = decisions.first() {
                    match intent.decision {
                        crate::probation::CommitDecision::Commit => MutationRecoveryState::CommitPending,
                        crate::probation::CommitDecision::Rollback => MutationRecoveryState::RollbackPending,
                    }
                } else if envelope.accepted_health.is_some() {
                    // Health accepted but the decision not yet journaled:
                    // the next legal step is the commit transaction.
                    MutationRecoveryState::CommitPending
                } else if !any_intent {
                    if envelope.probation_process.is_some() {
                        MutationRecoveryState::Inconsistent {
                            detail: "probation recorded without any replacement".to_string(),
                        }
                    } else {
                        MutationRecoveryState::UpdatingNoMutation
                    }
                } else if all_completed {
                    if envelope.probation_process.is_some() {
                        MutationRecoveryState::LaunchedAwaitingHealth
                    } else {
                        MutationRecoveryState::ReplacedAwaitingLaunch
                    }
                } else {
                    let backup_phase = facts
                        .iter()
                        .any(|fact| fact.intent_recorded && !fact.backup_recorded);
                    if backup_phase {
                        MutationRecoveryState::BackupPartial
                    } else {
                        MutationRecoveryState::ReplacePartial
                    }
                }
            }
            other => MutationRecoveryState::Inconsistent {
                detail: format!("receipt lifecycle is {other:?}"),
            },
        }
    };

    Ok(RecoveryClassification {
        session_id: session_id.to_string(),
        receipt_lifecycle: Some(receipt.lifecycle_state),
        session_phase: Some(envelope.phase),
        state,
        resources: facts,
    })
}

/// True when any resource still sits between its durable intent and a
/// completed, verified replacement — the mutation set is mid-flight.
fn any_pending_mutation(facts: &[ResourceRecoveryFact]) -> bool {
    facts
        .iter()
        .any(|fact| fact.intent_recorded && !fact.replace_completed)
}

/// The `--recover` entry: classify and report, read-only. This phase never
/// resumes, rolls back or cleans up — the classification output is the
/// recovery phase's input.
pub fn recover_report(paths: &Paths, session_id: &str) -> Result<String, Error> {
    let trust = desktop_todo_update_core::production_trust_store();
    let classification = classify_update_recovery(paths, &trust, session_id).map_err(Error::from)?;
    Ok(classification.report_text())
}

// --------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{validate_update_handoff, HandoffError, LiveProcessIdentity};
    use crate::paths::Paths;
    use crate::update_session::{publish_update_session, HandoffFacts, ProcessIdentity, SessionPhase};
    use crate::MAIN_EXE;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use desktop_todo_update_core::{derive_key_id, TrustStore};
    use ed25519_dalek::{Signer, SigningKey};

    /// TEST-ONLY seed. K1 is the fixture trust root; never a production
    /// trust root, never used for release signing.
    const TEST_SEED: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";

    fn test_trust() -> TrustStore {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap()
    }

    fn payload(seed: u8) -> Vec<u8> {
        (0..64u32).map(|i| (i as u8).wrapping_add(seed)).collect()
    }

    fn sha_of(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    fn own_creation_filetime() -> u64 {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        unsafe {
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
            .unwrap();
            ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64
        }
    }

    struct Fixture {
        paths: Paths,
        trust: TrustStore,
        session_id: String,
        digest: String,
        /// Live caller identity matching the recorded parent process (our
        /// own PID + real creation time + the sandbox main executable image).
        live: LiveProcessIdentity,
        main_old: Vec<u8>,
        helper_old: Vec<u8>,
        main_new: Vec<u8>,
        helper_new: Vec<u8>,
    }

    impl Fixture {
        fn source(&self) -> Version {
            Version::parse("1.1.0").unwrap()
        }
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
        fn begin(&self) -> Result<(), MutationError> {
            begin_update_transaction(
                &self.paths,
                &self.trust,
                // Production derives this from current_exe; the sandbox test
                // runs from the test binary, so the installed-helper image is
                // injected (the comparison semantics are what is tested).
                &self.paths.install().join(HELPER_EXE),
                &self.live,
                &self.source(),
                &self.session_id,
                &self.digest,
            )
        }
        fn continue_apply(&self) -> Result<(ValidatedHandoff, crate::lock::AppLease), MutationError> {
            let runner_dir = self.paths.state().join("runners").join("test-runner");
            paths::create_dir(&runner_dir).unwrap();
            continue_update_transaction(
                &self.paths,
                &self.trust,
                &runner_dir,
                &self.session_id,
                &self.digest,
            )
        }
        fn classify(&self) -> RecoveryClassification {
            classify_update_recovery(&self.paths, &self.trust, &self.session_id).unwrap()
        }
        fn receipt(&self) -> Receipt {
            Receipt::load(&self.paths).unwrap().unwrap()
        }
    }

    /// A complete transaction fixture: managed install with a validated
    /// receipt, a persisted signed session (target 1.2.0) with staged new
    /// executables, and a live-caller identity bound to this test process.
    fn tx_fixture(tag: &str) -> Fixture {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        let trust = test_trust();
        let id = uuid::Uuid::new_v4().to_string();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("apply-tests")
            .join(format!("{tag}-{id}"));
        let paths = Paths::sandbox(root, &id);
        paths::create_dir(paths.install()).unwrap();
        paths::create_dir(paths.state()).unwrap();

        let main_old = payload(3);
        let helper_old = payload(4);
        let main_new = payload(1);
        let helper_new = payload(2);

        // Installed managed runtime + support + unknown files. The support
        // and unknown files must survive the transaction untouched.
        std::fs::write(paths.install().join(MAIN_EXE), &main_old).unwrap();
        std::fs::write(paths.install().join(HELPER_EXE), &helper_old).unwrap();
        std::fs::write(paths.install().join("README.md"), b"old readme").unwrap();
        std::fs::write(paths.install().join("unknown.dll"), b"foreign").unwrap();

        let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
            .into_iter()
            .map(|identity| {
                let bytes = match identity {
                    Resource::MainExecutable => &main_old,
                    _ => &helper_old,
                };
                crate::receipt::FileRecord {
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

        // Signed package + persisted session contents (target 1.2.0).
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
        let target = desktop_todo_update_core::verify_and_parse(&trust, &envelope_bytes, &manifest_bytes)
            .expect("fixture target verifies");

        let session_id = uuid::Uuid::new_v4().to_string();
        let dir = paths.state().join("updates").join("sessions").join(&session_id);
        std::fs::create_dir_all(dir.join("staged")).unwrap();
        std::fs::write(dir.join(crate::handoff::MANIFEST_FILE), &manifest_bytes).unwrap();
        std::fs::write(dir.join(crate::handoff::ENVELOPE_FILE), &envelope_bytes).unwrap();
        std::fs::write(dir.join(crate::handoff::PACKAGE_FILE), &package).unwrap();
        std::fs::write(dir.join("staged/desktop-todo-widget.exe"), &main_new).unwrap();
        std::fs::write(dir.join("staged/desktop-todo-maintenance.exe"), &helper_new).unwrap();

        let live = LiveProcessIdentity {
            pid: std::process::id(),
            creation_filetime: own_creation_filetime(),
            image_path: paths.install().join(MAIN_EXE),
        };
        publish_update_session(
            &target,
            HandoffFacts {
                session_id: &session_id,
                installation_id: &receipt.installation_id,
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
        // The Initial validation (2C-B compat entry) — also journals the
        // verified digest, exactly like the production prelude.
        validate_update_handoff(
            &paths,
            &trust,
            &session_id,
            &digest,
            &Version::parse("1.1.0").unwrap(),
        )
        .expect("fixture handoff validates");

        Fixture {
            paths,
            trust,
            session_id,
            digest,
            live,
            main_old,
            helper_old,
            main_new,
            helper_new,
        }
    }

    fn journal_resource(fixture: &Fixture, identity: &str) -> crate::update_session::SessionResource {
        fixture
            .envelope()
            .resources
            .into_iter()
            .find(|entry| entry.identity == identity)
            .unwrap()
    }

    fn validated(fixture: &Fixture) -> ValidatedHandoff {
        crate::handoff::validate_resume(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap()
    }

    fn journal_replace_facts(
        fixture: &Fixture,
        identity: &str,
        old: (u64, String),
        completed: bool,
    ) {
        publish_mutation(&validated(fixture), |envelope| {
            if let Some(entry) = envelope
                .resources
                .iter_mut()
                .find(|entry| entry.identity == identity)
            {
                entry.intent = Some(ResourceAction::Replace);
                entry.old_present = true;
                entry.old_size = Some(old.0);
                entry.old_sha256 = Some(old.1);
                if completed {
                    entry.completed = Some(true);
                }
            }
        })
        .unwrap();
    }

    fn set_installed(fixture: &Fixture, main_bytes: &[u8], helper_bytes: &[u8]) {
        std::fs::write(fixture.paths.install().join(MAIN_EXE), main_bytes).unwrap();
        std::fs::write(fixture.paths.install().join(HELPER_EXE), helper_bytes).unwrap();
    }

    // ------------------------------------------------------------- begin half

    #[test]
    fn begin_journals_intent_then_transitions_receipt() {
        let fixture = tx_fixture("begin-happy");
        fixture.begin().expect("begin succeeds");

        // Journal leads: the session is handed-off with the previous-state
        // snapshots, durably.
        let envelope = fixture.envelope();
        assert_eq!(envelope.phase, SessionPhase::HandedOff);
        let previous = envelope.previous_receipt.expect("previous receipt captured");
        assert_eq!(previous.lifecycle_state, Lifecycle::Installed);
        assert_eq!(previous.current_version.as_deref(), Some("1.1.0"));
        assert!(envelope.previous_integration.is_some());
        assert!(envelope.generation >= 2);

        // Receipt follows: Updating + active session, currentVersion still
        // the last committed version (Updating is not success).
        let receipt = fixture.receipt();
        assert_eq!(receipt.lifecycle_state, Lifecycle::Updating);
        assert_eq!(
            receipt.maintenance.active_session_id.as_deref(),
            Some(fixture.session_id.as_str())
        );
        assert_eq!(receipt.current_version.as_deref(), Some("1.1.0"));
    }

    #[test]
    fn begin_refuses_non_helper_image_and_identity_mismatches() {
        let fixture = tx_fixture("begin-image");
        // Wrong running image: not the installed helper.
        let error = begin_update_transaction(
            &fixture.paths,
            &fixture.trust,
            &fixture.paths.state().join("elsewhere.exe"),
            &fixture.live,
            &fixture.source(),
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap_err();
        assert!(matches!(error, MutationError::HelperImageMismatch { .. }), "{error}");
        assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Installed);

        // PID-reuse simulation: same live PID, different creation time.
        let reused = LiveProcessIdentity {
            creation_filetime: fixture.live.creation_filetime + 1,
            ..fixture.live.clone()
        };
        let error = begin_update_transaction(
            &fixture.paths,
            &fixture.trust,
            &fixture.paths.install().join(HELPER_EXE),
            &reused,
            &fixture.source(),
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap_err();
        assert!(matches!(error, MutationError::CallerIdentityMismatch { .. }), "{error}");

        // Wrong live image.
        let wrong_image = LiveProcessIdentity {
            image_path: fixture.paths.state().join("not-the-main.exe"),
            ..fixture.live.clone()
        };
        let error = begin_update_transaction(
            &fixture.paths,
            &fixture.trust,
            &fixture.paths.install().join(HELPER_EXE),
            &wrong_image,
            &fixture.source(),
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap_err();
        assert!(matches!(error, MutationError::CallerIdentityMismatch { .. }), "{error}");

        // Wrong recorded PID in the durable session.
        let fixture = tx_fixture("begin-pid");
        let mut session_value = serde_json::to_value(fixture.envelope()).unwrap();
        session_value["parentProcess"]["pid"] = serde_json::Value::from(424242);
        std::fs::write(
            fixture.session_dir().join(SESSION_FILE),
            serde_json::to_vec_pretty(&session_value).unwrap(),
        )
        .unwrap();
        let error = begin_update_transaction(
            &fixture.paths,
            &fixture.trust,
            &fixture.paths.install().join(HELPER_EXE),
            &fixture.live,
            &fixture.source(),
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap_err();
        assert!(matches!(error, MutationError::CallerIdentityMismatch { .. }), "{error}");
    }

    #[test]
    fn query_detects_exited_and_live_processes() {
        // Live: our own process is queryable and self-consistent.
        let own = query_live_process_identity(std::process::id()).expect("own process queryable");
        assert_eq!(own.pid, std::process::id());
        assert_eq!(own.image_path, std::env::current_exe().unwrap());

        // Exited: a child that terminated before the query is CallerExited —
        // the exact mechanism PID reuse cannot fool (no creation time to
        // match, no image to match).
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--zzz-no-such-libtest-flag")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("probe child spawns");
        let pid = child.id();
        let status = child.wait().expect("probe child exits");
        assert!(!status.success());
        let error = query_live_process_identity(pid).unwrap_err();
        assert!(matches!(error, HandoffError::CallerExited { .. }), "{error}");
    }

    #[test]
    fn begin_refuses_stale_source_without_touching_anything() {
        let fixture = tx_fixture("begin-stale");
        let error = begin_update_transaction(
            &fixture.paths,
            &fixture.trust,
            &fixture.paths.install().join(HELPER_EXE),
            &fixture.live,
            &Version::parse("1.0.9").unwrap(),
            &fixture.session_id,
            &fixture.digest,
        )
        .unwrap_err();
        assert!(matches!(error, MutationError::SourceVersionChanged { .. }), "{error}");
        // Nothing moved: receipt still Installed, session still staged.
        assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Installed);
        assert_eq!(fixture.envelope().phase, SessionPhase::Staged);
    }

    // ---------------------------------------------------------- continue half

    #[test]
    fn continue_requires_the_updating_receipt() {
        let fixture = tx_fixture("continue-gate");
        let error = fixture.continue_apply().unwrap_err();
        // The resume entry refuses a session that never left the staged
        // phase — the authority transfer never happened.
        assert!(matches!(error, MutationError::Validation(_)), "{error}");
    }

    #[test]
    fn full_apply_replaces_both_executables_and_stops_before_launch() {
        let fixture = tx_fixture("apply-happy");
        fixture.begin().unwrap();
        fixture.continue_apply().expect("apply completes");

        // Both installed executables now hold the signed target bytes.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
        assert_eq!(
            std::fs::read(fixture.paths.install().join(HELPER_EXE)).unwrap(),
            fixture.helper_new
        );

        // Backup slots hold the verified preimages.
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        assert_eq!(
            std::fs::read(workspace.join("mainExecutable.backup")).unwrap(),
            fixture.main_old
        );
        assert_eq!(
            std::fs::read(workspace.join("maintenanceHelper.backup")).unwrap(),
            fixture.helper_old
        );

        // Journal: intent, old facts, completion for both; probation record.
        let envelope = fixture.envelope();
        assert_eq!(envelope.phase, SessionPhase::HandedOff);
        for resource in &envelope.resources {
            assert_eq!(resource.intent, Some(ResourceAction::Replace));
            assert!(resource.old_present);
            assert_eq!(resource.completed, Some(true));
        }
        let main = journal_resource(&fixture, "mainExecutable");
        assert_eq!(main.old_sha256.as_deref(), Some(sha_of(&fixture.main_old).as_str()));
        let helper = journal_resource(&fixture, "maintenanceHelper");
        assert_eq!(helper.old_sha256.as_deref(), Some(sha_of(&fixture.helper_old).as_str()));
        // Success endpoint: no probation record exists — the launch belongs
        // to the HealthAck phase (admission would refuse a non-probation
        // child by design, so none is performed).
        assert!(envelope.probation_process.is_none());
        // Generations advanced monotonically with every journal write.
        assert!(envelope.generation >= 8);

        // The receipt stays Updating at the SOURCE version — nothing is
        // committed, and no probation/commit/rollback fields exist.
        let receipt = fixture.receipt();
        assert_eq!(receipt.lifecycle_state, Lifecycle::Updating);
        assert_eq!(receipt.current_version.as_deref(), Some("1.1.0"));
        assert!(envelope.health_nonce.is_none());
        assert!(envelope.accepted_health.is_none());
        assert!(envelope.commit_intent.is_none());

        // Support files, unknown files are untouched; backups are retained
        // (cleanup belongs to the commit phase).
        assert_eq!(
            std::fs::read(fixture.paths.install().join("README.md")).unwrap(),
            b"old readme"
        );
        assert!(fixture.paths.install().join("unknown.dll").is_file());
        assert!(workspace.join("mainExecutable.backup").is_file());

        // Classification: replaced, awaiting the HealthAck phase's launch.
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::ReplacedAwaitingLaunch
        );
    }

    #[test]
    fn apply_resumes_idempotently_after_crash_like_restart() {
        let fixture = tx_fixture("apply-resume");
        fixture.begin().unwrap();
        fixture.continue_apply().unwrap();
        // A restart re-validates everything from disk and re-runs: every
        // step is a no-op verified against the durable facts.
        fixture.continue_apply().expect("resume is idempotent");
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::ReplacedAwaitingLaunch
        );
    }

    #[test]
    fn locked_staged_source_refuses_before_any_mutation() {
        use std::os::windows::fs::OpenOptionsExt;
        let fixture = tx_fixture("apply-locked");
        fixture.begin().unwrap();
        let staged_main = fixture
            .session_dir()
            .join("staged")
            .join("desktop-todo-widget.exe");
        let conflict = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&staged_main)
            .unwrap();
        let error = fixture.continue_apply().unwrap_err();
        drop(conflict);
        // A precise, typed refusal from the independent revalidation (the
        // locked staged source cannot be re-verified) — never a fake
        // success.
        assert!(matches!(error, MutationError::Validation(_)), "{error}");
        // Nothing was replaced and nothing was journaled beyond the handoff.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_old
        );
        assert!(journal_resource(&fixture, "mainExecutable").intent.is_none());
        assert_eq!(fixture.envelope().phase, SessionPhase::HandedOff);
    }

    #[test]
    fn reparse_substituted_destination_refused_before_mutation() {
        let fixture = tx_fixture("apply-reparse");
        fixture.begin().unwrap();
        // A junction standing in for the installed main executable.
        let victim = fixture.paths.install().join(MAIN_EXE);
        std::fs::remove_file(&victim).unwrap();
        let target_dir = fixture.paths.install().join("elsewhere");
        std::fs::create_dir_all(&target_dir).unwrap();
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&victim)
            .arg(&target_dir)
            .status()
            .expect("mklink");
        assert!(status.success(), "junction creation failed");
        let error = fixture.continue_apply().unwrap_err();
        assert!(matches!(error, MutationError::UnsafePath { .. }), "{error}");
        // The helper resource was never touched.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(HELPER_EXE)).unwrap(),
            fixture.helper_old
        );
        assert!(!session_workspace(&fixture.paths, &fixture.session_id)
            .join("maintenanceHelper.backup")
            .exists());
    }

    #[test]
    fn hardlinked_installed_runtime_refused() {
        let fixture = tx_fixture("apply-hardlink");
        fixture.begin().unwrap();
        std::fs::hard_link(
            fixture.paths.install().join(MAIN_EXE),
            fixture.paths.install().join("alias.exe"),
        )
        .unwrap();
        let error = fixture.continue_apply().unwrap_err();
        assert!(matches!(error, MutationError::UnsafePath { .. }), "{error}");
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_old
        );
    }

    #[test]
    fn lease_busy_surfaces_after_bounded_wait() {
        let fixture = tx_fixture("lease-busy");
        fixture.begin().unwrap();
        // A live app instance holds the shared lease until exit (taken via
        // the lease primitive without admission — the receipt is Updating
        // during the transaction by design).
        let app = crate::lock::shared_app(&fixture.paths).unwrap();
        let error = match wait_exclusive_lease(&fixture.paths, 2, Duration::from_millis(1)) {
            Err(error) => error,
            Ok(_) => panic!("lease should be busy while the app instance runs"),
        };
        assert!(matches!(error, MutationError::LeaseBusy { .. }), "{error}");
        drop(app);
        // Crash-release semantics: a dropped lease is immediately available.
        assert!(wait_exclusive_lease(&fixture.paths, 2, Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn runner_copy_and_handshake_behave() {
        let fixture = tx_fixture("runner-stage");
        // Stage: the copy is hash-verified and its digest recorded.
        let (runner_exe, marker) = stage_runner_copy(&fixture.paths).expect("runner staged");
        assert!(runner_exe.is_file());
        let expected = std::fs::read_to_string(runner_exe.parent().unwrap().join("runner.sha256"))
            .unwrap();
        let (_, actual) = fingerprint(&runner_exe).unwrap();
        assert_eq!(actual, expected.trim());

        // Provenance from an ordinary image (the test binary) is none.
        assert!(detect_runner_provenance(&fixture.paths).unwrap().is_none());

        // Handshake: a runner that signals ready satisfies the wait; a
        // silent runner fails after the bounded attempts.
        let marker_path = marker.clone();
        let result = hand_off_to_runner(
            &runner_exe,
            &marker,
            &fixture.session_id,
            &fixture.digest,
            2,
            &move |_: &Path, _: &[String]| {
                std::fs::write(&marker_path, b"{\"pid\":1}").unwrap();
                Ok(())
            },
        );
        assert!(result.is_ok());
        // The silent-runner timeout needs a fresh, absent marker.
        std::fs::remove_file(&marker).unwrap();
        let result = hand_off_to_runner(
            &runner_exe,
            &marker,
            &fixture.session_id,
            &fixture.digest,
            1,
            &|_: &Path, _: &[String]| Ok(()),
        );
        assert!(matches!(result.unwrap_err(), MutationError::RunnerNotReady));
    }

    // ------------------------------------------------ crash-window classification

    #[test]
    fn crash_windows_classify_without_guessing() {
        // A. Updating transition done, crash before any backup.
        let fixture = tx_fixture("crash-a");
        fixture.begin().unwrap();
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::UpdatingNoMutation
        );

        // B. Main intent journaled, crash before the backup.
        let fixture = tx_fixture("crash-b");
        fixture.begin().unwrap();
        publish_mutation(&validated(&fixture), |envelope| {
            envelope.resources[0].intent = Some(ResourceAction::Replace);
        })
        .unwrap();
        let classified = fixture.classify();
        assert_eq!(classified.state, MutationRecoveryState::BackupPartial);
        assert!(classified.resources[0].intent_recorded);
        assert!(!classified.resources[0].backup_recorded);

        // C. Backup copied and verified, crash before the old-fact journal
        // write: the ambiguous slot is observable evidence, and the
        // idempotent resume completes the journal from the verified slot.
        let fixture = tx_fixture("crash-c");
        fixture.begin().unwrap();
        publish_mutation(&validated(&fixture), |envelope| {
            envelope.resources[0].intent = Some(ResourceAction::Replace);
        })
        .unwrap();
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("mainExecutable.backup"), &fixture.main_old).unwrap();
        let classified = fixture.classify();
        assert_eq!(classified.state, MutationRecoveryState::BackupPartial);
        assert!(classified.resources[0].backup_slot_present);
        assert_eq!(classified.resources[0].backup_slot_matches_old, Some(true));
        fixture.continue_apply().unwrap();
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );

        // D. Main replaced on disk, crash before the completion journal.
        let fixture = tx_fixture("crash-d");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            false,
        );
        set_installed(&fixture, &fixture.main_new, &fixture.helper_old);
        let classified = fixture.classify();
        assert_eq!(classified.state, MutationRecoveryState::ReplacePartial);
        assert_eq!(classified.resources[0].installed_matches_new, Some(true));

        // E. Main done, helper untouched.
        let fixture = tx_fixture("crash-e");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            true,
        );
        journal_replace_facts(
            &fixture,
            "maintenanceHelper",
            (fixture.helper_old.len() as u64, sha_of(&fixture.helper_old)),
            false,
        );
        set_installed(&fixture, &fixture.main_new, &fixture.helper_old);
        assert_eq!(fixture.classify().state, MutationRecoveryState::ReplacePartial);

        // F. Helper backup recorded, crash before the helper replacement.
        let fixture = tx_fixture("crash-f");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            true,
        );
        journal_replace_facts(
            &fixture,
            "maintenanceHelper",
            (fixture.helper_old.len() as u64, sha_of(&fixture.helper_old)),
            false,
        );
        set_installed(&fixture, &fixture.main_new, &fixture.helper_old);
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        std::fs::write(workspace.join("maintenanceHelper.backup"), &fixture.helper_old).unwrap();
        let classified = fixture.classify();
        assert_eq!(classified.state, MutationRecoveryState::ReplacePartial);
        assert!(classified.resources[1].backup_recorded);

        // G. Both replaced, crash before the launch.
        let fixture = tx_fixture("crash-g");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            true,
        );
        journal_replace_facts(
            &fixture,
            "maintenanceHelper",
            (fixture.helper_old.len() as u64, sha_of(&fixture.helper_old)),
            true,
        );
        set_installed(&fixture, &fixture.main_new, &fixture.helper_new);
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::ReplacedAwaitingLaunch
        );

        // H. Launched and journaled (2D-B writes the nonce first, then the
        // launch record), crash before any health decision. The journal
        // writes reuse the one validated handoff — a re-validation after
        // the nonce exists is deliberately refused, so the test mirrors the
        // production single-handoff write pattern.
        let fixture = tx_fixture("crash-h");
        fixture.begin().unwrap();
        fixture.continue_apply().unwrap();
        let handoff = validated(&fixture);
        publish_mutation(&handoff, |envelope| {
            envelope.health_nonce = Some(STANDARD.encode([7u8; 32]));
        })
        .unwrap();
        publish_mutation(&handoff, |envelope| {
            envelope.probation_process = Some(ProcessIdentity {
                pid: 4242,
                process_created_at: "133000000000000000".to_string(),
                image_path: fixture.paths.install().join(MAIN_EXE),
            });
        })
        .unwrap();
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::LaunchedAwaitingHealth
        );

        // H-illegal (2D-B ordering invariant): a launch record without any
        // journaled nonce cannot exist — the nonce is minted and durable
        // before the launch, so this shape is evidence tampering and the
        // classifier refuses it.
        let fixture = tx_fixture("crash-h-illegal");
        fixture.begin().unwrap();
        fixture.continue_apply().unwrap();
        let handoff = validated(&fixture);
        publish_mutation(&handoff, |envelope| {
            envelope.probation_process = Some(ProcessIdentity {
                pid: 4242,
                process_created_at: "133000000000000000".to_string(),
                image_path: fixture.paths.install().join(MAIN_EXE),
            });
        })
        .unwrap();
        assert!(matches!(
            fixture.classify().state,
            MutationRecoveryState::Inconsistent { .. }
        ));
    }

    #[test]
    fn classification_refuses_inconsistent_and_absent_states() {
        // No session at all: nothing to recover.
        let fixture = tx_fixture("classify-none");
        std::fs::remove_dir_all(fixture.session_dir()).unwrap();
        assert_eq!(fixture.classify().state, MutationRecoveryState::NoActiveUpdate);

        // Updating receipt with a missing journal: RecoveryRequired
        // semantics, never a guessed action.
        let fixture = tx_fixture("classify-journal-lost");
        fixture.begin().unwrap();
        std::fs::remove_dir_all(fixture.session_dir()).unwrap();
        assert!(matches!(
            fixture.classify().state,
            MutationRecoveryState::Inconsistent { .. }
        ));

        // A completed fact whose file no longer holds the signed bytes.
        let fixture = tx_fixture("classify-lie");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            true,
        );
        journal_replace_facts(
            &fixture,
            "maintenanceHelper",
            (fixture.helper_old.len() as u64, sha_of(&fixture.helper_old)),
            true,
        );
        // Files still hold the OLD bytes — the completed facts lie.
        assert!(matches!(
            fixture.classify().state,
            MutationRecoveryState::Inconsistent { .. }
        ));

        // Handoff intent recorded but the receipt write lost: authority was
        // never transferred; classification says HandoffPrepared and the
        // Initial entry can redo the transition.
        let fixture = tx_fixture("classify-intent-only");
        fixture.begin().unwrap();
        let mut receipt = fixture.receipt();
        receipt.lifecycle_state = Lifecycle::Installed;
        receipt.maintenance.active_session_id = None;
        receipt.save(&fixture.paths).unwrap();
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::HandoffPrepared
        );
        fixture.begin().expect("resume after intent-only crash");
        assert_eq!(fixture.receipt().lifecycle_state, Lifecycle::Updating);
    }

    #[test]
    fn begin_refuses_mutation_facts_without_authority_transfer() {
        // The impossible-by-ordering state (mutation facts under an
        // Installed receipt) cannot be validated through the Initial entry.
        let fixture = tx_fixture("begin-impossible");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            false,
        );
        let mut receipt = fixture.receipt();
        receipt.lifecycle_state = Lifecycle::Installed;
        receipt.maintenance.active_session_id = None;
        receipt.save(&fixture.paths).unwrap();
        let error = fixture.begin().unwrap_err();
        assert!(
            matches!(
                error,
                MutationError::Validation(HandoffError::InvalidSessionState { .. })
            ),
            "{error}"
        );
    }

    // --------------------------------------------- crash-D reconciliation

    /// Crash-D (audit point 2): the replacement side effect landed, the
    /// completion journal write did not. Resume must fresh-verify the
    /// destination against the signed TARGET, verify the rollback slot
    /// against the journaled OLD preimage, reconcile the completion, and
    /// preserve the slot byte-identically — never treating TARGET as the
    /// preimage, never overwriting the rollback asset. Covered for BOTH
    /// managed EXEs.
    #[test]
    fn crash_d_reconciles_main_replacement_and_preserves_backup() {
        let fixture = tx_fixture("crash-d-reconcile");
        fixture.begin().unwrap();
        // Journal: main intent + old facts (backup phase durable); the
        // backup slot holds OLD; the destination already holds TARGET; no
        // completion journal.
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            false,
        );
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        let main_slot = workspace.join("mainExecutable.backup");
        std::fs::write(&main_slot, &fixture.main_old).unwrap();
        std::fs::write(fixture.paths.install().join(MAIN_EXE), &fixture.main_new).unwrap();
        // Sanity: the durable journal really lacks the completion.
        assert_eq!(journal_resource(&fixture, "mainExecutable").completed, None);

        fixture.continue_apply().expect("resume reconciles the crash-D state");

        // Destination fresh-verified: TARGET.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
        // Rollback asset preserved byte-identically — never deleted, never
        // rewritten from the target bytes.
        assert_eq!(std::fs::read(&main_slot).unwrap(), fixture.main_old);
        // Journal reconciled: completion durable for both resources.
        assert_eq!(journal_resource(&fixture, "mainExecutable").completed, Some(true));
        assert_eq!(
            journal_resource(&fixture, "maintenanceHelper").completed,
            Some(true)
        );
        assert_eq!(
            std::fs::read(fixture.paths.install().join(HELPER_EXE)).unwrap(),
            fixture.helper_new
        );
        assert_eq!(
            std::fs::read(workspace.join("maintenanceHelper.backup")).unwrap(),
            fixture.helper_old
        );
        // Idempotent: a further resume stays consistent.
        fixture.continue_apply().expect("second resume is a no-op");
        assert_eq!(std::fs::read(&main_slot).unwrap(), fixture.main_old);
    }

    /// Audit point 4: the helper EXE alone in the crash-D shape (the main
    /// EXE already completed and journaled). The runner is the executing
    /// process, so its own installed image is the resource under
    /// reconciliation — the rollback asset must survive untouched.
    #[test]
    fn helper_rollback_asset_preserved_through_reconcile() {
        let fixture = tx_fixture("helper-reconcile");
        fixture.begin().unwrap();
        // Main: fully completed and journaled.
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            true,
        );
        std::fs::write(fixture.paths.install().join(MAIN_EXE), &fixture.main_new).unwrap();
        // Helper: intent + old facts journaled, slot holds OLD, destination
        // already holds TARGET, completion journal missing.
        journal_replace_facts(
            &fixture,
            "maintenanceHelper",
            (fixture.helper_old.len() as u64, sha_of(&fixture.helper_old)),
            false,
        );
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        let helper_slot = workspace.join("maintenanceHelper.backup");
        std::fs::write(&helper_slot, &fixture.helper_old).unwrap();
        std::fs::write(fixture.paths.install().join(HELPER_EXE), &fixture.helper_new).unwrap();

        fixture.continue_apply().expect("helper reconcile succeeds");

        // Fresh verify: destination TARGET.
        assert_eq!(
            std::fs::read(fixture.paths.install().join(HELPER_EXE)).unwrap(),
            fixture.helper_new
        );
        // The helper's rollback asset is preserved byte-identically.
        assert_eq!(std::fs::read(&helper_slot).unwrap(), fixture.helper_old);
        assert_eq!(
            journal_resource(&fixture, "maintenanceHelper").completed,
            Some(true)
        );
        // The main EXE's facts and backup are untouched by the reconcile.
        assert_eq!(
            journal_resource(&fixture, "mainExecutable").completed,
            Some(true)
        );
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
        assert_eq!(
            fixture.classify().state,
            MutationRecoveryState::ReplacedAwaitingLaunch
        );
    }

    /// A destination holding TARGET with a rollback slot that does NOT hold
    /// the journaled preimage can never be reconciled: the old set is not
    /// provable, so the resume refuses and touches nothing.
    #[test]
    fn reconcile_refuses_when_rollback_asset_does_not_match() {
        let fixture = tx_fixture("reconcile-bad-slot");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            false,
        );
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        let main_slot = workspace.join("mainExecutable.backup");
        std::fs::write(&main_slot, &fixture.main_new).unwrap(); // wrong bytes
        std::fs::write(fixture.paths.install().join(MAIN_EXE), &fixture.main_new).unwrap();

        let error = fixture.continue_apply().unwrap_err();
        assert!(
            matches!(error, MutationError::RecoveryInconsistent { .. }),
            "{error}"
        );
        // Nothing reconciled, nothing deleted, nothing replaced.
        assert_eq!(journal_resource(&fixture, "mainExecutable").completed, None);
        assert_eq!(std::fs::read(&main_slot).unwrap(), fixture.main_new);
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
        assert_eq!(
            std::fs::read(fixture.paths.install().join(HELPER_EXE)).unwrap(),
            fixture.helper_old
        );
    }

    /// A destination holding TARGET with a missing rollback slot refuses
    /// the same way — the rollback asset may never be reconstructed from
    /// the target bytes.
    #[test]
    fn reconcile_refuses_when_rollback_asset_is_missing() {
        let fixture = tx_fixture("reconcile-no-slot");
        fixture.begin().unwrap();
        journal_replace_facts(
            &fixture,
            "mainExecutable",
            (fixture.main_old.len() as u64, sha_of(&fixture.main_old)),
            false,
        );
        let workspace = session_workspace(&fixture.paths, &fixture.session_id);
        paths::create_dir(&workspace).unwrap();
        std::fs::write(fixture.paths.install().join(MAIN_EXE), &fixture.main_new).unwrap();
        // No slot file at all.

        let error = fixture.continue_apply().unwrap_err();
        assert!(
            matches!(error, MutationError::RecoveryInconsistent { .. }),
            "{error}"
        );
        assert_eq!(journal_resource(&fixture, "mainExecutable").completed, None);
        assert_eq!(
            std::fs::read(fixture.paths.install().join(MAIN_EXE)).unwrap(),
            fixture.main_new
        );
    }

    #[test]
    fn resume_refuses_probation_fields_before_their_phase() {
        let fixture = tx_fixture("resume-early-fields");
        fixture.begin().unwrap();
        publish_mutation(&validated(&fixture), |envelope| {
            envelope.health_nonce = Some(STANDARD.encode([7u8; 32]));
        })
        .unwrap();
        let error = fixture.continue_apply().unwrap_err();
        assert!(
            matches!(
                error,
                MutationError::Validation(HandoffError::InvalidSessionState { .. })
            ),
            "{error}"
        );
    }
}
