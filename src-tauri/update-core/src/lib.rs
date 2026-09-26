//! Pure signed-update protocol core for Maintenance Protocol 1 (Phase 2B).
//!
//! This crate is the single shared implementation of the wire-level trust
//! rules frozen in [Maintenance protocol v1](../../docs/maintenance-protocol-v1.md):
//! canonical signed bytes, the `SignedEnvelopeV1` schema, the compiled
//! `TrustedKeyStore`, canonical Base64, strict closed parsing, manifest
//! semantic validation, and the bounded candidate eligibility planner. Both
//! the product binary and the offline maintenance helper link this same
//! source and each runs its own verification at runtime; a "verified" claim
//! from one process is coordination, never evidence for the other.
//!
//! Deliberate boundaries: no network, no filesystem, no Tauri, no SQLite, no
//! Win32, no async runtime, and no second version parser — the Phase 1.5
//! canonical comparator lives here now and is re-exported by the maintenance
//! crate. The caller supplies raw served bytes; the API never accepts a
//! parsed or reserialized manifest, so verification can only ever cover the
//! exact bytes the source served.

pub mod b64;
pub mod candidate;
pub mod compiled;
pub mod envelope;
pub mod error;
pub mod json;
pub mod manifest;
pub mod rfc3339;
pub mod trust;
pub mod verify;
pub mod version;

pub use candidate::{
    evaluate_candidate, select_candidates, CandidateInput, CandidateOutcome, CandidateScan,
    IneligibilityReason, RollbackCompatibility, SelectionContext,
};
pub use compiled::{
    identity_filename, production_trust_store, APP_ID, CLIENT_PLATFORM, ENVELOPE_MAX_BYTES,
    EXPANDED_MAX_BYTES, HELPER_EXECUTABLE_FILENAME, MAIN_EXECUTABLE_FILENAME,
    MANAGED_FILE_MAX_BYTES, MANIFEST_MAX_BYTES, PACKAGE_MAX_BYTES, SUPPORTED_CHANNEL,
    SUPPORTED_SCHEMA_VERSION, SUPPORTED_UPDATER_PROTOCOL,
};
pub use envelope::{EnvelopeAuth, SignedEnvelopeV1};
pub use error::{ErrorKind, ProtocolError, SemanticViolation, TrustStoreError};
pub use manifest::{
    InstallFileEntry, InstallIdentity, ManifestV1, PlatformAsset, RawManifest, ValidatedManifest,
};
pub use trust::{derive_key_id, TrustStore};
pub use verify::{verify_and_parse, VerifiedTarget};
pub use version::Version;

/// SHA-256 over exact bytes, lowercase hex — the form every protocol digest
/// takes (manifest digest, keyId derivation).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    trust::hex_lower(&Sha256::digest(bytes))
}
