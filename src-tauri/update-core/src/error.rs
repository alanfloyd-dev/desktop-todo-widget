//! Closed error taxonomy for the signed-update protocol core.
//!
//! The taxonomy is part of the Phase 2A freeze: callers must be able to
//! distinguish outcomes stably by kind, never by parsing error strings. The
//! candidate planner maps exactly three kinds to *candidate-ineligible*
//! (continue scanning older candidates) and treats every other kind as
//! *candidate-invalid* (reject this candidate, keep scanning).

use std::fmt;

/// A protocol failure with a stable kind and an unstructured human detail.
/// The kind is the contract; the detail is diagnostics only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError {
    pub kind: ErrorKind,
    pub detail: String,
}

impl ProtocolError {
    pub fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)
    }
}

impl std::error::Error for ProtocolError {}

/// Stable kinds, grouped by the Phase 2A error taxonomy
/// (application-lifecycle.md, "Phase 2 error taxonomy").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    // ManifestFormat: bounded-size / encoding / strict-parse failures.
    EnvelopeTooLarge,
    ManifestTooLarge,
    EnvelopeMalformed,
    ManifestMalformed,
    ManifestNotUtf8,
    ManifestBom,
    // Envelope authorization fields.
    EnvelopeSchemaUnsupported,
    UnsupportedAlgorithm,
    KeyIdMalformed,
    SignatureEncoding,
    // Trust and signature.
    UntrustedKey,
    BadSignature,
    // Manifest schema and semantics.
    ManifestSchemaUnsupported,
    UnsupportedUpdaterProtocol,
    VersionMalformed,
    SemanticViolation(SemanticViolation),
    PlatformMismatch,
    // Candidate-policy outcomes (produced by the planner, not verification).
    VersionNotNewer,
    HopIncompatible,
    NoEligibleCandidate,
}

/// Closed sub-taxonomy for manifest semantic violations (step 6 of the
/// frozen verification order). Each variant is a self-contradictory or
/// policy-violating value in an otherwise well-formed manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticViolation {
    AppIdMismatch,
    ChannelUnsupported,
    PublishedAtMalformed,
    PackageSizeOutOfRange,
    PackageSha256Malformed,
    SelectedAssetMalformed,
    InstallFileMissing,
    InstallFileDuplicate,
    InstallFileIdentityUnknown,
    InstallFileFilenameMismatch,
    InstallFileSizeOutOfRange,
    InstallFileSha256Malformed,
    ExpandedSizeOutOfRange,
}

/// Trust-store construction failure. This is a compiled-store corruption
/// condition (for example a SHA-256 key-id collision) and fails closed at
/// startup, never as a per-candidate outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustStoreError {
    DuplicateKeyId { key_id: String },
    InvalidKeyMaterial { detail: String },
}

impl fmt::Display for TrustStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrustStoreError::DuplicateKeyId { key_id } => {
                write!(f, "trust store contains duplicate key id {key_id}")
            }
            TrustStoreError::InvalidKeyMaterial { detail } => {
                write!(f, "trust store key material invalid: {detail}")
            }
        }
    }
}

impl std::error::Error for TrustStoreError {}
