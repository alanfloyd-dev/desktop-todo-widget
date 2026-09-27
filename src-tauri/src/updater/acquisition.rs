//! Trusted-target persistence and package acquisition/staging (Phase 2C-A).
//!
//! Chain: a discovery run's `VerifiedTarget` — and nothing less — is
//! atomically persisted as the durable **trusted-target-persisted** record
//! (protocol v1, "Durable milestones": the moment verification succeeds, the
//! updater publishes the raw signed manifest/envelope bytes and the frozen
//! target digest under the session directory, before any package download).
//! Only then may the package be acquired: streamed under the signed declared
//! size, hash-verified against the signed digest, structurally validated
//! against the frozen ZIP rules and the exact nine-entry root allowlist, and
//! staged as the two managed executables cross-checked against the signed
//! `installFiles` facts.
//!
//! Invariants this module enforces structurally:
//! - a trusted record can only be constructed from a `VerifiedTarget` (the
//!   API takes no parsed manifest, no provider metadata, no "verified" bool);
//! - the durable record binds the exact raw manifest bytes — the
//!   `expectedManifestSha256` for the later helper handoff is the digest of
//!   those persisted bytes, recomputed and re-checked on every read;
//! - state is an explicit typed field, never inferred from directory
//!   existence: a partial download or partial extraction can never read as
//!   ready;
//! - every read fails closed on schema/version mismatch, malformed content,
//!   or digest disagreement — nothing is repaired into a trusted state;
//! - no durable state here is the frozen `UpdateSession` envelope: that is
//!   created at helper handoff (next phase) per the frozen schema. This
//!   record is the trusted-target-persisted milestone.
//!
//! Location: `<maintenance state root>/updates/sessions/<uuid>/` — the
//! frozen per-user maintenance layout. The caller supplies the canonical
//! updates root; production derives it from compiled policy
//! (`Paths::resolve().state()`), tests inject sandbox roots.

use std::io::Read;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use desktop_todo_maintenance::paths;
use desktop_todo_update_core::{verify_and_parse, TrustStore, VerifiedTarget, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
/// Lowercase hex of a SHA-256 digest already computed.
fn digest_hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The frozen per-session file names. The manifest and envelope keep their
/// release asset names; the milestone record and package use fixed names in
/// the session directory. `package.downloading` is the only partial form —
/// it is never a verified state.
pub const RECORD_FILE: &str = "trusted-target.json";
pub const MANIFEST_FILE: &str = "update-manifest.json";
pub const ENVELOPE_FILE: &str = "update-manifest.json.sig";
pub const PACKAGE_DOWNLOADING_FILE: &str = "package.downloading";
pub const PACKAGE_FILE: &str = "package.zip";
pub const STAGED_DIR: &str = "staged";

/// Explicit typed milestone state. Nothing in this module infers state from
/// the filesystem — a directory or file existing proves nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MilestoneState {
    /// The verified target is durable; package acquisition may start.
    TrustedTargetPersisted,
    /// The package is downloaded, hash-verified, archive-validated, and its
    /// two managed executables staged and cross-checked. Ready for the later
    /// helper handoff.
    PackageStaged,
}

/// One signed `installFiles` fact, copied from the verified manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InstallFileFact {
    pub identity: String,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}

/// The durable trusted-target record (schema 1, closed). Every field is a
/// fact of the verified target plus the caller's installed-version anchor;
/// the provider field is diagnostics only and never a trust input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TrustedTargetRecord {
    pub schema_version: u32,
    pub state: MilestoneState,
    pub target_version: String,
    /// SHA-256 over the exact raw manifest bytes — the
    /// `--expected-manifest-sha256` binding value for the helper handoff.
    pub manifest_sha256: String,
    /// SHA-256 over the exact raw envelope bytes (helper re-verifies the
    /// signature over the persisted manifest itself).
    pub envelope_sha256: String,
    /// The compatibility baseline this target was selected against.
    pub installed_source_version: String,
    pub package: PackageFact,
    pub install_files: Vec<InstallFileFact>,
    /// Diagnostics only; never a trust input.
    pub discovered_via: String,
    /// Transport locator for the package ZIP. Non-authoritative: content
    /// identity is the signed package hash, and the origin allowlist still
    /// applies on every fetch.
    pub package_url: String,
    pub created_at: String,
}

/// The signed package fact, copied from the verified manifest's selected
/// platform asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PackageFact {
    pub filename: String,
    pub size: u64,
    pub sha256: String,
}

impl TrustedTargetRecord {
    const SUPPORTED_SCHEMA_VERSION: u32 = 1;

    fn from_verified_target(
        target: &VerifiedTarget,
        package_url: &str,
        discovered_via: &str,
        installed_source_version: &desktop_todo_update_core::Version,
        created_at: String,
    ) -> Self {
        let asset = target.manifest().asset();
        Self {
            schema_version: Self::SUPPORTED_SCHEMA_VERSION,
            state: MilestoneState::TrustedTargetPersisted,
            target_version: target.manifest().version().to_string(),
            manifest_sha256: target.manifest_sha256_hex().to_string(),
            envelope_sha256: String::new(), // filled by the persist step
            installed_source_version: installed_source_version.to_string(),
            package: PackageFact {
                filename: asset.filename().to_string(),
                size: asset.size(),
                sha256: asset.sha256_hex().to_string(),
            },
            install_files: asset
                .install_files()
                .iter()
                .map(|entry| InstallFileFact {
                    identity: entry.identity().to_string(),
                    filename: entry.filename().to_string(),
                    size: entry.size(),
                    sha256: entry.sha256_hex().to_string(),
                })
                .collect(),
            discovered_via: discovered_via.to_string(),
            package_url: package_url.to_string(),
            created_at,
        }
    }

    fn install_file_expectations(&self) -> Vec<(String, String, u64, String)> {
        self.install_files
            .iter()
            .map(|fact| {
                (
                    fact.identity.clone(),
                    fact.filename.clone(),
                    fact.size,
                    fact.sha256.clone(),
                )
            })
            .collect()
    }
}

/// Durable-state failures. Distinct from transport and candidate errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistError {
    SessionExists {
        path: PathBuf,
    },
    Io {
        detail: String,
    },
    DigestMismatch {
        file: String,
    },
    Malformed {
        detail: String,
    },
    UnsupportedSchema {
        version: u32,
    },
    /// The persisted raw bytes no longer verify against the trust store
    /// (tampered manifest/envelope, wrong key, corrupted bytes).
    SignatureRecovery {
        detail: String,
    },
    /// A signed-derived field in the durable record disagrees with the
    /// re-verified target: the record was tampered with and cannot be
    /// repaired into a trusted state.
    RecordTampered {
        field: String,
    },
    /// The recorded compatibility baseline differs from the current actual
    /// installed source version — the staged state predates a
    /// rollback/reinstall and must not be reused.
    SourceVersionChanged {
        recorded: String,
        current: String,
    },
    /// A destination directory is (or passes through) a reparse point.
    ReparsePoint {
        path: PathBuf,
    },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistError::SessionExists { path } => {
                write!(f, "session path already exists: {}", path.display())
            }
            PersistError::Io { detail } => write!(f, "durable write failure: {detail}"),
            PersistError::DigestMismatch { file } => {
                write!(f, "persisted {file} does not match its recorded digest")
            }
            PersistError::Malformed { detail } => write!(f, "record malformed: {detail}"),
            PersistError::UnsupportedSchema { version } => {
                write!(f, "record schema version {version} not supported")
            }
            PersistError::SignatureRecovery { detail } => {
                write!(f, "persisted bytes fail signature recovery: {detail}")
            }
            PersistError::RecordTampered { field } => {
                write!(
                    f,
                    "durable record field {field} disagrees with the re-verified target"
                )
            }
            PersistError::SourceVersionChanged { recorded, current } => {
                write!(
                    f,
                    "recorded source baseline {recorded} != current installed {current}; rediscovery required"
                )
            }
            PersistError::ReparsePoint { path } => {
                write!(
                    f,
                    "destination passes through a reparse point: {}",
                    path.display()
                )
            }
        }
    }
}

/// Package acquisition and staging failures — a layer of their own, never
/// candidate verdicts and never signature verdicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquisitionError {
    NoTrustedTarget,
    PackageUrlMissing,
    PackageUrlRejected { detail: String },
    Persist(PersistError),
    Transport { detail: String },
    HttpStatus { status: u16 },
    BodyTooLarge { cap: u64 },
    SizeMismatch { declared: u64, actual: u64 },
    PackageHashMismatch { expected: String, actual: String },
    Archive(desktop_todo_maintenance::package_zip::ArchiveError),
    StateConflict { detail: String },
}

impl std::fmt::Display for AcquisitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcquisitionError::NoTrustedTarget => {
                write!(f, "no durable trusted target for this session")
            }
            AcquisitionError::PackageUrlMissing => {
                write!(f, "provider published no package asset locator")
            }
            AcquisitionError::PackageUrlRejected { detail } => {
                write!(f, "package url rejected: {detail}")
            }
            AcquisitionError::Persist(error) => write!(f, "{error}"),
            AcquisitionError::Transport { detail } => write!(f, "transport failure: {detail}"),
            AcquisitionError::HttpStatus { status } => write!(f, "provider HTTP status {status}"),
            AcquisitionError::BodyTooLarge { cap } => {
                write!(f, "package body exceeded the {cap}-byte bound")
            }
            AcquisitionError::SizeMismatch { declared, actual } => {
                write!(
                    f,
                    "package is {actual} bytes, signed manifest declares {declared}"
                )
            }
            AcquisitionError::PackageHashMismatch { expected, actual } => {
                write!(f, "package hash {actual} != signed {expected}")
            }
            AcquisitionError::Archive(error) => write!(f, "package archive rejected: {error:?}"),
            AcquisitionError::StateConflict { detail } => write!(f, "state conflict: {detail}"),
        }
    }
}

/// One durable update session. `target` is **re-derived at every recovery**
/// by running the persisted raw bytes back through
/// `update_core::verify_and_parse` with the caller's compiled trust store —
/// on-disk content is untrusted input after a restart, and no signed-derived
/// field regains authority merely because the record survived. The record's
/// own signed-derived fields are compared against the re-derived target and
/// any disagreement fails closed; the record remains the carrier of the
/// typed milestone state, diagnostics, and transport data only.
#[derive(Debug)]
pub struct UpdateSession {
    pub id: String,
    pub dir: PathBuf,
    pub record: TrustedTargetRecord,
    pub target: VerifiedTarget,
}

fn session_dir(updates_root: &Path, id: &str) -> PathBuf {
    updates_root.join("sessions").join(id)
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// Fail closed when `path` exists as, or passes through, a reparse point
/// (junction/symlink). The frozen threat model does not defend against a
/// malicious same-user process, but accidental or pre-existing reparse
/// substitution of a maintenance destination must never be followed.
fn assert_no_reparse(path: &Path) -> Result<(), PersistError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(PersistError::ReparsePoint {
                    path: path.to_path_buf(),
                });
            }
            Ok(())
        }
        Err(_) => Ok(()), // nonexistent: nothing to follow
    }
}

/// Walk the record JSON once, rejecting any object that repeats a key.
fn reject_duplicate_keys(bytes: &[u8]) -> Result<(), serde_json::Error> {
    use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
    use std::collections::HashSet;

    struct NoDup;
    struct NoDupVisitor;

    impl<'de> Visitor<'de> for NoDupVisitor {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("any JSON value")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<(), A::Error> {
            let mut seen: HashSet<String> = HashSet::new();
            while let Some(key) = access.next_key::<String>()? {
                if !seen.insert(key) {
                    return Err(serde::de::Error::custom("duplicate object key"));
                }
                access.next_value::<NoDup>()?;
            }
            Ok(())
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<(), A::Error> {
            while access.next_element::<NoDup>()?.is_some() {}
            Ok(())
        }
        fn visit_bool<E: serde::de::Error>(self, _v: bool) -> Result<(), E> {
            Ok(())
        }
        fn visit_i64<E: serde::de::Error>(self, _v: i64) -> Result<(), E> {
            Ok(())
        }
        fn visit_u64<E: serde::de::Error>(self, _v: u64) -> Result<(), E> {
            Ok(())
        }
        fn visit_f64<E: serde::de::Error>(self, _v: f64) -> Result<(), E> {
            Ok(())
        }
        fn visit_str<E: serde::de::Error>(self, _v: &str) -> Result<(), E> {
            Ok(())
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
            Ok(())
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<(), E> {
            Ok(())
        }
    }

    impl<'de> Deserialize<'de> for NoDup {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(NoDupVisitor)?;
            Ok(NoDup)
        }
    }

    let mut de = serde_json::Deserializer::from_slice(bytes);
    NoDup::deserialize(&mut de).map(|_| ())
}

fn load_record(session_dir: &Path) -> Result<TrustedTargetRecord, PersistError> {
    let bytes = std::fs::read(session_dir.join(RECORD_FILE)).map_err(|e| PersistError::Io {
        detail: format!("record unreadable: {e}"),
    })?;
    // Closed struct: unknown fields, duplicate keys, and trailing input fail
    // closed (the duplicate-key walk mirrors the protocol's closed-document
    // rule for this durable record).
    reject_duplicate_keys(&bytes).map_err(|e| PersistError::Malformed {
        detail: e.to_string(),
    })?;
    let record: TrustedTargetRecord =
        serde_json::from_slice(&bytes).map_err(|e| PersistError::Malformed {
            detail: e.to_string(),
        })?;
    if record.schema_version != TrustedTargetRecord::SUPPORTED_SCHEMA_VERSION {
        return Err(PersistError::UnsupportedSchema {
            version: record.schema_version,
        });
    }
    Ok(record)
}

/// Re-verify the persisted raw manifest/envelope bytes against the record's
/// digests. The binding survives restarts only while the bytes do.
fn verify_persisted_bytes(
    session_dir: &Path,
    record: &TrustedTargetRecord,
) -> Result<(), PersistError> {
    let manifest =
        std::fs::read(session_dir.join(MANIFEST_FILE)).map_err(|e| PersistError::Io {
            detail: format!("persisted manifest unreadable: {e}"),
        })?;
    let envelope =
        std::fs::read(session_dir.join(ENVELOPE_FILE)).map_err(|e| PersistError::Io {
            detail: format!("persisted envelope unreadable: {e}"),
        })?;
    if desktop_todo_update_core::sha256_hex(&manifest) != record.manifest_sha256 {
        return Err(PersistError::DigestMismatch {
            file: MANIFEST_FILE.to_string(),
        });
    }
    if desktop_todo_update_core::sha256_hex(&envelope) != record.envelope_sha256 {
        return Err(PersistError::DigestMismatch {
            file: ENVELOPE_FILE.to_string(),
        });
    }
    Ok(())
}

/// Persist a verified target as the durable trusted-target milestone.
///
/// The only input that can reach this function is a `VerifiedTarget` — the
/// type produced exclusively by the update-core verifier after the full
/// eligibility predicate. Raw manifest and envelope bytes are stored exactly
/// as served; the record binds their digests.
pub fn persist_trusted_target(
    updates_root: &Path,
    target: VerifiedTarget,
    envelope_bytes: &[u8],
    package_url: Option<&str>,
    discovered_via: &str,
    installed_source_version: &desktop_todo_update_core::Version,
) -> Result<UpdateSession, PersistError> {
    let id = uuid::Uuid::new_v4().to_string();
    let sessions_dir = updates_root.join("sessions");
    assert_no_reparse(&sessions_dir)?;
    let dir = session_dir(updates_root, &id);
    if dir.exists() {
        return Err(PersistError::SessionExists { path: dir });
    }
    std::fs::create_dir_all(&dir).map_err(|e| PersistError::Io {
        detail: e.to_string(),
    })?;
    assert_no_reparse(&dir)?;

    // Exact raw bytes, unmodified, under their frozen asset names.
    paths::atomic_write(&dir.join(MANIFEST_FILE), target.raw_bytes()).map_err(|e| {
        PersistError::Io {
            detail: e.to_string(),
        }
    })?;
    paths::atomic_write(&dir.join(ENVELOPE_FILE), envelope_bytes).map_err(|e| {
        PersistError::Io {
            detail: e.to_string(),
        }
    })?;

    let mut record = TrustedTargetRecord::from_verified_target(
        &target,
        package_url.unwrap_or_default(),
        discovered_via,
        installed_source_version,
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    record.envelope_sha256 = desktop_todo_update_core::sha256_hex(envelope_bytes);

    let record_bytes = serde_json::to_vec_pretty(&record).map_err(|e| PersistError::Io {
        detail: e.to_string(),
    })?;
    paths::atomic_write(&dir.join(RECORD_FILE), &record_bytes).map_err(|e| PersistError::Io {
        detail: e.to_string(),
    })?;

    // The milestone is only real if the durable bytes agree with it.
    verify_persisted_bytes(&dir, &record)?;
    Ok(UpdateSession {
        id,
        dir,
        record,
        target,
    })
}

/// Recover a previously persisted session by id (restart path).
///
/// Trust is **re-established from the persisted raw bytes**, never assumed
/// from the record's existence: the persisted `update-manifest.json(.sig)`
/// are untrusted disk content after a restart, so they are run back through
/// `update_core::verify_and_parse` with the caller's compiled trust store
/// (production: `production_trust_store()` — an empty store fails closed
/// until release-signing provisioning). The re-derived `VerifiedTarget` is
/// the only authority for targetVersion, package facts, installFiles, and
/// the expected-manifest digest; every signed-derived field in the record
/// must agree with it or the session fails closed. The recorded
/// compatibility baseline must equal the caller's current actual installed
/// source version — a target staged under an older runtime (after a
/// rollback or reinstall) is never reused.
pub fn recover_session(
    updates_root: &Path,
    id: &str,
    trust: &TrustStore,
    current_installed: &Version,
) -> Result<UpdateSession, PersistError> {
    let dir = session_dir(updates_root, id);
    assert_no_reparse(&dir)?;
    let record = load_record(&dir)?;
    verify_persisted_bytes(&dir, &record)?;

    // Re-verification over the exact persisted bytes: canonical Base64,
    // compiled-key lookup, Ed25519, closed parsing, full semantics.
    let envelope = std::fs::read(dir.join(ENVELOPE_FILE)).map_err(|e| PersistError::Io {
        detail: format!("persisted envelope unreadable: {e}"),
    })?;
    let manifest = std::fs::read(dir.join(MANIFEST_FILE)).map_err(|e| PersistError::Io {
        detail: format!("persisted manifest unreadable: {e}"),
    })?;
    let target = verify_and_parse(trust, &envelope, &manifest).map_err(|e| {
        PersistError::SignatureRecovery {
            detail: e.to_string(),
        }
    })?;

    // Field-by-field agreement between the durable record and the
    // re-verified target (belt-and-braces on top of derivation: a tampered
    // record cannot smuggle a stale or altered fact back in).
    let asset = target.manifest().asset();
    let tampered = |field: &str| PersistError::RecordTampered {
        field: field.to_string(),
    };
    if record.target_version != target.manifest().version().to_string() {
        return Err(tampered("targetVersion"));
    }
    if record.manifest_sha256 != target.manifest_sha256_hex() {
        return Err(tampered("manifestSha256"));
    }
    if record.package.filename != asset.filename()
        || record.package.size != asset.size()
        || record.package.sha256 != asset.sha256_hex()
    {
        return Err(tampered("package"));
    }
    let record_files: Vec<(String, String, u64, String)> = record
        .install_files
        .iter()
        .map(|fact| {
            (
                fact.identity.clone(),
                fact.filename.clone(),
                fact.size,
                fact.sha256.clone(),
            )
        })
        .collect();
    let target_files: Vec<(String, String, u64, String)> = asset
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
        .collect();
    if record_files != target_files {
        return Err(tampered("installFiles"));
    }

    // Option A compatibility baseline: the recorded source version must be
    // the *current actual* installed source version.
    if record.installed_source_version != current_installed.to_string() {
        return Err(PersistError::SourceVersionChanged {
            recorded: record.installed_source_version.clone(),
            current: current_installed.to_string(),
        });
    }

    Ok(UpdateSession {
        id: id.to_string(),
        dir,
        record,
        target,
    })
}

/// Download the package for a persisted, trusted session and stage it:
/// bounded streaming download → exact declared size → exact signed SHA-256
/// → full archive validation against the frozen rules and the exact
/// nine-entry allowlist → staged managed executables cross-checked against
/// the signed `installFiles` → the record atomically advanced to
/// `PackageStaged`. Idempotent on restart when already staged.
pub fn acquire_and_stage(
    session: &mut UpdateSession,
    client: &reqwest::blocking::Client,
) -> Result<(), AcquisitionError> {
    match session.record.state {
        MilestoneState::PackageStaged => {
            // Restart (scenario E): staged files must still verify.
            let staged_dir = session.dir.join(STAGED_DIR);
            let expectations = session.record.install_file_expectations();
            desktop_todo_maintenance::package_zip::verify_staged_executables(&staged_dir, &expectations)
                .map_err(AcquisitionError::Archive)?;
            return Ok(());
        }
        MilestoneState::TrustedTargetPersisted => {}
    }

    let url = if session.record.package_url.is_empty() {
        return Err(AcquisitionError::PackageUrlMissing);
    } else {
        &session.record.package_url
    };
    // The locator must name the signed package file exactly; provider
    // recommendations and frontend arguments play no part.
    let url = reqwest::Url::parse(url).map_err(|e| AcquisitionError::PackageUrlRejected {
        detail: e.to_string(),
    })?;
    let filename = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    if filename != session.record.package.filename {
        return Err(AcquisitionError::PackageUrlRejected {
            detail: format!(
                "package asset {filename:?} does not match the signed package filename {:?}",
                session.record.package.filename
            ),
        });
    }

    let declared_size = session.record.package.size;
    let compiled_cap = desktop_todo_update_core::PACKAGE_MAX_BYTES;
    if declared_size > compiled_cap {
        return Err(AcquisitionError::BodyTooLarge { cap: compiled_cap });
    }

    // Bounded streaming download: precheck the declared Content-Length, then
    // stream with a hard ceiling of declared + 1 bytes straight into the
    // partial temp file, hashing as we go. The body is never fully in RAM.
    let response = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .map_err(|e| {
            if e.is_timeout() {
                AcquisitionError::Transport {
                    detail: "package download timed out".to_string(),
                }
            } else {
                AcquisitionError::Transport {
                    detail: e.to_string(),
                }
            }
        })?;
    let status = response.status();
    if !status.is_success() {
        return Err(AcquisitionError::HttpStatus {
            status: status.as_u16(),
        });
    }
    if let Some(encoding) = response.headers().get(reqwest::header::CONTENT_ENCODING) {
        let encoding = encoding.to_str().unwrap_or_default().trim();
        if !encoding.eq_ignore_ascii_case("identity") {
            return Err(AcquisitionError::PackageUrlRejected {
                detail: format!("non-identity content encoding {encoding:?}"),
            });
        }
    }
    if let Some(len) = response.content_length() {
        if len > compiled_cap {
            return Err(AcquisitionError::BodyTooLarge { cap: compiled_cap });
        }
        if len != declared_size {
            return Err(AcquisitionError::SizeMismatch {
                declared: declared_size,
                actual: len,
            });
        }
    }

    let downloading = session.dir.join(PACKAGE_DOWNLOADING_FILE);
    let mut file = std::fs::File::create(&downloading).map_err(|e| {
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;
    let mut reader = response.take(declared_size.saturating_add(1));
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| AcquisitionError::Transport {
                detail: e.to_string(),
            })?;
        if read == 0 {
            break;
        }
        downloaded = downloaded.saturating_add(read as u64);
        if downloaded > declared_size {
            let _ = std::fs::remove_file(&downloading);
            return Err(AcquisitionError::SizeMismatch {
                declared: declared_size,
                actual: downloaded,
            });
        }
        hasher.update(&buffer[..read]);
        std::io::Write::write_all(&mut file, &buffer[..read]).map_err(|e| {
            let _ = std::fs::remove_file(&downloading);
            AcquisitionError::Persist(PersistError::Io {
                detail: e.to_string(),
            })
        })?;
    }
    file.sync_all().map_err(|e| {
        let _ = std::fs::remove_file(&downloading);
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;
    drop(file);

    // Exact declared size — the signed manifest is the authority.
    if downloaded != declared_size {
        let _ = std::fs::remove_file(&downloading);
        return Err(AcquisitionError::SizeMismatch {
            declared: declared_size,
            actual: downloaded,
        });
    }

    // Exact signed package hash over the actual downloaded bytes, computed
    // before anything is staged or published. The partial file is never a
    // verified state; on mismatch it is removed so no ready state survives.
    let actual_hash = digest_hex(&hasher.finalize());
    if actual_hash != session.record.package.sha256 {
        let _ = std::fs::remove_file(&downloading);
        return Err(AcquisitionError::PackageHashMismatch {
            expected: session.record.package.sha256.clone(),
            actual: actual_hash,
        });
    }

    // Publish the verified package bytes atomically.
    paths::move_replace(&downloading, &session.dir.join(PACKAGE_FILE)).map_err(|e| {
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;

    // Full archive validation before any extraction, then staged extraction
    // of exactly the two managed executables with per-file cross-checks.
    let package_bytes = std::fs::read(session.dir.join(PACKAGE_FILE)).map_err(|e| {
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;
    let archive =
        desktop_todo_maintenance::package_zip::validate_archive(std::io::Cursor::new(package_bytes))
            .map_err(AcquisitionError::Archive)?;
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
    let staged_dir = session.dir.join(STAGED_DIR);
    assert_no_reparse(&staged_dir).map_err(AcquisitionError::Persist)?;
    desktop_todo_maintenance::package_zip::extract_managed_executables(archive, &staged_dir, &expectations)
        .map_err(AcquisitionError::Archive)?;

    // Atomically advance the typed state to PackageStaged. The marker write
    // is the only thing that makes staging "ready"; a crash before it leaves
    // a TrustedTargetPersisted record whose partial staged files are
    // re-acquired from scratch on the next run.
    session.record.state = MilestoneState::PackageStaged;
    let record_bytes = serde_json::to_vec_pretty(&session.record).map_err(|e| {
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;
    paths::atomic_write(&session.dir.join(RECORD_FILE), &record_bytes).map_err(|e| {
        AcquisitionError::Persist(PersistError::Io {
            detail: e.to_string(),
        })
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record type rejects unknown fields and wrong schema versions.
    #[test]
    fn record_parsing_fails_closed() {
        let valid = r#"{"schemaVersion":1,"state":"trustedTargetPersisted","targetVersion":"1.4.0","manifestSha256":"a","envelopeSha256":"b","installedSourceVersion":"1.2.0","package":{"filename":"p.zip","size":1,"sha256":"c"},"installFiles":[],"discoveredVia":"GitHub","packageUrl":"https://x/p.zip","createdAt":"2026-10-01T00:00:00Z"}"#;
        assert!(serde_json::from_str::<TrustedTargetRecord>(valid).is_ok());
        // Unknown fields fail closed (deny_unknown_fields).
        assert!(serde_json::from_str::<TrustedTargetRecord>(
            valid
                .replace(
                    "\"targetVersion\":\"1.4.0\"",
                    "\"targetVersion\":\"1.4.0\",\"extra\":1"
                )
                .as_str()
        )
        .is_err());
        // Duplicate keys fail closed (structural pre-pass used by load_record).
        assert!(
            reject_duplicate_keys(r#"{"schemaVersion":1,"schemaVersion":1}"#.as_bytes()).is_err()
        );
        // schemaVersion enforcement lives in load_record and is covered by
        // the restart tests through recover_session.
    }
}
