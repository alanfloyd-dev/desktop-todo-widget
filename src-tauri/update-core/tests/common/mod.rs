//! Shared deterministic fixtures for the signed-update core tests.
//!
//! **TEST-ONLY KEYS.** The seeds below exist to make every test fixed and
//! fully offline (Ed25519 signing is deterministic per RFC 8032). They are
//! not production trust roots, are not compiled into any production trust
//! store, and must never be used to sign a release. The production trust
//! store stays empty until the publisher provisions real keys
//! (`update-core/src/compiled.rs`).

// Each integration-test binary links this module and uses a different
// subset of the helpers.
#![allow(dead_code)]

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
pub use desktop_todo_update_core::derive_key_id;
use desktop_todo_update_core::{CandidateInput, TrustStore};
use ed25519_dalek::{Signer, SigningKey};

pub const TEST_SEED_K1: [u8; 32] = *b"dtw-test-KEY1-ONLY-not-for-relea";
pub const TEST_SEED_K2: [u8; 32] = *b"dtw-test-KEY2-ONLY-not-for-relea";

pub fn signing_k1() -> SigningKey {
    SigningKey::from_bytes(&TEST_SEED_K1)
}

pub fn signing_k2() -> SigningKey {
    SigningKey::from_bytes(&TEST_SEED_K2)
}

/// A store trusting exactly the given test signing keys' public halves.
pub fn trust_with(keys: &[&SigningKey]) -> TrustStore {
    let raw: Vec<[u8; 32]> = keys
        .iter()
        .map(|key| key.verifying_key().to_bytes())
        .collect();
    TrustStore::from_raw_keys(&raw).expect("test keys cannot collide")
}

/// Sign exact manifest bytes (deterministic).
pub fn sign(key: &SigningKey, manifest_bytes: &[u8]) -> [u8; 64] {
    key.sign(manifest_bytes).to_bytes()
}

/// Build an envelope document for a signature, deriving the keyId from the
/// key's raw public half.
pub fn envelope_bytes(key: &SigningKey, signature: &[u8; 64]) -> Vec<u8> {
    format!(
        r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
        derive_key_id(&public_raw(key)),
        STANDARD.encode(signature)
    )
    .into_bytes()
}

pub fn public_raw(key: &SigningKey) -> [u8; 32] {
    key.verifying_key().to_bytes()
}

/// A protocol-valid manifest document with the frozen field set. Synthetic
/// hashes/sizes only; never installable release metadata.
pub fn manifest_text(version: &str, updater_protocol: u32, assets_key: &str) -> String {
    let hash = "a".repeat(64);
    format!(
        concat!(
            r#"{{"schemaVersion":1,"appId":"net.alanfloyd.desktop","channel":"stable","version":"{v}","#,
            r#""publishedAt":"2026-10-01T00:00:00Z","notes":"Application lifecycle management.","updaterProtocol":{p},"#,
            r#""assets":{{"{assets_key}":{{"filename":"desktop-todo-widget-v{v}-windows-x64.zip","size":3000000,"sha256":"{hash}","installFiles":["#,
            r#"{{"identity":"mainExecutable","filename":"desktop-todo-widget.exe","size":5500000,"sha256":"{hash}"}},"#,
            r#"{{"identity":"maintenanceHelper","filename":"desktop-todo-maintenance.exe","size":800000,"sha256":"{hash}"}}]}}}}}}"#
        ),
        v = version,
        p = updater_protocol,
        assets_key = assets_key,
        hash = hash
    )
}

/// A complete candidate: raw manifest bytes signed by `key` plus envelope.
pub fn candidate(
    key: &SigningKey,
    version: &str,
    updater_protocol: u32,
    assets_key: &str,
) -> CandidateInput {
    let manifest_bytes = manifest_text(version, updater_protocol, assets_key).into_bytes();
    let signature = sign(key, &manifest_bytes);
    CandidateInput {
        envelope_bytes: envelope_bytes(key, &signature),
        manifest_bytes,
    }
}

/// A candidate whose manifest bytes were tampered after signing.
pub fn tampered_candidate(key: &SigningKey, version: &str) -> CandidateInput {
    let candidate = candidate(key, version, 1, "windows-x64");
    let mut manifest_bytes = candidate.manifest_bytes;
    manifest_bytes.insert(manifest_bytes.len() - 1, b' ');
    CandidateInput {
        envelope_bytes: candidate.envelope_bytes,
        manifest_bytes,
    }
}
