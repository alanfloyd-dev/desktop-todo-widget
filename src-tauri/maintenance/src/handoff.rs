//! The frozen `--update` helper handoff: independent validation, stopping
//! exactly at "validated and ready to mutate" (Phase 2C-B).
//!
//! The helper is an offline executor. It never contacts a provider, never
//! trusts the main application's "verified" claim, and never trusts a
//! boolean or a provider field: every signed-derived fact is re-derived
//! here from the persisted raw bytes through the shared `update-core`
//! verifier with this binary's own compiled trust store, and every local
//! binding (session id, staging directory, installed source version,
//! staged file hashes) is re-derived from compiled policy and the validated
//! receipt. A session whose claims disagree with independently derived
//! state is a refusal — an edited session can only make the updater
//! refuse, never widen it.
//!
//! The CLI value `--expected-manifest-sha256` is a binding check, not an
//! authority: it decides *which* verified target is the approved one. The
//! helper re-verifies the signature over the persisted bytes, recomputes
//! the digest, and requires exact equality with the CLI value — then
//! persists that digest into its own journal before any destructive work.
//!
//! This phase stops before any mutation: no installed runtime file is
//! replaced, no lifecycle transition is written, no lease is taken. The
//! only durable writes are the helper journal (the verified digest, before
//! any destructive work) and nothing else. The returned
//! [`ValidatedHandoff`] is the unforgeable boundary type the future
//! replacement API will require instead of raw session JSON or paths.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use desktop_todo_update_core::{
    sha256_hex, verify_and_parse, TrustStore, VerifiedTarget, Version,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::{self, Paths};
use crate::receipt::{validate_uuid, Lifecycle, Receipt};
use crate::update_session::{
    backup_slot, load_update_session, SessionOperation, SessionPhase, UpdateSessionEnvelope,
    SESSION_MAX_BYTES,
};
use crate::{APP_ID, Error, ErrorKind, MAIN_EXE};

/// The frozen per-session file layout (durable session, manifest, sig,
/// package, staging — application-lifecycle §4). The manifest/envelope keep
/// their release asset names.
pub const MANIFEST_FILE: &str = "update-manifest.json";
pub const ENVELOPE_FILE: &str = "update-manifest.json.sig";
pub const PACKAGE_FILE: &str = "package.zip";
pub const STAGED_DIR: &str = "staged";
/// The helper's own journal inside the session directory: the verified
/// digest is persisted here before any destructive work.
pub const HELPER_JOURNAL_FILE: &str = "helper-journal.json";
/// Installed signed release evidence (commit publishes these; absent for
/// bootstrap-only installations).
pub const INSTALLED_MANIFEST_FILE: &str = "installed-manifest.json";
pub const INSTALLED_ENVELOPE_FILE: &str = "installed-manifest.json.sig";

/// Why the handoff was refused. A closed taxonomy of its own — never
/// collapsed into a generic failure, never reusing a lower-layer error kind
/// unless the failure really originates there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffError {
    /// The CLI arguments are not the frozen handoff form.
    CliArgument { detail: String },
    /// The session directory does not exist under the canonical sessions
    /// root.
    SessionMissing,
    SessionMalformed { detail: String },
    UnsupportedSchema { version: u32 },
    /// The session is well-formed but its state/fields are illegal for the
    /// selected operation (wrong phase, phase-gated fields present,
    /// unusable process identity).
    InvalidSessionState { detail: String },
    /// The selected operation does not agree with the persisted session
    /// type.
    OperationMismatch { session: String, requested: String },
    /// The CLI `--expected-manifest-sha256` disagrees with the session's
    /// frozen digest.
    ExpectedDigestMismatch { cli: String, session: String },
    /// The recomputed digest of the persisted raw manifest bytes disagrees
    /// with the expected digest.
    ManifestDigestMismatch { expected: String, actual: String },
    /// The persisted raw bytes fail the helper's own signature recovery
    /// (tampered bytes, unknown key, malformed envelope).
    SignatureRecovery { detail: String },
    /// The recorded/derived source version is no longer the actual
    /// installed source version (rollback/reinstall happened).
    SourceVersionChanged { recorded: String, current: String },
    /// The session's target version disagrees with the re-verified signed
    /// manifest.
    TargetVersionMismatch { session: String, manifest: String },
    /// A session field disagrees with the re-verified signed facts.
    SignedFactMismatch { field: String },
    /// The staged package is missing or fails its signed hash.
    PackageMissing,
    PackageHashMismatch { expected: String, actual: String },
    /// The staged package fails the frozen archive validation.
    PackageInvalid { detail: String },
    StagedFileMissing { file: String },
    StagedSizeMismatch { file: String, declared: u64, actual: u64 },
    StagedHashMismatch { file: String, expected: String, actual: String },
    /// The staging directory carries an unexpected entry (extra file,
    /// subdirectory, leftover temp).
    StagedUnexpectedFile { name: String },
    /// Session/staging/install-root identity bindings disagree with the
    /// independently derived locations.
    StagingIdentityMismatch { detail: String },
    /// A destination passes through (or is) a reparse point.
    UnsafePath { path: PathBuf },
    /// The local installation cannot support a handoff (no receipt,
    /// malformed receipt, broken version anchor, committed-evidence
    /// conflict).
    InstallationInvalid { detail: String },
    /// The receipt is in a transitional lifecycle state.
    LifecycleBusy { detail: String },
    Io { detail: String },
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CliArgument { detail } => write!(f, "handoff CLI arguments rejected: {detail}"),
            Self::SessionMissing => write!(f, "no session exists under the canonical sessions root"),
            Self::SessionMalformed { detail } => write!(f, "session malformed: {detail}"),
            Self::UnsupportedSchema { version } => {
                write!(f, "session schema/protocol {version} not supported")
            }
            Self::InvalidSessionState { detail } => write!(f, "session state invalid: {detail}"),
            Self::OperationMismatch { session, requested } => {
                write!(f, "session operation {session:?} does not agree with the requested {requested:?}")
            }
            Self::ExpectedDigestMismatch { cli, session } => write!(
                f,
                "CLI expected manifest digest {cli} does not bind this session ({session})"
            ),
            Self::ManifestDigestMismatch { expected, actual } => write!(
                f,
                "persisted manifest digest {actual} != expected {expected}"
            ),
            Self::SignatureRecovery { detail } => {
                write!(f, "persisted bytes fail signature recovery: {detail}")
            }
            Self::SourceVersionChanged { recorded, current } => write!(
                f,
                "session source baseline {recorded} != actual installed {current}; stale session refused"
            ),
            Self::TargetVersionMismatch { session, manifest } => write!(
                f,
                "session target {session} != signed manifest target {manifest}"
            ),
            Self::SignedFactMismatch { field } => write!(
                f,
                "session field {field} disagrees with the re-verified signed facts"
            ),
            Self::PackageMissing => write!(f, "staged package missing"),
            Self::PackageHashMismatch { expected, actual } => {
                write!(f, "staged package hash {actual} != signed {expected}")
            }
            Self::PackageInvalid { detail } => write!(f, "staged package rejected: {detail}"),
            Self::StagedFileMissing { file } => write!(f, "staged file missing: {file}"),
            Self::StagedSizeMismatch { file, declared, actual } => {
                write!(f, "staged {file} is {actual} bytes, signed {declared}")
            }
            Self::StagedHashMismatch { file, expected, actual } => {
                write!(f, "staged {file} hash {actual} != signed {expected}")
            }
            Self::StagedUnexpectedFile { name } => {
                write!(f, "staging directory carries unexpected entry {name:?}")
            }
            Self::StagingIdentityMismatch { detail } => {
                write!(f, "session/staging identity mismatch: {detail}")
            }
            Self::UnsafePath { path } => {
                write!(f, "handoff path passes through a reparse point: {}", path.display())
            }
            Self::InstallationInvalid { detail } => {
                write!(f, "installation cannot support a handoff: {detail}")
            }
            Self::LifecycleBusy { detail } => write!(f, "installation lifecycle busy: {detail}"),
            Self::Io { detail } => write!(f, "handoff IO failure: {detail}"),
        }
    }
}

impl From<HandoffError> for Error {
    fn from(error: HandoffError) -> Self {
        Error::new(ErrorKind::UpdateHandoffRejected, error.to_string())
    }
}

/// The independently derived actual installed source version (provisional
/// anchor and, where committed evidence exists, the full consistency
/// invariant): the validated receipt's `currentVersion`, cross-checked
/// against the installed main executable's PE ProductVersion, and — when
/// the receipt records committed signed evidence — against that evidence's
/// verified target version. Any disagreement is an installation conflict,
/// never a selection input.
pub fn derive_installed_source(paths: &Paths, trust: &TrustStore) -> Result<Version, HandoffError> {
    let receipt = Receipt::load(paths).map_err(|e| HandoffError::InstallationInvalid {
        detail: format!("receipt unusable: {e}"),
    })?
    .ok_or(HandoffError::InstallationInvalid {
        detail: "no installation receipt".to_string(),
    })?;
    if receipt.lifecycle_state != Lifecycle::Installed {
        return Err(HandoffError::LifecycleBusy {
            detail: format!("receipt lifecycle is {:?}", receipt.lifecycle_state),
        });
    }
    let current = receipt
        .current_version
        .as_deref()
        .ok_or(HandoffError::InstallationInvalid {
            detail: "receipt carries no current version".to_string(),
        })?;

    // Provisional anchor: Receipt.currentVersion == main EXE PE ProductVersion.
    let main_exe = paths.install().join(MAIN_EXE);
    let pe_version = crate::lifecycle::product_version(&main_exe).map_err(|e| {
        HandoffError::InstallationInvalid {
            detail: format!("installed main executable version unreadable: {e}"),
        }
    })?;
    if pe_version != current {
        return Err(HandoffError::InstallationInvalid {
            detail: format!(
                "receipt currentVersion {current:?} != main executable PE {pe_version:?}"
            ),
        });
    }

    // Full invariant where committed signed evidence exists.
    if let Some(digest) = receipt
        .maintenance
        .committed_manifest_sha256
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        let manifest = read_bounded(&paths.install().join(INSTALLED_MANIFEST_FILE), "installed manifest")?;
        let envelope =
            read_bounded(&paths.install().join(INSTALLED_ENVELOPE_FILE), "installed envelope")?;
        let committed = verify_and_parse(trust, &envelope, &manifest).map_err(|e| {
            HandoffError::InstallationInvalid {
                detail: format!("committed signed evidence fails re-verification: {e}"),
            }
        })?;
        if sha256_hex(&manifest) != digest {
            return Err(HandoffError::InstallationInvalid {
                detail: "committed manifest digest disagrees with the receipt".to_string(),
            });
        }
        if committed.manifest().version().to_string() != current {
            return Err(HandoffError::InstallationInvalid {
                detail: "committed evidence target disagrees with the receipt version"
                    .to_string(),
            });
        }
    }

    Version::parse(current).map_err(|e| HandoffError::InstallationInvalid {
        detail: format!("receipt currentVersion {current:?} is not canonical: {e}"),
    })
}

fn read_bounded(path: &Path, what: &str) -> Result<Vec<u8>, HandoffError> {
    let file = paths::open_regular(path).map_err(|e| HandoffError::Io {
        detail: format!("{what} unreadable: {e}"),
    })?;
    let mut bytes = Vec::new();
    file.take((SESSION_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| HandoffError::Io { detail: e.to_string() })?;
    if bytes.len() > SESSION_MAX_BYTES {
        return Err(HandoffError::Io {
            detail: format!("{what} exceeds its size bound"),
        });
    }
    Ok(bytes)
}

fn is_digest_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn assert_no_reparse(path: &Path) -> Result<(), HandoffError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x0400 != 0 {
                return Err(HandoffError::UnsafePath { path: path.to_path_buf() });
            }
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

/// Stream SHA-256 and byte size through an already opened regular file.
fn hash_stream(file: &mut std::fs::File) -> Result<(u64, String), HandoffError> {
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| HandoffError::Io { detail: e.to_string() })?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    // Hex of the digest itself — `sha256_hex` would hash the digest again.
    Ok((size, format!("{:x}", hasher.finalize())))
}

/// The validated handoff outcome. Constructed only by
/// [`validate_update_handoff`] after every check passed and the verified
/// digest is durably journaled; there is no public constructor, so no
/// caller can fabricate a mutation-ready claim from session JSON or paths.
#[derive(Debug)]
pub struct ValidatedHandoff {
    session_id: String,
    manifest_sha256: String,
    target_version: String,
    source_version: String,
    install_root: PathBuf,
    session_dir: PathBuf,
    envelope: UpdateSessionEnvelope,
    target: VerifiedTarget,
    staged: Vec<crate::package_zip::StagedExecutable>,
}

impl ValidatedHandoff {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn target_version(&self) -> &str {
        &self.target_version
    }
    pub fn source_version(&self) -> &str {
        &self.source_version
    }
    pub fn install_root(&self) -> &Path {
        &self.install_root
    }
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }
    pub fn envelope(&self) -> &UpdateSessionEnvelope {
        &self.envelope
    }
    pub fn target(&self) -> &VerifiedTarget {
        &self.target
    }
    pub fn staged(&self) -> &[crate::package_zip::StagedExecutable] {
        &self.staged
    }
}

/// The helper journal record (implementation-internal durable state, closed
/// struct). Not a frozen protocol document: it carries the verified digest
/// and the validated bindings so a later destructive phase can prove what
/// was validated, and nothing more.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HelperJournal {
    journal_schema_version: u32,
    session_id: String,
    verified_manifest_sha256: String,
    target_version: String,
    source_version: String,
    install_root: PathBuf,
    validated_at: String,
}

/// Validate one frozen `--update` handoff, independently and offline, and
/// stop exactly at "validated and ready to mutate". No runtime file is
/// replaced, no lifecycle transition happens, no lease is taken; the only
/// durable write is the helper journal.
///
/// `installed_source` is the actual installed source version derived by
/// [`derive_installed_source`] (the production CLI derives it; it is a
/// parameter so tests can exercise the rebinding semantics deterministically
/// without fabricating PE version resources).
pub fn validate_update_handoff(
    paths: &Paths,
    trust: &TrustStore,
    session_id_arg: &str,
    expected_manifest_sha256_arg: &str,
    installed_source: &Version,
) -> Result<ValidatedHandoff, HandoffError> {
    // A. resolve session — the CLI value is the only handle a caller names,
    // resolved strictly against the canonical sessions root.
    validate_uuid(session_id_arg).map_err(|e| HandoffError::CliArgument {
        detail: format!("--session-id: {e}"),
    })?;
    if !is_digest_hex(expected_manifest_sha256_arg) {
        return Err(HandoffError::CliArgument {
            detail: "--expected-manifest-sha256 must be 64 lowercase hex characters".to_string(),
        });
    }
    let dir = paths
        .state()
        .join("updates")
        .join("sessions")
        .join(session_id_arg);
    assert_no_reparse(&dir)?;
    if !dir.is_dir() {
        return Err(HandoffError::SessionMissing);
    }

    // B/C. parse the frozen envelope strictly; schema/protocol/state checks.
    if !dir.join(crate::update_session::SESSION_FILE).is_file() {
        return Err(HandoffError::SessionMissing);
    }
    let envelope = load_update_session(&dir).map_err(|e| match e {
        crate::update_session::SessionError::UnsupportedSchema { version } => {
            HandoffError::UnsupportedSchema { version }
        }
        crate::update_session::SessionError::Malformed { detail } => {
            HandoffError::SessionMalformed { detail }
        }
        other => HandoffError::Io {
            detail: other.to_string(),
        },
    })?;
    if envelope.session_id != session_id_arg {
        return Err(HandoffError::StagingIdentityMismatch {
            detail: "session id does not match its canonical directory".to_string(),
        });
    }
    if envelope.operation != SessionOperation::Update {
        return Err(HandoffError::OperationMismatch {
            session: format!("{:?}", envelope.operation),
            requested: "Update".to_string(),
        });
    }
    if envelope.phase != SessionPhase::Staged {
        return Err(HandoffError::InvalidSessionState {
            detail: format!("phase {:?} is not a handoff-validation phase", envelope.phase),
        });
    }
    if envelope.generation == 0 {
        return Err(HandoffError::InvalidSessionState {
            detail: "generation must be monotonically increasing from 1".to_string(),
        });
    }
    // Phase-gated fields: everything belonging to later phases must be
    // absent before validation (frozen: acceptedHealth/commitIntent absent
    // before validation; the rest cannot exist before their phase).
    if envelope.probation_process.is_some()
        || envelope.health_nonce.is_some()
        || envelope.previous_receipt.is_some()
        || envelope.previous_integration.is_some()
        || envelope.accepted_health.is_some()
        || envelope.commit_intent.is_some()
        || envelope.last_error.is_some()
    {
        return Err(HandoffError::InvalidSessionState {
            detail: "phase-gated fields are present before their phase".to_string(),
        });
    }

    // Process identity: PID alone is insufficient — creation time and the
    // exact canonical image identity must be recorded.
    let parent = &envelope.parent_process;
    if parent.pid == 0
        || parent.process_created_at.is_empty()
        || parent.process_created_at.bytes().any(|b| !b.is_ascii_digit())
        || parent.process_created_at.parse::<u64>().is_err()
    {
        return Err(HandoffError::InvalidSessionState {
            detail: "parent process identity is incomplete".to_string(),
        });
    }
    paths::validate_path_syntax(&parent.image_path).map_err(|e| {
        HandoffError::InvalidSessionState {
            detail: format!("parent process image path rejected: {e}"),
        }
    })?;
    if !parent
        .image_path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(MAIN_EXE))
    {
        return Err(HandoffError::InvalidSessionState {
            detail: "parent process image is not the canonical main executable".to_string(),
        });
    }

    // L. install root / staging root snapshots validated against derived
    // locations.
    paths::validate_path_syntax(&envelope.install_root).map_err(|e| {
        HandoffError::StagingIdentityMismatch {
            detail: format!("install root snapshot rejected: {e}"),
        }
    })?;
    paths::validate_path_syntax(&envelope.staging_root).map_err(|e| {
        HandoffError::StagingIdentityMismatch {
            detail: format!("staging root snapshot rejected: {e}"),
        }
    })?;
    if !paths::equal(&envelope.install_root, paths.install()) {
        return Err(HandoffError::StagingIdentityMismatch {
            detail: "session install root does not match the canonical installation".to_string(),
        });
    }
    if !paths::equal(&envelope.staging_root, &dir) {
        return Err(HandoffError::StagingIdentityMismatch {
            detail: "session staging root does not match the derived session directory"
                .to_string(),
        });
    }

    // Installation binding: the session must belong to this installation.
    let receipt = Receipt::load(paths).map_err(|e| HandoffError::InstallationInvalid {
        detail: format!("receipt unusable: {e}"),
    })?
    .ok_or(HandoffError::InstallationInvalid {
        detail: "no installation receipt".to_string(),
    })?;
    if receipt.installation_id != envelope.installation_id {
        return Err(HandoffError::StagingIdentityMismatch {
            detail: "session installationId does not match this installation".to_string(),
        });
    }
    if envelope.app_id != APP_ID {
        return Err(HandoffError::SignedFactMismatch {
            field: "appId".to_string(),
        });
    }

    // D. source version rebinding: the session's frozen source baseline must
    // still be the actual installed source version.
    let current = installed_source.to_string();
    if receipt.current_version.as_deref() != Some(current.as_str()) {
        return Err(HandoffError::SourceVersionChanged {
            recorded: receipt.current_version.clone().unwrap_or_default(),
            current: current.clone(),
        });
    }
    if envelope.from_version != current {
        return Err(HandoffError::SourceVersionChanged {
            recorded: envelope.from_version.clone(),
            current: current.clone(),
        });
    }

    // E. the CLI digest binds the session.
    if expected_manifest_sha256_arg != envelope.manifest_sha256 {
        return Err(HandoffError::ExpectedDigestMismatch {
            cli: expected_manifest_sha256_arg.to_string(),
            session: envelope.manifest_sha256.clone(),
        });
    }

    // F/G. re-verify the persisted raw bytes through the shared core with
    // this binary's own compiled trust store.
    let manifest = read_bounded(&dir.join(MANIFEST_FILE), "persisted manifest")?;
    let envelope_bytes = read_bounded(&dir.join(ENVELOPE_FILE), "persisted envelope")?;
    let recomputed = sha256_hex(&manifest);
    if recomputed != envelope.manifest_sha256 {
        return Err(HandoffError::ManifestDigestMismatch {
            expected: envelope.manifest_sha256.clone(),
            actual: recomputed,
        });
    }
    let target = verify_and_parse(trust, &envelope_bytes, &manifest).map_err(|e| {
        HandoffError::SignatureRecovery {
            detail: e.to_string(),
        }
    })?;
    if target.manifest_sha256_hex() != envelope.manifest_sha256 {
        return Err(HandoffError::ManifestDigestMismatch {
            expected: envelope.manifest_sha256.clone(),
            actual: target.manifest_sha256_hex().to_string(),
        });
    }

    // H. target identity.
    let manifest_version = target.manifest().version().to_string();
    if envelope.to_version != manifest_version {
        return Err(HandoffError::TargetVersionMismatch {
            session: envelope.to_version.clone(),
            manifest: manifest_version,
        });
    }

    // I. signed package/installFiles facts == session facts.
    let asset = target.manifest().asset();
    if envelope.package_sha256 != asset.sha256_hex() || envelope.package_size != asset.size() {
        return Err(HandoffError::SignedFactMismatch {
            field: "package".to_string(),
        });
    }
    let signed_files = asset.install_files();
    if envelope.resources.len() != signed_files.len() {
        return Err(HandoffError::SignedFactMismatch {
            field: "resources".to_string(),
        });
    }
    for (resource, entry) in envelope.resources.iter().zip(signed_files.iter()) {
        if resource.identity != entry.identity()
            || resource.new_sha256 != entry.sha256_hex()
            || resource.new_size != entry.size()
            || resource.backup_slot != backup_slot(session_id_arg, entry.identity())
        {
            return Err(HandoffError::SignedFactMismatch {
                field: "resources".to_string(),
            });
        }
    }

    // The staged package: re-hash against the signed digest, then re-run the
    // frozen archive validation through the single shared implementation.
    let package_path = dir.join(PACKAGE_FILE);
    if !package_path.is_file() {
        return Err(HandoffError::PackageMissing);
    }
    assert_no_reparse(&package_path)?;
    let mut package = paths::open_regular(&package_path).map_err(|e| HandoffError::Io {
        detail: format!("staged package unreadable: {e}"),
    })?;
    let (_, package_hash) = hash_stream(&mut package)?;
    if package_hash != envelope.package_sha256 {
        return Err(HandoffError::PackageHashMismatch {
            expected: envelope.package_sha256.clone(),
            actual: package_hash,
        });
    }
    package
        .seek(std::io::SeekFrom::Start(0))
        .map_err(|e| HandoffError::Io { detail: e.to_string() })?;
    crate::package_zip::validate_archive(&mut package).map_err(|e| HandoffError::PackageInvalid {
        detail: format!("{e:?}"),
    })?;
    drop(package);

    // J/K. staged managed executables: exact filenames, exact count, fresh
    // opens, signed size and hash — nothing is trusted between verification
    // and use.
    let staged_dir = dir.join(STAGED_DIR);
    assert_no_reparse(&staged_dir)?;
    let mut actual: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&staged_dir).map_err(|e| HandoffError::Io {
        detail: format!("staging directory unreadable: {e}"),
    })? {
        let entry = entry.map_err(|e| HandoffError::Io { detail: e.to_string() })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type().map_err(|e| HandoffError::Io { detail: e.to_string() })?.is_file() {
            return Err(HandoffError::StagedUnexpectedFile { name });
        }
        actual.push(name);
    }
    let expected_names: Vec<&str> = signed_files.iter().map(|entry| entry.filename()).collect();
    for name in &actual {
        if !expected_names.iter().any(|expected| expected == name) {
            return Err(HandoffError::StagedUnexpectedFile { name: name.clone() });
        }
    }
    let mut staged = Vec::new();
    for entry in signed_files {
        let filename = entry.filename();
        if !actual.iter().any(|name| name == filename) {
            return Err(HandoffError::StagedFileMissing {
                file: filename.to_string(),
            });
        }
        let path = staged_dir.join(filename);
        let mut file = paths::open_regular(&path).map_err(|e| HandoffError::Io {
            detail: format!("staged {filename} unreadable: {e}"),
        })?;
        let (size, hash) = hash_stream(&mut file)?;
        if size != entry.size() {
            return Err(HandoffError::StagedSizeMismatch {
                file: filename.to_string(),
                declared: entry.size(),
                actual: size,
            });
        }
        if hash != entry.sha256_hex() {
            return Err(HandoffError::StagedHashMismatch {
                file: filename.to_string(),
                expected: entry.sha256_hex().to_string(),
                actual: hash,
            });
        }
        staged.push(crate::package_zip::StagedExecutable {
            identity: entry.identity().to_string(),
            filename: filename.to_string(),
            size,
            sha256_hex: hash,
        });
    }

    // M. destination path-safety preconditions: the future replacement
    // destinations (the install-root managed files and the fixed backup
    // slots) must be free of reparse substitution before anything is
    // mutation-ready.
    for entry in signed_files {
        let destination = paths.install().join(entry.filename());
        assert_no_reparse(&destination)?;
    }

    // The verified digest is persisted into the helper's own journal before
    // any destructive work could ever begin.
    let journal = HelperJournal {
        journal_schema_version: 1,
        session_id: session_id_arg.to_string(),
        verified_manifest_sha256: envelope.manifest_sha256.clone(),
        target_version: manifest_version,
        source_version: current,
        install_root: paths.install().to_path_buf(),
        validated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    let journal_bytes = serde_json::to_vec_pretty(&journal).map_err(|e| HandoffError::Io {
        detail: e.to_string(),
    })?;
    paths::atomic_write(&dir.join(HELPER_JOURNAL_FILE), &journal_bytes).map_err(|e| {
        HandoffError::Io {
            detail: e.to_string(),
        }
    })?;

    Ok(ValidatedHandoff {
        session_id: session_id_arg.to_string(),
        manifest_sha256: envelope.manifest_sha256.clone(),
        target_version: journal.target_version,
        source_version: journal.source_version,
        install_root: paths.install().to_path_buf(),
        session_dir: dir,
        envelope,
        target,
        staged,
    })
}

/// The production CLI-facing handoff: strict argument validation, the
/// production compiled trust store (an empty store fails closed until
/// release-signing provisioning), independent installed-source derivation,
/// and full validation. Returns the validated handoff and stops — the
/// caller must not mutate anything with it in this phase.
pub fn run_update_handoff(
    paths: &Paths,
    session_id: &str,
    expected_manifest_sha256: &str,
) -> Result<ValidatedHandoff, Error> {
    let trust = desktop_todo_update_core::production_trust_store();
    let installed = derive_installed_source(paths, &trust)?;
    let validated = validate_update_handoff(
        paths,
        &trust,
        session_id,
        expected_manifest_sha256,
        &installed,
    )?;
    Ok(validated)
}

// --------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::Resource;
    use crate::update_session::ProcessIdentity;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use desktop_todo_update_core::derive_key_id;
    use ed25519_dalek::{Signer, SigningKey};

    /// TEST-ONLY seeds. K1 is the fixture trust root; K2 is deliberately
    /// NOT in any test store, so K2-signed forgeries fail closed. Never
    /// production trust roots, never used for release signing.
    const TEST_SEED: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";
    const UNTRUSTED_TEST_SEED: [u8; 32] = *b"dtw-test-KEY2-ONLY-not-for-relea";

    fn test_trust() -> TrustStore {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap()
    }

    fn sha_of(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    fn exe_payload(seed: u8) -> Vec<u8> {
        (0..64u32).map(|i| (i as u8).wrapping_add(seed)).collect()
    }

    struct Fixture {
        paths: Paths,
        trust: TrustStore,
        session_id: String,
        digest: String,
    }

    /// A complete valid handoff fixture: sandbox installation with receipt
    /// and runtime files, a persisted session directory (raw manifest/
    /// envelope, package ZIP, staged EXEs) and the frozen session envelope
    /// published through the single creation path.
    fn handoff_fixture(tag: &str, target_version: &str) -> Fixture {
        let id = uuid::Uuid::new_v4().to_string();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target")
            .join("handoff-tests")
            .join(format!("{tag}-{id}"));
        let paths = Paths::sandbox(root, &id);
        paths::create_dir(paths.install()).unwrap();
        paths::create_dir(paths.state()).unwrap();

        let exe1 = exe_payload(1);
        let exe2 = exe_payload(2);
        let support = b"support".to_vec();
        let mut package_entries: Vec<(&str, Vec<u8>)> = vec![
            ("desktop-todo-widget.exe", exe1.clone()),
            ("desktop-todo-maintenance.exe", exe2.clone()),
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
            let options =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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
            v = target_version,
            pkg = package.len(),
            psha = sha_of(&package),
            s1 = exe1.len(),
            h1 = sha_of(&exe1),
            s2 = exe2.len(),
            h2 = sha_of(&exe2),
        );
        let manifest_bytes = manifest.into_bytes();
        let signature = SigningKey::from_bytes(&TEST_SEED).sign(&manifest_bytes).to_bytes();
        let envelope_bytes = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
            derive_key_id(&SigningKey::from_bytes(&TEST_SEED).verifying_key().to_bytes()),
            STANDARD.encode(signature)
        )
        .into_bytes();

        let trust = test_trust();
        let target =
            desktop_todo_update_core::verify_and_parse(&trust, &envelope_bytes, &manifest_bytes)
                .expect("fixture target verifies");

        // Sandbox installation: receipt + runtime files at the canonical
        // sandbox install root.
        for name in [crate::MAIN_EXE, crate::HELPER_EXE] {
            std::fs::write(paths.install().join(name), b"installed runtime").unwrap();
        }
        let installed = exe_payload(9);
        let files = [Resource::MainExecutable, Resource::MaintenanceHelper]
            .into_iter()
            .map(|identity| crate::receipt::FileRecord {
                identity,
                version: "1.1.0".to_string(),
                size: installed.len() as u64,
                sha256: sha_of(&installed),
            })
            .collect();
        let mut receipt = Receipt::new(&paths, "1.1.0", files, false);
        receipt.lifecycle_state = Lifecycle::Installed;
        receipt.save(&paths).unwrap();

        // Persisted session directory contents.
        let session_id = uuid::Uuid::new_v4().to_string();
        let dir = paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&session_id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), &manifest_bytes).unwrap();
        std::fs::write(dir.join(ENVELOPE_FILE), &envelope_bytes).unwrap();
        std::fs::write(dir.join(PACKAGE_FILE), &package).unwrap();
        let staged_dir = dir.join(STAGED_DIR);
        std::fs::create_dir_all(&staged_dir).unwrap();
        std::fs::write(staged_dir.join("desktop-todo-widget.exe"), &exe1).unwrap();
        std::fs::write(staged_dir.join("desktop-todo-maintenance.exe"), &exe2).unwrap();

        let digest = target.manifest_sha256_hex().to_string();
        crate::update_session::publish_update_session(
            &target,
            crate::update_session::HandoffFacts {
                session_id: &session_id,
                installation_id: &receipt.installation_id,
                from_version: "1.1.0",
                discovered_via: "GitHub",
                package_url: "https://mirror.invalid/desktop-todo-widget.zip",
                install_root: paths.install(),
                updates_root: &paths.state().join("updates"),
                parent_process: ProcessIdentity {
                    pid: 4242,
                    process_created_at: "133000000000000000".to_string(),
                    image_path: paths.install().join(MAIN_EXE),
                },
            },
        )
        .expect("session envelope publishes");

        Fixture {
            paths,
            trust,
            session_id,
            digest,
        }
    }

    fn installed_version() -> Version {
        Version::parse("1.1.0").unwrap()
    }

    fn validate(fixture: &Fixture) -> Result<ValidatedHandoff, HandoffError> {
        validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &fixture.digest,
            &installed_version(),
        )
    }

    fn rewrite_session(fixture: &Fixture, mutate: impl FnOnce(&mut UpdateSessionEnvelope)) {
        let dir = fixture
            .paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&fixture.session_id);
        let mut envelope = load_update_session(&dir).unwrap();
        mutate(&mut envelope);
        let bytes = serde_json::to_vec_pretty(&envelope).unwrap();
        std::fs::write(dir.join(crate::update_session::SESSION_FILE), bytes).unwrap();
    }

    fn session_dir_of(fixture: &Fixture) -> PathBuf {
        fixture
            .paths
            .state()
            .join("updates")
            .join("sessions")
            .join(&fixture.session_id)
    }

    #[test]
    fn valid_handoff_reaches_ready_to_mutate_and_journals_the_digest() {
        let fixture = handoff_fixture("valid", "1.2.0");
        let validated = validate(&fixture).expect("valid handoff validates");
        assert_eq!(validated.session_id(), fixture.session_id);
        assert_eq!(validated.manifest_sha256(), fixture.digest);
        assert_eq!(validated.target_version(), "1.2.0");
        assert_eq!(validated.source_version(), "1.1.0");
        assert_eq!(validated.staged().len(), 2);
        // The helper journal carries the verified digest, durably.
        let journal = std::fs::read(session_dir_of(&fixture).join(HELPER_JOURNAL_FILE)).unwrap();
        assert!(String::from_utf8_lossy(&journal).contains(&fixture.digest));
    }

    #[test]
    fn wrong_cli_digest_rejected() {
        let fixture = handoff_fixture("cli-digest", "1.2.0");
        let other = "f".repeat(64);
        let error = validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &other,
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::ExpectedDigestMismatch { .. }), "{error}");
    }

    #[test]
    fn malformed_cli_arguments_rejected() {
        let fixture = handoff_fixture("cli-shape", "1.2.0");
        for id in ["not-a-uuid", uuid::Uuid::nil().to_string().as_str()] {
            assert!(matches!(
                validate_update_handoff(&fixture.paths, &fixture.trust, id, &fixture.digest, &installed_version()),
                Err(HandoffError::CliArgument { .. })
            ));
        }
        assert!(matches!(
            validate_update_handoff(&fixture.paths, &fixture.trust, &fixture.session_id, "ABCDEF", &installed_version()),
            Err(HandoffError::CliArgument { .. })
        ));
    }

    #[test]
    fn tampered_session_digest_field_rejected() {
        let fixture = handoff_fixture("session-digest", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.manifest_sha256 = "a".repeat(64);
        });
        // CLI still carries the approved digest; the edited session
        // disagrees with it.
        assert!(matches!(validate(&fixture), Err(HandoffError::ExpectedDigestMismatch { .. })));
    }

    #[test]
    fn tampered_manifest_bytes_rejected() {
        let fixture = handoff_fixture("manifest-bytes", "1.2.0");
        let dir = session_dir_of(&fixture);
        let mut manifest = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
        let last = manifest.len() - 1;
        manifest[last] = b' ';
        std::fs::write(dir.join(MANIFEST_FILE), &manifest).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::ManifestDigestMismatch { .. })));
    }

    #[test]
    fn re_signed_tampered_manifest_still_fails_signature_recovery() {
        let fixture = handoff_fixture("re-signed", "1.2.0");
        // Tamper the manifest AND update every digest binding AND swap the
        // envelope to a signature from a key the compiled store does not
        // trust: the helper still refuses — a session swap can only cause
        // a refusal.
        let dir = session_dir_of(&fixture);
        let mut manifest = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
        manifest.truncate(manifest.len() - 1);
        manifest.push(b' ');
        let signature =
            SigningKey::from_bytes(&UNTRUSTED_TEST_SEED).sign(&manifest).to_bytes();
        let forged_envelope = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
            derive_key_id(&SigningKey::from_bytes(&UNTRUSTED_TEST_SEED).verifying_key().to_bytes()),
            STANDARD.encode(signature)
        );
        std::fs::write(dir.join(MANIFEST_FILE), &manifest).unwrap();
        std::fs::write(dir.join(ENVELOPE_FILE), forged_envelope.as_bytes()).unwrap();
        rewrite_session(&fixture, |envelope| {
            envelope.manifest_sha256 = sha_of(&manifest);
        });
        // The CLI must carry the swapped session's own digest to reach the
        // signature check at all — the binding check fires first, which is
        // itself the frozen order.
        let error = validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &sha_of(&manifest),
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::SignatureRecovery { .. }), "{error}");
    }

    #[test]
    fn tampered_envelope_bytes_rejected() {
        let fixture = handoff_fixture("envelope-bytes", "1.2.0");
        let dir = session_dir_of(&fixture);
        let mut envelope = std::fs::read(dir.join(ENVELOPE_FILE)).unwrap();
        // Corrupt one byte inside the base64 signature payload: the
        // canonical decode/verification must refuse.
        let middle = envelope.len() / 2;
        envelope[middle] = envelope[middle].wrapping_add(1);
        std::fs::write(dir.join(ENVELOPE_FILE), &envelope).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::SignatureRecovery { .. })));
    }

    #[test]
    fn source_version_rebinding_refuses_stale_sessions() {
        let fixture = handoff_fixture("source-rebind", "1.2.0");
        // A rollback/reinstall happened between staging and the handoff.
        let error = validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &fixture.digest,
            &Version::parse("1.0.9").unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::SourceVersionChanged { .. }), "{error}");
    }

    #[test]
    fn target_version_mismatch_rejected() {
        let fixture = handoff_fixture("target-version", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.to_version = "9.9.9".to_string();
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::TargetVersionMismatch { .. })));
    }

    #[test]
    fn session_swapped_into_another_directory_rejected() {
        let fixture_a = handoff_fixture("pair-a", "1.2.0");
        let fixture_b = handoff_fixture("pair-b", "1.2.0");
        // Session A's envelope copied into session B's canonical directory:
        // the id/directory and staging-root bindings cannot be spliced.
        let bytes = std::fs::read(session_dir_of(&fixture_a).join(crate::update_session::SESSION_FILE)).unwrap();
        std::fs::write(
            session_dir_of(&fixture_b).join(crate::update_session::SESSION_FILE),
            bytes,
        )
        .unwrap();
        let error = validate(&fixture_b).unwrap_err();
        assert!(
            matches!(error, HandoffError::StagingIdentityMismatch { .. }),
            "{error}"
        );
    }

    #[test]
    fn cross_session_digest_and_staging_cannot_be_spliced() {
        // Session A's approved digest invoked against session B.
        let fixture_a = handoff_fixture("splice-a", "1.2.0");
        let fixture_b = handoff_fixture("splice-b", "1.3.0");
        let error = validate_update_handoff(
            &fixture_b.paths,
            &fixture_b.trust,
            &fixture_b.session_id,
            &fixture_a.digest,
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::ExpectedDigestMismatch { .. }), "{error}");

        // A staged EXE copied from another session with different content:
        // hash mismatch.
        let other = exe_payload(77);
        std::fs::write(
            session_dir_of(&fixture_b).join(STAGED_DIR).join("desktop-todo-widget.exe"),
            &other,
        )
        .unwrap();
        let error = validate(&fixture_b).unwrap_err();
        assert!(matches!(error, HandoffError::StagedHashMismatch { .. }), "{error}");
    }

    #[test]
    fn staged_executable_tampering_rejected() {
        // Main EXE hash mismatch (same declared size, different bytes).
        let fixture = handoff_fixture("staged-main", "1.2.0");
        let path = session_dir_of(&fixture).join(STAGED_DIR).join("desktop-todo-widget.exe");
        std::fs::write(&path, [0u8; 64]).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::StagedHashMismatch { .. })));

        // Helper EXE size mismatch.
        let fixture = handoff_fixture("staged-helper", "1.2.0");
        let path = session_dir_of(&fixture)
            .join(STAGED_DIR)
            .join("desktop-todo-maintenance.exe");
        std::fs::write(&path, b"short").unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::StagedSizeMismatch { .. })));

        // Missing staged file.
        let fixture = handoff_fixture("staged-missing", "1.2.0");
        std::fs::remove_file(
            session_dir_of(&fixture)
                .join(STAGED_DIR)
                .join("desktop-todo-maintenance.exe"),
        )
        .unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::StagedFileMissing { .. })));

        // Extra staged file.
        let fixture = handoff_fixture("staged-extra", "1.2.0");
        std::fs::write(
            session_dir_of(&fixture).join(STAGED_DIR).join("notes.txt"),
            b"extra",
        )
        .unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::StagedUnexpectedFile { .. })));
    }

    #[test]
    fn tampered_package_rejected() {
        let fixture = handoff_fixture("package-hash", "1.2.0");
        let path = session_dir_of(&fixture).join(PACKAGE_FILE);
        let mut package = std::fs::read(&path).unwrap();
        package[10] ^= 0xff;
        std::fs::write(&path, &package).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::PackageHashMismatch { .. })));
    }

    #[test]
    fn malformed_and_unsupported_sessions_rejected() {
        // Truncated write (torn session).
        let fixture = handoff_fixture("malformed", "1.2.0");
        let path = session_dir_of(&fixture).join(crate::update_session::SESSION_FILE);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::SessionMalformed { .. })));

        let fixture = handoff_fixture("unknown-field", "1.2.0");
        let path = session_dir_of(&fixture).join(crate::update_session::SESSION_FILE);
        let text = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        let injected = text.replace(
            "\"schemaVersion\": 1,",
            "\"schemaVersion\": 1,\n  \"convenienceField\": true,",
        );
        std::fs::write(&path, injected).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::SessionMalformed { .. })));

        let fixture = handoff_fixture("schema", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.schema_version = 2;
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::UnsupportedSchema { .. })));

        let fixture = handoff_fixture("protocol", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.updater_protocol = 2;
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::UnsupportedSchema { .. })));
    }

    #[test]
    fn wrong_phase_operation_and_phase_gated_fields_rejected() {
        let fixture = handoff_fixture("phase", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.phase = SessionPhase::Committed;
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::InvalidSessionState { .. })));

        let fixture = handoff_fixture("operation", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.operation = SessionOperation::Uninstall;
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::OperationMismatch { .. })));

        let fixture = handoff_fixture("phase-gated", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.commit_intent = Some(serde_json::json!({"decision": true}));
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::InvalidSessionState { .. })));

        let fixture = handoff_fixture("nonce-early", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.health_nonce = Some(STANDARD.encode([7u8; 32]));
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::InvalidSessionState { .. })));
    }

    #[test]
    fn parent_process_identity_is_validated_structurally() {
        let fixture = handoff_fixture("parent-pid", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.parent_process.pid = 0;
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::InvalidSessionState { .. })));

        let fixture = handoff_fixture("parent-image", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.parent_process.image_path = fixture.paths.install().join("other.exe");
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::InvalidSessionState { .. })));
    }

    #[test]
    fn install_root_and_staging_root_snapshots_are_re_derived() {
        let fixture = handoff_fixture("roots", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.install_root = PathBuf::from("C:\\Windows");
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::StagingIdentityMismatch { .. })));

        let fixture = handoff_fixture("staging-root", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.staging_root = fixture.paths.install().to_path_buf();
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::StagingIdentityMismatch { .. })));
    }

    #[test]
    fn installation_binding_is_enforced() {
        // A session whose installationId belongs to a different installation
        // is refused.
        let fixture = handoff_fixture("installation", "1.2.0");
        rewrite_session(&fixture, |envelope| {
            envelope.installation_id = uuid::Uuid::new_v4().to_string();
        });
        assert!(matches!(validate(&fixture), Err(HandoffError::StagingIdentityMismatch { .. })));
    }

    #[test]
    fn missing_session_directory_is_a_distinct_refusal() {
        let fixture = handoff_fixture("missing", "1.2.0");
        let error = validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &uuid::Uuid::new_v4().to_string(),
            &fixture.digest,
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::SessionMissing));
    }

    #[test]
    fn reparse_substituted_staging_is_refused() {
        let fixture = handoff_fixture("reparse", "1.2.0");
        let dir = session_dir_of(&fixture);
        let victim = dir.join(STAGED_DIR);
        std::fs::remove_dir_all(&victim).unwrap();
        let target = dir.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        // A junction needs no elevation; symlink_dir would.
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&victim)
            .arg(&target)
            .status()
            .expect("mklink");
        assert!(status.success(), "junction creation failed");
        let error = validate(&fixture).unwrap_err();
        assert!(matches!(error, HandoffError::UnsafePath { .. }), "{error}");
    }

    #[test]
    fn empty_production_trust_store_fails_closed() {
        let fixture = handoff_fixture("production-store", "1.2.0");
        let error = validate_update_handoff(
            &fixture.paths,
            &desktop_todo_update_core::production_trust_store(),
            &fixture.session_id,
            &fixture.digest,
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::SignatureRecovery { .. }), "{error}");
    }

    #[test]
    fn crash_restart_models() {
        // A: staged → prepare → crash → restart → same session validates
        // again (recovery re-derives everything; nothing is cached).
        let fixture = handoff_fixture("crash-a", "1.2.0");
        validate(&fixture).expect("first validation");
        validate(&fixture).expect("post-restart validation");

        // B: a torn session write is never a valid session.
        let fixture = handoff_fixture("crash-b", "1.2.0");
        let path = session_dir_of(&fixture).join(crate::update_session::SESSION_FILE);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::SessionMalformed { .. })));

        // C: source version changed after session creation → refuse.
        let fixture = handoff_fixture("crash-c", "1.2.0");
        let error = validate_update_handoff(
            &fixture.paths,
            &fixture.trust,
            &fixture.session_id,
            &fixture.digest,
            &Version::parse("1.0.5").unwrap(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::SourceVersionChanged { .. }));

        // G: a successful validation grants nothing afterwards — tamper a
        // staged file and the next invocation refuses even though the
        // helper journal from the earlier success still exists.
        let fixture = handoff_fixture("crash-g", "1.2.0");
        validate(&fixture).expect("first validation");
        assert!(session_dir_of(&fixture).join(HELPER_JOURNAL_FILE).is_file());
        std::fs::write(
            session_dir_of(&fixture).join(STAGED_DIR).join("desktop-todo-widget.exe"),
            [7u8; 64],
        )
        .unwrap();
        assert!(matches!(validate(&fixture), Err(HandoffError::StagedHashMismatch { .. })));

        // H: a stale invocation carrying an older session's approved digest
        // is refused.
        let old = handoff_fixture("crash-h-old", "1.2.0");
        let new = handoff_fixture("crash-h-new", "1.3.0");
        let error = validate_update_handoff(
            &new.paths,
            &new.trust,
            &new.session_id,
            &old.digest,
            &installed_version(),
        )
        .unwrap_err();
        assert!(matches!(error, HandoffError::ExpectedDigestMismatch { .. }));
    }

    #[test]
    fn derive_installed_source_fail_closed_paths() {
        let fixture = handoff_fixture("derive", "1.2.0");
        // The sandbox main executable is not a real PE with our product
        // identity: the provisional anchor refuses.
        let error = derive_installed_source(&fixture.paths, &fixture.trust).unwrap_err();
        assert!(matches!(error, HandoffError::InstallationInvalid { .. }), "{error}");

        // Transitional lifecycle is busy, not invalid.
        let mut receipt = Receipt::load(&fixture.paths).unwrap().unwrap();
        receipt.lifecycle_state = Lifecycle::Updating;
        receipt.save(&fixture.paths).unwrap();
        let error = derive_installed_source(&fixture.paths, &fixture.trust).unwrap_err();
        assert!(matches!(error, HandoffError::LifecycleBusy { .. }), "{error}");
    }

    #[test]
    fn helper_stays_network_free() {
        // Structural gate: the helper crate's manifest must not grow any
        // network dependency while the handoff layer exists.
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .unwrap();
        for forbidden in ["reqwest", "ureq", "attohttpc", "curl", "hyper", "tokio"] {
            assert!(
                !manifest.contains(forbidden),
                "maintenance crate must not depend on {forbidden}"
            );
        }
    }
}
