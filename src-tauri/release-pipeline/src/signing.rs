//! Signing integration (Phase 3B, pipeline step 4).
//!
//! The pipeline never implements cryptography: signing is delegated to the
//! existing offline release signer (`desktop-todo-release-signer`).
//!
//! Two strictly separated modes:
//!
//! - **Production** — `release_signer::sign_manifest`, hard-pinned to the
//!   compiled `production_trust_store()`. While the compiled store is empty
//!   (no real key ceremony yet) every key fails closed before any signing
//!   happens. No test material can enter this path.
//! - **Rehearsal** — an explicit TEST/REHEARSAL mode with a test signing
//!   key and a trust store built from an explicitly supplied test public
//!   key file. The rehearsal store is injected, never compiled; the
//!   resulting artifacts are labeled rehearsal in `facts.json`, and the
//!   publish step refuses them on production provider endpoints. A
//!   rehearsal key id is additionally checked against the compiled
//!   production key ids and refused if it were ever to match.

use std::path::Path;

use desktop_todo_release_signer as signer;
use desktop_todo_update_core::TrustStore;

use crate::{ReleaseMode, PipelineError};

/// The operator-facing outcome of the signing step (public facts only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningOutcome {
    pub mode: ReleaseMode,
    pub key_id: String,
    /// The exact envelope bytes (`update-manifest.json.sig` content).
    pub envelope: Vec<u8>,
    /// SHA-256 of the exact signed manifest bytes.
    pub manifest_sha256_hex: String,
    /// The target version as verified by the signer's mandatory
    /// self-verification round-trip.
    pub version: String,
}

/// Production signing: exact raw manifest bytes through the compiled
/// production trust gate. The operator's raw 32-byte seed file supplies the
/// signing key (never committed, never logged); the gate refuses any key
/// that is not an exact member of the compiled `production_trust_store()` —
/// so while the store is empty (no real key ceremony yet), every key fails
/// closed before any signing happens.
pub fn sign_production(manifest_bytes: &[u8], key_path: &Path) -> Result<SigningOutcome, PipelineError> {
    let key = signer::load_signing_key(key_path)?;
    let outcome = signer::sign_manifest(manifest_bytes, &key)?;
    Ok(SigningOutcome {
        mode: ReleaseMode::Production,
        key_id: outcome.key_id,
        envelope: outcome.envelope,
        manifest_sha256_hex: outcome.manifest_sha256,
        version: outcome.version,
    })
}

/// Rehearsal signing: the test key signs through an explicitly injected
/// TEST trust store (built from the test public key file). The production
/// compiled gate is not touched and cannot be reached from here.
///
/// `key_path` is the raw 32-byte TEST seed file; `trust_public_path` is the
/// raw 32-byte TEST public key that forms the rehearsal trust store. Both
/// are operator-supplied rehearsal material — this crate never generates,
/// stores, or logs key bytes.
pub fn sign_rehearsal(
    manifest_bytes: &[u8],
    key_path: &Path,
    trust_public_path: &Path,
) -> Result<SigningOutcome, PipelineError> {
    let key = signer::load_signing_key(key_path)?;
    let public = signer::load_public_key(trust_public_path)?;
    let trust = TrustStore::from_raw_keys(&[public.to_bytes()]).map_err(|e| {
        PipelineError::Signing {
            detail: format!("rehearsal trust store rejected: {e}"),
            exit_code: 13,
        }
    })?;

    // Safety gate: a rehearsal key id that equals a compiled production
    // trust root is a mode-conflict, not a rehearsal.
    let key_id = signer::key_id_of(&public);
    if desktop_todo_update_core::production_key_ids().contains(&key_id) {
        return Err(PipelineError::ModeConflict {
            detail: format!(
                "the rehearsal key id {key_id} is a compiled production trust root; \
                 rehearsal material must be clearly non-production"
            ),
        });
    }

    let outcome = signer::sign_manifest_for_rehearsal(manifest_bytes, &key, &trust)?;
    Ok(SigningOutcome {
        mode: ReleaseMode::Rehearsal,
        key_id: outcome.key_id,
        envelope: outcome.envelope,
        manifest_sha256_hex: outcome.manifest_sha256,
        version: outcome.version,
    })
}

/// Build the verification trust store for a mode: production uses the
/// actual compiled production trust store; rehearsal uses the explicitly
/// supplied test public key. Read-back verification must verify with the
/// same authority the release was signed for — a rehearsal artifact must
/// never be accepted by the production store and vice versa.
pub fn verification_trust(
    mode: ReleaseMode,
    trust_public_path: Option<&Path>,
) -> Result<TrustStore, PipelineError> {
    match mode {
        ReleaseMode::Production => Ok(desktop_todo_update_core::production_trust_store()),
        ReleaseMode::Rehearsal => {
            let path = trust_public_path.ok_or_else(|| {
                PipelineError::Usage(
                    "--trust-public <path> is required in rehearsal mode".to_string(),
                )
            })?;
            let public = signer::load_public_key(path)?;
            TrustStore::from_raw_keys(&[public.to_bytes()]).map_err(|e| PipelineError::Signing {
                detail: format!("rehearsal trust store rejected: {e}"),
                exit_code: 13,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "release-pipeline-signing-{}-{}-{label}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest_bytes() -> Vec<u8> {
        r#"{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"1.2.0","publishedAt":"2026-10-02T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":1,"assets":{"windows-x64":{"filename":"desktop-todo-widget-v1.2.0-windows-x64.zip","size":3000000,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","installFiles":[{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}]}}}"#.as_bytes().to_vec()
    }

    #[test]
    fn production_mode_fails_closed_while_the_store_is_empty() {
        // The compiled production trust store is empty (no key ceremony
        // yet): production signing must refuse a perfectly valid test key
        // with the signer's typed unprovisioned-key failure (exit 20).
        let dir = temp_dir("prodkey");
        let key_path = dir.join("test.seed");
        let public_path = dir.join("test.pub");
        signer::generate_keypair(&key_path, &public_path).unwrap();
        let key = signer::load_signing_key(&key_path).unwrap();
        let error = signer::sign_manifest(&manifest_bytes(), &key).unwrap_err();
        assert_eq!(error.exit_code(), 20);
        // And through the pipeline's own wrapper, mapped onto Signing.
        let error = sign_production(&manifest_bytes(), &key_path).unwrap_err();
        assert!(
            matches!(error, PipelineError::Signing { .. }),
            "production sign must fail closed: {error}"
        );
        assert_eq!(error.exit_code(), 20);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rehearsal_signing_succeeds_only_with_explicit_test_trust() {
        let dir = temp_dir("rehearsal");
        let key_path = dir.join("test.seed");
        let public_path = dir.join("test.pub");
        let key_id = signer::generate_keypair(&key_path, &public_path).unwrap();
        let outcome = sign_rehearsal(&manifest_bytes(), &key_path, &public_path).unwrap();
        assert_eq!(outcome.mode, ReleaseMode::Rehearsal);
        assert_eq!(outcome.key_id, key_id);
        assert_eq!(outcome.version, "1.2.0");
        // The envelope verifies through the same rehearsal store.
        let trust = verification_trust(ReleaseMode::Rehearsal, Some(&public_path)).unwrap();
        let facts = signer::verify_envelope(&trust, &outcome.envelope, &manifest_bytes()).unwrap();
        assert_eq!(facts.manifest_sha256, outcome.manifest_sha256_hex);
        // Rehearsal verification uses the rehearsal store — the envelope
        // must NOT verify under the (empty) production store.
        let production = verification_trust(ReleaseMode::Production, None).unwrap();
        assert!(signer::verify_envelope(&production, &outcome.envelope, &manifest_bytes()).is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_test_key_never_enters_the_production_store() {
        let dir = temp_dir("isolation");
        let key_path = dir.join("test.seed");
        let public_path = dir.join("test.pub");
        signer::generate_keypair(&key_path, &public_path).unwrap();
        // The compiled production store carries only the real provisioned
        // ceremony key (Phase 5A); a freshly generated rehearsal/test key
        // can never appear in it.
        assert_eq!(desktop_todo_update_core::production_trust_store().len(), 1);
        let public = signer::load_public_key(&public_path).unwrap();
        assert!(!desktop_todo_update_core::production_key_ids()
            .contains(&signer::key_id_of(&public)));
        fs::remove_dir_all(&dir).ok();
    }
}
