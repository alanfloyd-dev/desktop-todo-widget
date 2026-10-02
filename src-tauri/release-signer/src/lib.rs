//! Offline release-signing tool core (Phase 3A operator tooling).
//!
//! This library implements the narrow operations the offline release signer
//! needs, exclusively through the shared protocol core
//! (`desktop-todo-update-core`): it never re-implements canonical Base64,
//! key-id derivation, manifest validation, or verification, and it never
//! reserializes, normalizes, or repairs a signed document. The signature
//! covers the **exact raw manifest bytes** (protocol v1, "Canonical signed
//! bytes").
//!
//! Deliberate boundaries: no network, no provider API, no Tauri, no SQLite,
//! no async runtime. The private key exists only inside the signing key
//! object for the lifetime of the operation; it is never logged, never
//! debug-formatted, and never included in any error value. Private-key
//! storage outside this process is an operational responsibility (see
//! `docs/release-signing.md`).
//!
//! **Trust authority (Phase 3A pre-commit audit):** the production `sign`
//! path is pinned to the actual compiled `production_trust_store()`. The
//! signer's own key is never a trust authority: a sign succeeds only when
//! the derived key id is already a provisioned production trust root, and
//! the produced envelope is then re-verified through that same compiled
//! store. An unprovisioned (or empty-store) key fails closed before any
//! signing or output exists.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use desktop_todo_update_core::{
    derive_key_id, sha256_hex, validate_untrusted_manifest, verify_and_parse, ErrorKind,
    ProtocolError, TrustStore,
};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};

/// The frozen envelope filename convention (`update-manifest.json.sig`).
pub const ENVELOPE_FILENAME: &str = "update-manifest.json.sig";

/// A raw Ed25519 private seed is exactly 32 bytes. No other encoding is
/// accepted: the signer defines one unambiguous key-file format and fails
/// closed on anything else.
const KEY_BYTES: usize = 32;

/// `FILE_ATTRIBUTE_REPARSE_POINT` — a key file behind a symlink/junction or
/// any other reparse point is refused.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400_0000;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// Typed tool failures (§15 of the Phase 3A round). Every failure maps to a
/// distinct non-zero exit code, and no `Display` output ever contains key
/// material.
#[derive(Debug)]
pub enum ToolError {
    /// The manifest file could not be read (missing, unreadable, I/O).
    ManifestRead { path: PathBuf, detail: String },
    /// The manifest failed the frozen encoding/parse/semantic validation.
    ManifestInvalid(ProtocolError),
    /// The key file could not be read.
    KeyRead { path: PathBuf, detail: String },
    /// The key file is not exactly 32 raw bytes, is not a regular file, or
    /// sits behind a symlink/reparse point.
    KeyFormat { path: PathBuf, detail: String },
    /// The Ed25519 signing operation itself failed.
    SigningFailure { detail: String },
    /// Creating/publishing the output file failed.
    OutputCreate { path: PathBuf, detail: String },
    /// The mandatory post-signing self-verification failed.
    SelfVerification(ProtocolError),
    /// The output path already exists and `--overwrite` was not given.
    OutputCollision { path: PathBuf },
    /// The derived signing key id is not provisioned in the compiled
    /// production trust store. The signer's own key is never a trust
    /// authority: its PUBLIC key must be provisioned and committed first.
    /// The detail carries only the public key id, never private material.
    UnprovisionedKey { key_id: String },
    /// The `verify` subcommand's verification failed.
    Verification(ProtocolError),
    /// A package/EXE fact input file could not be read or hashed.
    FactsRead { path: PathBuf, detail: String },
    /// Command-line usage error.
    Usage(String),
}

impl ToolError {
    /// Distinct non-zero exit codes per failure class.
    pub fn exit_code(&self) -> i32 {
        match self {
            ToolError::Usage(_) => 2,
            ToolError::ManifestRead { .. } => 10,
            ToolError::ManifestInvalid(_) => 11,
            ToolError::KeyRead { .. } => 12,
            ToolError::KeyFormat { .. } => 13,
            ToolError::SigningFailure { .. } => 14,
            ToolError::OutputCreate { .. } => 15,
            ToolError::SelfVerification(_) => 16,
            ToolError::OutputCollision { .. } => 17,
            ToolError::UnprovisionedKey { .. } => 20,
            ToolError::Verification(_) => 18,
            ToolError::FactsRead { .. } => 19,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::ManifestRead { path, detail } => {
                write!(f, "manifest read failed ({}): {detail}", path.display())
            }
            ToolError::ManifestInvalid(e) => write!(f, "manifest is invalid: {e}"),
            ToolError::KeyRead { path, detail } => {
                write!(f, "key file read failed ({}): {detail}", path.display())
            }
            ToolError::KeyFormat { path, detail } => {
                write!(f, "key file rejected ({}): {detail}", path.display())
            }
            ToolError::SigningFailure { detail } => write!(f, "signing failed: {detail}"),
            ToolError::OutputCreate { path, detail } => {
                write!(f, "output write failed ({}): {detail}", path.display())
            }
            ToolError::SelfVerification(e) => {
                write!(f, "self-verification of the signed envelope failed: {e}")
            }
            ToolError::OutputCollision { path } => write!(
                f,
                "output already exists ({}); pass --overwrite to replace it",
                path.display()
            ),
            ToolError::UnprovisionedKey { key_id } => write!(
                f,
                "signing key {key_id} is not provisioned in the compiled production trust store; \
                 provision its PUBLIC key first (docs/release-signing.md)"
            ),
            ToolError::Verification(e) => write!(f, "verification failed: {e}"),
            ToolError::FactsRead { path, detail } => {
                write!(
                    f,
                    "package facts read failed ({}): {detail}",
                    path.display()
                )
            }
            ToolError::Usage(detail) => write!(f, "usage error: {detail}"),
        }
    }
}

impl std::error::Error for ToolError {}

/// The operator-facing summary of a successful sign operation. Contains
/// only public facts.
#[derive(Debug)]
pub struct SignOutcome {
    pub key_id: String,
    pub envelope: Vec<u8>,
    pub manifest_sha256: String,
    pub version: String,
    pub package_filename: String,
    pub package_size: u64,
    pub package_sha256: String,
}

/// The operator-facing summary of a successful verification.
#[derive(Debug)]
pub struct VerifiedFacts {
    pub key_id: String,
    pub manifest_sha256: String,
    pub version: String,
    pub package_filename: String,
    pub package_size: u64,
    pub package_sha256: String,
}

/// Read exactly [`KEY_BYTES`] raw bytes from a regular, non-reparse local
/// file. The read is bounded (an oversized file is rejected without being
/// slurped), no symlink/reparse indirection is accepted, and the bytes are
/// returned only through this function's single caller-controlled path.
fn load_raw_key_file(path: &Path) -> Result<[u8; KEY_BYTES], ToolError> {
    let mut file = fs::File::open(path).map_err(|e| ToolError::KeyRead {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    // Metadata from the open handle, not the path: no TOCTOU re-read, and
    // the checks apply to the file actually read.
    let metadata = file.metadata().map_err(|e| ToolError::KeyRead {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: "key file is a symlink; only regular local files are accepted".to_string(),
        });
    }
    #[cfg(windows)]
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: "key file carries a reparse point; only regular local files are accepted"
                .to_string(),
        });
    }
    if !metadata.is_file() {
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: "key file is not a regular file".to_string(),
        });
    }
    // Bounded read: one byte more than the exact size is enough to detect
    // any oversized input without reading it.
    let mut bounded = (&mut file).take((KEY_BYTES + 1) as u64);
    let mut bytes = Vec::with_capacity(KEY_BYTES);
    bounded
        .read_to_end(&mut bytes)
        .map_err(|e| ToolError::KeyRead {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    if bytes.len() > KEY_BYTES {
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: format!(
                "key file must be exactly {KEY_BYTES} raw bytes (found more than {KEY_BYTES})"
            ),
        });
    }
    if bytes.len() < KEY_BYTES {
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: format!(
                "key file must be exactly {KEY_BYTES} raw bytes (found {})",
                bytes.len()
            ),
        });
    }
    let Ok(raw) = <[u8; KEY_BYTES]>::try_from(bytes.as_slice()) else {
        // Unreachable after the length checks; kept as a typed failure
        // rather than a panic.
        return Err(ToolError::KeyFormat {
            path: path.to_path_buf(),
            detail: "key file length check failed".to_string(),
        });
    };
    Ok(raw)
}

/// Load the private signing seed from an explicit local file (exactly 32
/// raw bytes). The seed never escapes this function's return value as text.
pub fn load_signing_key(path: &Path) -> Result<SigningKey, ToolError> {
    let raw = load_raw_key_file(path)?;
    Ok(SigningKey::from_bytes(&raw))
}

/// Load a raw 32-byte public key from an explicit local file.
pub fn load_public_key(path: &Path) -> Result<VerifyingKey, ToolError> {
    let raw = load_raw_key_file(path)?;
    VerifyingKey::from_bytes(&raw).map_err(|e| ToolError::KeyFormat {
        path: path.to_path_buf(),
        detail: format!("public key material invalid: {e}"),
    })
}

/// The derived protocol key id (lowercase SHA-256 hex of the raw public key).
pub fn key_id_of(verifying_key: &VerifyingKey) -> String {
    derive_key_id(&verifying_key.to_bytes())
}

/// Generate an Ed25519 keypair for the operator's **offline** key ceremony
/// (entropy from the OS CSPRNG). Writes the raw 32-byte public key and the
/// raw 32-byte seed to the two explicit output paths (no overwrite) and
/// returns the derived key id. The seed file is the operator's to store
/// offline; this tool makes no copies.
///
/// Write order is deliberate: the public key is published first, so a
/// failed seed write leaves at most a harmless public file — never a
/// partial usable signing key.
pub fn generate_keypair(seed_out: &Path, public_out: &Path) -> Result<String, ToolError> {
    let mut seed = [0u8; KEY_BYTES];
    getrandom::fill(&mut seed).map_err(|e| ToolError::SigningFailure {
        detail: format!("entropy source unavailable: {e}"),
    })?;
    let signing = SigningKey::from_bytes(&seed);
    let public = signing.verifying_key().to_bytes();
    write_atomic(public_out, &public, false)?;
    write_atomic(seed_out, &seed, false)?;
    Ok(derive_key_id(&public))
}

/// Build the frozen envelope document for a signature: compact JSON in the
/// frozen field order, canonical padded Base64, no trailing newline. The
/// values (64 lowercase hex chars, standard Base64) never need escaping.
fn envelope_document(key_id: &str, signature: &[u8; 64]) -> Vec<u8> {
    format!(
        r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{key_id}","signature":"{}"}}"#,
        STANDARD.encode(signature)
    )
    .into_bytes()
}

/// Sign the **exact raw manifest bytes** under the production release gate:
/// the trust authority is the actual compiled `production_trust_store()`
/// shared by the product binary and the maintenance helper — never the
/// signer's own key. A successful sign therefore proves, in order:
///
/// 1. the manifest passes the full frozen validation (before any signing),
/// 2. the derived key id is an **actual provisioned production trust root**
///    (exact membership, before any signing),
/// 3. the envelope cryptographically verifies over the exact raw bytes
///    under that same compiled store (the production-equivalent verifier),
/// 4. the frozen digest agrees with an independent recomputation.
///
/// An unprovisioned key — including any valid-but-untrusted Ed25519 key, and
/// the entirely-empty store before real provisioning — fails closed with a
/// typed error and produces no output.
pub fn sign_manifest(manifest_bytes: &[u8], key: &SigningKey) -> Result<SignOutcome, ToolError> {
    sign_manifest_with_trust(
        manifest_bytes,
        key,
        &desktop_todo_update_core::production_trust_store(),
    )
}

/// **TEST/REHEARSAL-ONLY** signing entry point for the release pipeline's
/// explicit rehearsal mode (Phase 3B): identical gates with an operator- or
/// test-supplied trust store. This is *not* a production path and never can
/// become one implicitly:
///
/// - the production CLI (`sign` subcommand) and the pipeline's production
///   mode both go through [`sign_manifest`], which is hard-pinned to the
///   compiled `production_trust_store()`;
/// - a rehearsal store built around a TEST key can never pass that compiled
///   gate, and no production binary ever links a rehearsal store;
/// - callers must label the produced artifacts as rehearsal (the pipeline
///   records the mode in `facts.json` and refuses rehearsal artifacts on
///   production provider endpoints).
///
/// This wrapper widens nothing inside the signer: it only exposes the same
/// injected-store core the unit tests exercise.
pub fn sign_manifest_for_rehearsal(
    manifest_bytes: &[u8],
    key: &SigningKey,
    trust: &TrustStore,
) -> Result<SignOutcome, ToolError> {
    sign_manifest_with_trust(manifest_bytes, key, trust)
}

/// Testable signing core: identical gates with an explicitly injected trust
/// store. Crate-internal on purpose — the production CLI is fixed to
/// [`sign_manifest`], which pins the real compiled production store; only
/// test fixtures may inject another store.
pub(crate) fn sign_manifest_with_trust(
    manifest_bytes: &[u8],
    key: &SigningKey,
    trust: &TrustStore,
) -> Result<SignOutcome, ToolError> {
    // Gate 1 — fail before signing: the exact bytes must pass the full
    // frozen encoding/parse/semantic validation.
    validate_untrusted_manifest(manifest_bytes).map_err(ToolError::ManifestInvalid)?;

    // Gate 2 — trust gate before any signing: the derived key id must be an
    // exact member of the caller-pinned trust store. There is no prefix
    // matching, no case folding, and no fallback to a store built from this
    // operation's own key: wrong-but-valid Ed25519 keys are not trusted.
    let verifying_key = key.verifying_key();
    let key_id = derive_key_id(&verifying_key.to_bytes());
    if trust.lookup(&key_id).is_none() {
        return Err(ToolError::UnprovisionedKey {
            key_id: key_id.clone(),
        });
    }

    // Deterministic Ed25519 (RFC 8032) over the exact raw bytes.
    let signature = key.sign(manifest_bytes).to_bytes();
    let envelope = envelope_document(&key_id, &signature);

    // Gate 3 — mandatory self-verification through the same trust store
    // (the compiled production authority in the production path), running
    // the frozen verification order the clients themselves run.
    let target =
        verify_and_parse(trust, &envelope, manifest_bytes).map_err(ToolError::SelfVerification)?;

    // Gate 4 — assert selected target facts: the verifier's frozen digest
    // must equal an independently computed digest of the exact bytes signed.
    let manifest_sha256 = sha256_hex(manifest_bytes);
    if target.manifest_sha256_hex() != manifest_sha256 {
        return Err(ToolError::SelfVerification(ProtocolError::new(
            ErrorKind::BadSignature,
            "verified manifest digest disagrees with the signed bytes",
        )));
    }

    let manifest = target.manifest();
    Ok(SignOutcome {
        key_id,
        envelope,
        manifest_sha256,
        version: manifest.version().to_string(),
        package_filename: manifest.asset().filename().to_string(),
        package_size: manifest.asset().size(),
        package_sha256: manifest.asset().sha256_hex().to_string(),
    })
}

/// Production-equivalent verification of a manifest/envelope pair against a
/// caller-supplied trust store (the `verify` subcommand's operation).
pub fn verify_envelope(
    trust: &TrustStore,
    envelope_bytes: &[u8],
    manifest_bytes: &[u8],
) -> Result<VerifiedFacts, ToolError> {
    let target =
        verify_and_parse(trust, envelope_bytes, manifest_bytes).map_err(ToolError::Verification)?;
    let manifest = target.manifest();
    Ok(VerifiedFacts {
        key_id: trust
            .entries()
            .iter()
            .map(|entry| entry.key_id().to_string())
            .collect::<Vec<_>>()
            .join(","),
        manifest_sha256: target.manifest_sha256_hex().to_string(),
        version: manifest.version().to_string(),
        package_filename: manifest.asset().filename().to_string(),
        package_size: manifest.asset().size(),
        package_sha256: manifest.asset().sha256_hex().to_string(),
    })
}

/// Build the trust store for verification from exactly one raw public key.
pub fn trust_from_public_key(verifying_key: &VerifyingKey) -> Result<TrustStore, ToolError> {
    TrustStore::from_raw_keys(&[verifying_key.to_bytes()]).map_err(|e| ToolError::KeyFormat {
        path: PathBuf::from("<derived public key>"),
        detail: e.to_string(),
    })
}

/// Streaming SHA-256 and byte size of one explicit file. Used for the
/// `package-facts` subcommand; never scans directories.
pub fn file_facts(path: &Path) -> Result<(u64, String), ToolError> {
    use sha2::Digest;
    let mut file = fs::File::open(path).map_err(|e| ToolError::FactsRead {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    let mut hasher = sha2::Sha256::new();
    let mut size: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|e| ToolError::FactsRead {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        size = size.saturating_add(read as u64);
    }
    Ok((size, hex_lower(&hasher.finalize())))
}

/// Lowercase hex of arbitrary bytes (the protocol digest form). The protocol
/// core keeps its hex helper crate-private, so the tool carries this tiny
/// local formatting function — no second digest implementation, only hex
/// formatting of an already-computed SHA-256.
fn hex_lower(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from_digit((byte >> 4) as u32, 16).expect("hex digit"));
        text.push(char::from_digit((byte & 0xf) as u32, 16).expect("hex digit"));
    }
    text
}

/// Atomic output publish: write a sibling temp file, then rename onto the
/// destination. Existing outputs are never silently overwritten; with
/// `overwrite` the destination is removed before the rename.
pub fn write_atomic(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), ToolError> {
    if path.exists() && !overwrite {
        return Err(ToolError::OutputCollision {
            path: path.to_path_buf(),
        });
    }
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    let temp = parent.join(format!(
        "{}.signer-tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let publish = |result: std::io::Result<()>| {
        result.map_err(|e| ToolError::OutputCreate {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })
    };
    publish(fs::write(&temp, bytes))?;
    if overwrite {
        let _ = fs::remove_file(path);
    }
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(ToolError::OutputCreate {
            path: path.to_path_buf(),
            detail: e.to_string(),
        });
    }
    Ok(())
}

/// Read a manifest with the compiled bound applied *at read time* (the
/// signer never slurps an unbounded file).
pub fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, ToolError> {
    let mut file = fs::File::open(path).map_err(|e| ToolError::ManifestRead {
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    let mut bytes = Vec::new();
    (&mut file)
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| ToolError::ManifestRead {
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// TEST-ONLY / NON-PRODUCTION. Deterministic RFC 8032 fixture seed so
    /// every test is fixed. Never a production trust root; never used for
    /// release signing; never compiled into any production binary.
    const TEST_SEED: [u8; 32] = *b"dtw-signer-TEST-ONLY-not4release";

    #[test]
    fn test_seed_is_exactly_32_bytes() {
        assert_eq!(TEST_SEED.len(), 32);
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-signer-test-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_key_file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// A trust store provisioned with exactly the TEST-ONLY fixture key.
    /// Test-fixture injection only — the production CLI always goes through
    /// `sign_manifest`, which pins the real compiled production store.
    fn test_trust() -> TrustStore {
        TrustStore::from_raw_keys(&[SigningKey::from_bytes(&TEST_SEED)
            .verifying_key()
            .to_bytes()])
        .unwrap()
    }

    /// The frozen example manifest from docs/maintenance-protocol-v1.md.
    fn manifest_bytes() -> Vec<u8> {
        r#"{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.2.0","publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"assets":{"windows-x64":{"filename":"desktop-todo-widget-v1.2.0-windows-x64.zip","size":3000000,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","installFiles":[{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}]}}}"#.as_bytes().to_vec()
    }

    #[test]
    fn signing_is_deterministic_and_self_verified() {
        let key = SigningKey::from_bytes(&TEST_SEED);
        let first = sign_manifest_with_trust(&manifest_bytes(), &key, &test_trust()).unwrap();
        let second = sign_manifest_with_trust(&manifest_bytes(), &key, &test_trust()).unwrap();
        assert_eq!(first.envelope, second.envelope);

        // The envelope is the frozen shape: canonical padded base64, no
        // whitespace, no trailing newline, exactly 64 signature bytes.
        let text = String::from_utf8(first.envelope.clone()).unwrap();
        assert!(!text.contains(char::is_whitespace));
        assert!(text.contains(r#""schemaVersion":1"#));
        assert!(text.contains(r#""algorithm":"Ed25519""#));
        assert!(text.contains(&first.key_id));
        let auth =
            desktop_todo_update_core::SignedEnvelopeV1::parse(first.envelope.as_slice()).unwrap();
        assert_eq!(auth.signature.len(), 64);
        assert_eq!(auth.key_id, first.key_id);

        // Summary facts come from the verified target.
        assert_eq!(first.version, "1.2.0");
        assert_eq!(
            first.package_filename,
            "desktop-todo-widget-v1.2.0-windows-x64.zip"
        );
        assert_eq!(first.package_size, 3_000_000);
        assert_eq!(first.manifest_sha256.len(), 64);
    }

    #[test]
    fn envelope_round_trips_through_production_verify() {
        let key = SigningKey::from_bytes(&TEST_SEED);
        let outcome = sign_manifest_with_trust(&manifest_bytes(), &key, &test_trust()).unwrap();
        let trust = test_trust();
        let facts = verify_envelope(&trust, &outcome.envelope, &manifest_bytes()).unwrap();
        assert_eq!(facts.version, "1.2.0");
        assert_eq!(facts.manifest_sha256, outcome.manifest_sha256);
    }

    #[test]
    fn one_byte_and_whitespace_manifest_mutations_fail_self_verification() {
        let key = SigningKey::from_bytes(&TEST_SEED);
        let original = manifest_bytes();
        let mutations: Vec<(&str, Vec<u8>)> = vec![
            ("trailing space", {
                let mut m = original.clone();
                m.push(b' ');
                m
            }),
            ("trailing newline", {
                let mut m = original.clone();
                m.push(b'\n');
                m
            }),
            ("crlf ending", {
                let mut m = original.clone();
                m.push(b'\r');
                m.push(b'\n');
                m
            }),
            ("one-byte value change", {
                let mut m = original.clone();
                let last = m.len() - 1;
                m[last] = m[last].wrapping_add(1);
                m
            }),
        ];
        for (label, mutated) in mutations {
            assert_ne!(mutated, original, "{label}");
            // An unchanged envelope against mutated bytes fails at the
            // verifier; re-signing mutated bytes succeeds but self-verify
            // then pins the digest of the mutated bytes — so the primary
            // guarantee exercised here is: verification binds to exact
            // bytes, never to meaning.
            let trust = test_trust();
            let outcome = sign_manifest_with_trust(&original, &key, &trust).unwrap();
            let error = verify_envelope(&trust, &outcome.envelope, &mutated).unwrap_err();
            assert!(matches!(error, ToolError::Verification(_)), "{label}");
        }
    }

    #[test]
    fn invalid_manifest_fails_before_signing() {
        let key = SigningKey::from_bytes(&TEST_SEED);
        // Unknown top-level field → closed-parse rejection, typed.
        let text = String::from_utf8(manifest_bytes())
            .unwrap()
            .replace(r#""schemaVersion":1,"#, r#""schemaVersion":1,"extra":1,"#);
        let error = sign_manifest_with_trust(text.as_bytes(), &key, &test_trust()).unwrap_err();
        assert!(matches!(error, ToolError::ManifestInvalid(_)));
        assert_eq!(error.exit_code(), 11);

        // Invalid version grammar → typed version failure.
        let text = String::from_utf8(manifest_bytes())
            .unwrap()
            .replace(r#""version":"1.2.0""#, r#""version":"1.2.0-beta""#);
        let error = sign_manifest_with_trust(text.as_bytes(), &key, &test_trust()).unwrap_err();
        assert!(matches!(error, ToolError::ManifestInvalid(_)));
    }

    #[test]
    fn wrong_key_fails_verification() {
        let key = SigningKey::from_bytes(&TEST_SEED);
        let outcome = sign_manifest_with_trust(&manifest_bytes(), &key, &test_trust()).unwrap();
        // TEST-ONLY second key: a trust store that does not contain the
        // signing key must reject the envelope (untrusted, fail closed).
        let other = SigningKey::from_bytes(&[9u8; 32]);
        let trust = TrustStore::from_raw_keys(&[other.verifying_key().to_bytes()]).unwrap();
        let error = verify_envelope(&trust, &outcome.envelope, &manifest_bytes()).unwrap_err();
        assert!(matches!(error, ToolError::Verification(_)));
    }

    #[test]
    fn malformed_key_files_are_typed_rejections() {
        let dir = temp_dir("keyfiles");
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("empty.key", Vec::new(), "short"),
            ("short.key", vec![1u8; 31], "short"),
            ("long.key", vec![1u8; 33], "oversize"),
            ("huge.key", vec![1u8; 4097], "oversize"),
        ];
        for (name, bytes, _label) in cases {
            let path = write_key_file(&dir, name, &bytes);
            let error = load_signing_key(&path).unwrap_err();
            assert!(matches!(error, ToolError::KeyFormat { .. }), "{name}");
            assert_eq!(error.exit_code(), 13);
        }
        // A directory is refused — on Windows the open itself fails
        // (typed read failure); on platforms where it opens, the
        // regular-file check rejects it. Either way it never loads.
        let error = load_signing_key(&dir).unwrap_err();
        assert!(matches!(
            error,
            ToolError::KeyRead { .. } | ToolError::KeyFormat { .. }
        ));
        // Missing file is a read failure.
        let error = load_signing_key(&dir.join("absent.key")).unwrap_err();
        assert!(matches!(error, ToolError::KeyRead { .. }));
        assert_eq!(error.exit_code(), 12);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn output_collision_is_refused_unless_overwritten() {
        let dir = temp_dir("collision");
        let out = dir.join("update-manifest.json.sig");
        fs::write(&out, b"existing").unwrap();
        let error = write_atomic(&out, b"new", false).unwrap_err();
        assert!(matches!(error, ToolError::OutputCollision { .. }));
        assert_eq!(error.exit_code(), 17);
        assert_eq!(fs::read(&out).unwrap(), b"existing");
        write_atomic(&out, b"new", true).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"new");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_keypair_writes_exact_length_files_and_refuses_overwrite() {
        let dir = temp_dir("generate");
        let seed = dir.join("prod.key");
        let public = dir.join("prod.pub");
        let key_id = generate_keypair(&seed, &public).unwrap();
        assert_eq!(key_id.len(), 64);
        assert_eq!(fs::read(&seed).unwrap().len(), 32);
        assert_eq!(fs::read(&public).unwrap().len(), 32);
        // The derived id matches the public file's contents.
        let verifying = load_public_key(&public).unwrap();
        assert_eq!(key_id_of(&verifying), key_id);
        // No silent overwrite.
        assert!(matches!(
            generate_keypair(&seed, &public).unwrap_err(),
            ToolError::OutputCollision { .. }
        ));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn file_facts_stream_hash_and_size() {
        let dir = temp_dir("facts");
        let path = dir.join("blob.bin");
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &payload).unwrap();
        let (size, digest) = file_facts(&path).unwrap();
        assert_eq!(size, payload.len() as u64);
        assert_eq!(digest, sha256_hex(&payload));
        assert_eq!(digest.len(), 64);
        let error = file_facts(&dir.join("absent.bin")).unwrap_err();
        assert!(matches!(error, ToolError::FactsRead { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn errors_never_contain_key_material() {
        // Render every failure shape and assert none of them carries the
        // seed bytes, their hex, or their base64 anywhere.
        let seed_hex: String = TEST_SEED.iter().map(|b| format!("{b:02x}")).collect();
        let seed_b64 = STANDARD.encode(TEST_SEED);
        let key = SigningKey::from_bytes(&TEST_SEED);
        let key_id = derive_key_id(&key.verifying_key().to_bytes());
        let dir = temp_dir("leak");
        let texts = vec![
            format!("{}", ToolError::Usage("x".into())),
            format!(
                "{}",
                ToolError::ManifestRead {
                    path: dir.join("m.json"),
                    detail: "nope".into()
                }
            ),
            format!(
                "{}",
                ToolError::KeyRead {
                    path: dir.join("k"),
                    detail: "nope".into()
                }
            ),
            format!(
                "{}",
                ToolError::KeyFormat {
                    path: dir.join("k"),
                    detail: "bad".into()
                }
            ),
            format!("{}", ToolError::SigningFailure { detail: "x".into() }),
            format!(
                "{}",
                ToolError::OutputCollision {
                    path: dir.join("o")
                }
            ),
            format!(
                "{}",
                ToolError::UnprovisionedKey {
                    key_id: key_id.clone()
                }
            ),
            // Signing succeeds; its summary must not leak either.
            format!(
                "{:?}",
                sign_manifest_with_trust(&manifest_bytes(), &key, &test_trust()).unwrap()
            ),
            format!("{key_id}"),
        ];
        for text in texts {
            assert!(!text.contains(&seed_hex), "hex leak: {text}");
            assert!(!text.contains(&seed_b64), "base64 leak: {text}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn production_sign_fails_closed_while_no_key_is_provisioned() {
        // The compiled production trust store is currently empty (no real
        // key ceremony yet). The production sign path must therefore refuse
        // every key — including a perfectly valid Ed25519 key — with a
        // typed failure, before signing anything and with no output.
        let key = SigningKey::from_bytes(&TEST_SEED);
        let error = sign_manifest(&manifest_bytes(), &key).unwrap_err();
        assert!(matches!(error, ToolError::UnprovisionedKey { .. }));
        assert_eq!(error.exit_code(), 20);
        // The message carries only the public key id.
        let text = format!("{error}");
        assert!(text.contains(&derive_key_id(&key.verifying_key().to_bytes())));
    }

    #[test]
    fn no_output_file_survives_a_failed_trust_gate() {
        // The CLI's operation order is: read → sign → write. A trust-gate
        // failure happens inside sign, so no envelope may be published.
        let dir = temp_dir("trustgate");
        let out = dir.join("update-manifest.json.sig");
        let key = SigningKey::from_bytes(&TEST_SEED);
        // Simulate the CLI flow exactly.
        let result = sign_manifest(&manifest_bytes(), &key);
        if let Ok(outcome) = result {
            write_atomic(&out, &outcome.envelope, false).unwrap();
        }
        assert!(!out.exists(), "failed trust gate must leave no output");
        // And the sibling temp file must not linger either.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert!(leftovers.is_empty(), "unexpected leftovers: {leftovers:?}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unprovisioned_valid_ed25519_key_is_rejected() {
        // Key A is provisioned; an entirely different valid Ed25519 key B
        // is not. Signing with B must fail closed — the signer's key is
        // never its own trust authority, and there is no fallback to a
        // store built from B's own public half.
        let key_a = SigningKey::from_bytes(&TEST_SEED);
        let key_b = SigningKey::from_bytes(&[7u8; 32]);
        assert_ne!(
            derive_key_id(&key_a.verifying_key().to_bytes()),
            derive_key_id(&key_b.verifying_key().to_bytes())
        );
        let error = sign_manifest_with_trust(&manifest_bytes(), &key_b, &test_trust()).unwrap_err();
        assert!(matches!(error, ToolError::UnprovisionedKey { .. }));
        assert_eq!(error.exit_code(), 20);
    }

    #[test]
    fn provisioned_test_store_sign_and_actual_trust_verification_succeed() {
        // The full production gate sequence, exercised against the injected
        // TEST-ONLY store: validation → exact membership → sign →
        // self-verification through the SAME store (not an own-key store).
        let key = SigningKey::from_bytes(&TEST_SEED);
        let trust = test_trust();
        let outcome = sign_manifest_with_trust(&manifest_bytes(), &key, &trust).unwrap();
        // The verifying store is the same provisioned one, and the
        // envelope's keyId is exactly the provisioned member.
        assert_eq!(trust.entries().len(), 1);
        assert_eq!(trust.entries()[0].key_id(), outcome.key_id);
        let facts = verify_envelope(&trust, &outcome.envelope, &manifest_bytes()).unwrap();
        assert_eq!(facts.key_id, outcome.key_id);
        assert_eq!(facts.manifest_sha256, outcome.manifest_sha256);
    }

    #[test]
    fn trust_lookup_is_exact_derived_id_membership() {
        // Pin the exactness the gate relies on: the store derives ids
        // itself (no case folding, no aliasing), and the gate compares the
        // exact derived id against exact members.
        let trust = test_trust();
        let key_id = derive_key_id(
            &SigningKey::from_bytes(&TEST_SEED)
                .verifying_key()
                .to_bytes(),
        );
        assert!(trust.lookup(&key_id).is_some());
        assert!(trust.lookup(&key_id.to_uppercase()).is_none());
        assert!(trust.lookup(&key_id[..63].to_string()).is_none());
    }

    #[test]
    fn read_bounded_enforces_the_compiled_bound() {
        let dir = temp_dir("bounded");
        let path = dir.join("m.json");
        fs::write(&path, vec![b'a'; 300 * 1024]).unwrap();
        let bytes = read_bounded(&path, desktop_todo_update_core::MANIFEST_MAX_BYTES).unwrap();
        assert_eq!(
            bytes.len(),
            desktop_todo_update_core::MANIFEST_MAX_BYTES + 1
        );
        // Feeding the oversized read to validation fails closed (the
        // compiled manifest bound, not an unbounded slurp).
        let error = sign_manifest(&bytes, &SigningKey::from_bytes(&TEST_SEED)).unwrap_err();
        assert!(matches!(error, ToolError::ManifestInvalid(_)));
        fs::remove_dir_all(&dir).ok();
    }
}
