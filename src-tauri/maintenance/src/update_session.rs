//! The frozen `UpdateSession` envelope (Maintenance protocol v1,
//! "UpdateSession and journal") — schema 1, closed, rejected on unknown
//! fields, duplicate keys and trailing input.
//!
//! The session describes what an update is trying to turn this installation
//! into; the receipt describes what is installed. They never merge, and the
//! session holds no resource authority: every binding it carries is
//! re-derived or recomputed before use (see [`crate::handoff`]), because the
//! session lives in user-writable space and an edited session can only make
//! the updater refuse, never widen it.
//!
//! Creation boundary (frozen): the envelope is published only from a
//! `VerifiedTarget` — the type produced exclusively by the shared
//! `update-core` verifier after the full eligibility predicate. Every
//! signed-derived field (target version, manifest digest, package facts,
//! install files) is derived from that target inside
//! [`publish_update_session`], never passed in by the caller, so no caller
//! — main app, test, or future code — can publish a session whose
//! signed-derived facts disagree with the verified bytes.
//!
//! Field names are the frozen protocol table verbatim (camelCase). Fields
//! that belong to later phases (`probationProcess`, `healthNonce`,
//! `previousReceipt`, `previousIntegration`, `acceptedHealth`,
//! `commitIntent`, `lastError`) are absent before their phase — the protocol
//! explicitly requires `acceptedHealth`/`commitIntent` to be absent before
//! validation, and the rest cannot exist before their phase produces their
//! facts. The typed sub-shapes of `acceptedHealth`/`commitIntent` are not
//! frozen anywhere; they are carried as opaque JSON values that handoff
//! validation rejects in every pre-handoff phase, and their shapes get
//! frozen together with the phases that first write them.

use std::path::{Path, PathBuf};

use desktop_todo_update_core::{VerifiedTarget, Version};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::receipt::{validate_uuid, Receipt};

/// Session file name inside the session directory.
pub const SESSION_FILE: &str = "session.json";
/// Receipt/session size bound (protocol v1, "Encoding and validation").
pub const SESSION_MAX_BYTES: usize = 256 * 1024;

/// The frozen session phases — exactly the durable milestone vocabulary of
/// protocol v1 ("Durable milestones"), plus the bounded pre-handoff `failed`
/// terminal. This phase creates and validates only `staged`; later phases
/// are accepted by the parser (so schema 1 never changes shape) and refused
/// by handoff validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionPhase {
    #[serde(rename = "staged")]
    Staged,
    #[serde(rename = "handed-off")]
    HandedOff,
    #[serde(rename = "committed")]
    Committed,
    #[serde(rename = "rolled-back")]
    RolledBack,
    #[serde(rename = "recovery-required")]
    RecoveryRequired,
    #[serde(rename = "failed")]
    Failed,
}

/// The frozen typed operations, matching the fixed CLI selectors
/// (`--update`, `--install`, `--uninstall`, `--recover`). The selected
/// operation must agree with the persisted session type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionOperation {
    #[serde(rename = "update")]
    Update,
    #[serde(rename = "install")]
    Install,
    #[serde(rename = "uninstall")]
    Uninstall,
    #[serde(rename = "recover")]
    Recover,
}

/// Process identity: PID, process creation time (Windows FILETIME as a
/// decimal string, mirroring the HealthAck form) and the exact image path.
/// PID alone is insufficient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProcessIdentity {
    pub pid: u32,
    pub process_created_at: String,
    pub image_path: PathBuf,
}

/// Discovery source plus actual download locator metadata. Never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceInfo {
    pub discovered_via: String,
    pub package_url: String,
}

/// The only runtime-mutation intent protocol 1 defines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceAction {
    #[serde(rename = "replace")]
    Replace,
}

/// One identity-indexed managed resource: old/new hash/size, old-present
/// flag, operation intent/completion and the fixed backup slot. The backup
/// slot is compiled-derived (`.maintenance/<UUID>/<identity>.<role>`);
/// no arbitrary relative destination is accepted. Old facts, intent and
/// completion are absent until their phase captures them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionResource {
    pub identity: String,
    pub new_sha256: String,
    pub new_size: u64,
    pub old_present: bool,
    pub backup_slot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<ResourceAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
}

/// Bounded structured error record. Never written before the terminal phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SessionErrorRecord {
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub win32_code: Option<u32>,
    pub message: String,
}

/// The frozen UpdateSession envelope (schema 1, closed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct UpdateSessionEnvelope {
    pub schema_version: u32,
    pub updater_protocol: u32,
    pub operation: SessionOperation,
    pub session_id: String,
    pub installation_id: String,
    pub app_id: String,
    /// Frozen previous (installed source) version.
    pub from_version: String,
    /// Frozen target version; equals the signed manifest.
    pub to_version: String,
    pub source: SourceInfo,
    /// SHA-256 over the exact raw signed manifest bytes — the
    /// `--expected-manifest-sha256` binding value.
    pub manifest_sha256: String,
    pub package_sha256: String,
    pub package_size: u64,
    pub phase: SessionPhase,
    /// Monotonically increasing write generation (write-new-generation →
    /// flush → atomic publish).
    pub generation: u64,
    /// Absolute diagnostic snapshots, validated against derived locations.
    pub install_root: PathBuf,
    pub staging_root: PathBuf,
    pub created_at: String,
    pub updated_at: String,
    pub parent_process: ProcessIdentity,
    pub resources: Vec<SessionResource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probation_process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_receipt: Option<Receipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_integration: Option<Vec<crate::receipt::IntegrationRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_health: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_intent: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<SessionErrorRecord>,
}

impl UpdateSessionEnvelope {
    pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;
    pub const SUPPORTED_UPDATER_PROTOCOL: u32 =
        desktop_todo_update_core::SUPPORTED_UPDATER_PROTOCOL;
}

/// Publishing failures. A conflict means an existing durable envelope
/// disagrees with the facts derived from the verified target — never
/// repaired, never overwritten with different facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    Io { detail: String },
    Malformed { detail: String },
    UnsupportedSchema { version: u32 },
    /// An existing envelope disagrees with the verified target or the
    /// re-derived locations.
    Conflict { field: String },
    /// A caller-supplied non-derived fact failed structural validation.
    InvalidArgument { detail: String },
    /// The session directory passes through a reparse point.
    ReparsePoint { path: PathBuf },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { detail } => write!(f, "session write failure: {detail}"),
            Self::Malformed { detail } => write!(f, "session malformed: {detail}"),
            Self::UnsupportedSchema { version } => {
                write!(f, "session schema version {version} not supported")
            }
            Self::Conflict { field } => write!(
                f,
                "durable session field {field} disagrees with the verified target; \
                 the session cannot be repointed"
            ),
            Self::InvalidArgument { detail } => write!(f, "invalid session fact: {detail}"),
            Self::ReparsePoint { path } => {
                write!(f, "session path passes through a reparse point: {}", path.display())
            }
        }
    }
}

/// The caller-supplied, non-signed-derived facts of a handoff preparation.
/// Everything signed-derived is derived from `target` inside
/// [`publish_update_session`].
pub struct HandoffFacts<'a> {
    /// Canonical UUID of the already-persisted session directory.
    pub session_id: &'a str,
    /// The installation's UUID from the validated receipt.
    pub installation_id: &'a str,
    /// The re-bound actual installed source version.
    pub from_version: &'a str,
    pub discovered_via: &'a str,
    pub package_url: &'a str,
    /// Canonical install root snapshot (re-derived by the helper).
    pub install_root: &'a Path,
    /// The canonical updates root (`<maintenance state>/updates`); the
    /// staging root is derived from it plus the session id.
    pub updates_root: &'a Path,
    pub parent_process: ProcessIdentity,
}

/// The fixed backup slot grammar: `.maintenance/<UUID>/<identity>.<role>`.
pub fn backup_slot(session_id: &str, identity: &str) -> String {
    format!(r".maintenance\{session_id}\{identity}.backup")
}

fn session_dir(updates_root: &Path, session_id: &str) -> PathBuf {
    updates_root.join("sessions").join(session_id)
}

fn is_reparse(path: &Path) -> Result<bool, SessionError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            use std::os::windows::fs::MetadataExt;
            Ok(metadata.file_attributes() & 0x0400 != 0)
        }
        Err(_) => Ok(false),
    }
}

fn now_utc() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Build the envelope from a verified target plus the caller's non-derived
/// facts. Every signed-derived field comes from `target`.
fn build_envelope(
    target: &VerifiedTarget,
    facts: &HandoffFacts<'_>,
    created_at: String,
    updated_at: String,
    generation: u64,
) -> Result<UpdateSessionEnvelope, SessionError> {
    validate_uuid(facts.session_id)
        .map_err(|e| SessionError::InvalidArgument { detail: e.to_string() })?;
    validate_uuid(facts.installation_id)
        .map_err(|e| SessionError::InvalidArgument { detail: e.to_string() })?;
    Version::parse(facts.from_version).map_err(|e| SessionError::InvalidArgument {
        detail: format!("fromVersion {:?} is not a canonical version: {e}", facts.from_version),
    })?;
    if facts.discovered_via.len() > 64 || facts.package_url.len() > 2048 {
        return Err(SessionError::InvalidArgument {
            detail: "source metadata exceeds its bound".to_string(),
        });
    }
    paths::validate_path_syntax(facts.install_root).map_err(|e| SessionError::InvalidArgument {
        detail: format!("install root snapshot rejected: {e}"),
    })?;

    let asset = target.manifest().asset();
    let session_id = facts.session_id.to_string();
    let mut resources = Vec::new();
    for entry in asset.install_files() {
        let identity = entry.identity().to_string();
        let old_present = facts
            .install_root
            .join(desktop_todo_update_core::identity_filename(
                desktop_todo_update_core::InstallIdentity::from_manifest_text(&identity)
                    .ok_or_else(|| SessionError::InvalidArgument {
                        detail: format!("installFiles identity {identity:?} is not compiled"),
                    })?,
            ))
            .exists();
        resources.push(SessionResource {
            new_sha256: entry.sha256_hex().to_string(),
            new_size: entry.size(),
            identity: identity.clone(),
            old_present,
            backup_slot: backup_slot(&session_id, &identity),
            old_sha256: None,
            old_size: None,
            intent: None,
            completed: None,
        });
    }

    Ok(UpdateSessionEnvelope {
        schema_version: UpdateSessionEnvelope::SUPPORTED_SCHEMA_VERSION,
        updater_protocol: UpdateSessionEnvelope::SUPPORTED_UPDATER_PROTOCOL,
        operation: SessionOperation::Update,
        phase: SessionPhase::Staged,
        to_version: target.manifest().version().to_string(),
        manifest_sha256: target.manifest_sha256_hex().to_string(),
        package_sha256: asset.sha256_hex().to_string(),
        package_size: asset.size(),
        source: SourceInfo {
            discovered_via: facts.discovered_via.to_string(),
            package_url: facts.package_url.to_string(),
        },
        from_version: facts.from_version.to_string(),
        app_id: desktop_todo_update_core::APP_ID.to_string(),
        session_id: session_id.clone(),
        installation_id: facts.installation_id.to_string(),
        install_root: facts.install_root.to_path_buf(),
        staging_root: session_dir(facts.updates_root, &session_id),
        created_at,
        updated_at,
        generation,
        parent_process: facts.parent_process.clone(),
        resources,
        probation_process: None,
        health_nonce: None,
        previous_receipt: None,
        previous_integration: None,
        accepted_health: None,
        commit_intent: None,
        last_error: None,
    })
}

/// Fields whose disagreement makes a re-publish a conflict instead of a
/// resume. Phase, generation, timestamps and the parent process identity are
/// legitimately rewritten by a resumed preparation; every frozen fact is not.
fn frozen_facts_agree(existing: &UpdateSessionEnvelope, fresh: &UpdateSessionEnvelope) -> bool {
    let comparable = |e: &UpdateSessionEnvelope| {
        serde_json::to_value(UpdateSessionEnvelope {
            generation: 0,
            created_at: String::new(),
            updated_at: String::new(),
            parent_process: ProcessIdentity {
                pid: 0,
                process_created_at: String::new(),
                image_path: PathBuf::new(),
            },
            ..e.clone()
        })
        .unwrap_or(serde_json::Value::Null)
    };
    comparable(existing) == comparable(fresh)
}

/// Publish (or idempotently re-publish after a restart) the frozen session
/// envelope for a verified target: write-new-generation → flush → atomic
/// publish. The first durable write with handoff intent; the receipt stays
/// untouched (the `Updating` transition belongs to the destructive handoff).
pub fn publish_update_session(
    target: &VerifiedTarget,
    facts: HandoffFacts<'_>,
) -> Result<UpdateSessionEnvelope, SessionError> {
    let dir = session_dir(facts.updates_root, facts.session_id);
    let file = dir.join(SESSION_FILE);
    if is_reparse(&dir)? || is_reparse(&file)? {
        return Err(SessionError::ReparsePoint { path: file });
    }

    let existing = if file.exists() {
        Some(load_update_session(&dir)?)
    } else {
        None
    };
    let (created_at, generation) = match &existing {
        Some(previous) => (previous.created_at.clone(), previous.generation),
        None => (now_utc(), 0),
    };
    let fresh = build_envelope(target, &facts, created_at, now_utc(), generation + 1)?;

    if let Some(previous) = &existing {
        if !frozen_facts_agree(previous, &fresh) {
            // Find one disagreeing frozen field for the diagnostic.
            let field = if previous.manifest_sha256 != fresh.manifest_sha256 {
                "manifestSha256"
            } else if previous.to_version != fresh.to_version {
                "toVersion"
            } else if previous.from_version != fresh.from_version {
                "fromVersion"
            } else if previous.package_sha256 != fresh.package_sha256 {
                "packageSha256"
            } else if previous.installation_id != fresh.installation_id {
                "installationId"
            } else if previous.resources != fresh.resources {
                "resources"
            } else if previous.staging_root != fresh.staging_root {
                "stagingRoot"
            } else {
                "session"
            };
            return Err(SessionError::Conflict {
                field: field.to_string(),
            });
        }
        if previous.phase != SessionPhase::Staged {
            return Err(SessionError::Conflict {
                field: "phase".to_string(),
            });
        }
    }

    let bytes = serde_json::to_vec_pretty(&fresh)
        .map_err(|e| SessionError::Io { detail: e.to_string() })?;
    paths::atomic_write(&file, &bytes).map_err(|e| SessionError::Io {
        detail: e.to_string(),
    })?;
    // The milestone is only real if the durable bytes parse back to it.
    let published = load_update_session(&dir)?;
    let same = serde_json::to_value(&published).ok() == serde_json::to_value(&fresh).ok();
    if !same {
        return Err(SessionError::Io {
            detail: "published session does not read back identically".to_string(),
        });
    }
    Ok(published)
}

/// Strictly load a durable session envelope: bounded read, closed struct
/// (unknown fields, duplicate keys and trailing input rejected by the
/// deserializer), supported schema and updater protocol.
pub fn load_update_session(session_dir: &Path) -> Result<UpdateSessionEnvelope, SessionError> {
    let file = session_dir.join(SESSION_FILE);
    let std_file = paths::open_regular(&file).map_err(|e| SessionError::Io {
        detail: format!("session unreadable: {e}"),
    })?;
    let mut bytes = Vec::new();
    {
        use std::io::Read;
        let mut handle = std_file.take((SESSION_MAX_BYTES + 1) as u64);
        handle.read_to_end(&mut bytes).map_err(|e| SessionError::Io {
            detail: e.to_string(),
        })?;
    }
    if bytes.len() > SESSION_MAX_BYTES {
        return Err(SessionError::Malformed {
            detail: format!("session exceeds the {SESSION_MAX_BYTES}-byte bound"),
        });
    }
    let envelope: UpdateSessionEnvelope = serde_json::from_slice(&bytes).map_err(|e| {
        SessionError::Malformed {
            detail: e.to_string(),
        }
    })?;
    if envelope.schema_version != UpdateSessionEnvelope::SUPPORTED_SCHEMA_VERSION {
        return Err(SessionError::UnsupportedSchema {
            version: envelope.schema_version,
        });
    }
    if envelope.updater_protocol != UpdateSessionEnvelope::SUPPORTED_UPDATER_PROTOCOL {
        return Err(SessionError::UnsupportedSchema {
            version: envelope.updater_protocol,
        });
    }
    Ok(envelope)
}
