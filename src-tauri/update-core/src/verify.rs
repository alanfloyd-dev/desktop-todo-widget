//! The frozen verification order (protocol v1, "Canonical signed bytes").
//!
//! The signature covers the exact raw bytes of `update-manifest.json` as
//! served by the source, and nothing else — never the envelope, never a
//! reserialized, re-encoded, or whitespace-normalized document. The API
//! therefore only accepts raw byte slices: there is no code path that could
//! verify a parsed object or a reserialization, because the function never
//! sees one. A failure at any step ends the candidate; no repair,
//! re-encoding, or retry-with-different-bytes path exists.
//!
//! 1. bounded-size check of the raw envelope and manifest bytes
//! 2. strict-parse the envelope; validate schema/algorithm/keyId/canonical
//!    signature encoding
//! 3. locate the compiled trusted public key by exact keyId
//! 4. verify the Ed25519 signature over the raw manifest bytes
//! 5. validate the same bytes' encoding (UTF-8, no BOM, size bound), then
//!    strict-parse the same bytes into the closed manifest structure
//! 6. validate semantics, then freeze: SHA-256 of the raw manifest bytes
//!    plus every identity field

use crate::envelope::SignedEnvelopeV1;
use crate::error::{ErrorKind, ProtocolError};
use crate::manifest::{
    manifest_digest_hex, parse_manifest, validate_manifest, RawManifest, ValidatedManifest,
};
use crate::trust::TrustStore;
use ed25519_dalek::Signature;

/// A fully verified and validated update target, carrying the exact raw
/// manifest bytes the signature was verified over, the parsed manifest, and
/// the frozen manifest digest.
#[derive(Debug)]
pub struct VerifiedTarget {
    raw: RawManifest,
    manifest: ValidatedManifest,
    manifest_sha256: String,
}

impl VerifiedTarget {
    /// The exact raw manifest bytes as served. Persisted for
    /// trusted-target-persisted and re-verified by the helper.
    pub fn raw_bytes(&self) -> &[u8] {
        self.raw.bytes()
    }

    pub fn manifest(&self) -> &ValidatedManifest {
        &self.manifest
    }

    /// The frozen target digest: SHA-256 over the raw manifest bytes,
    /// lowercase hex. This is the `--expected-manifest-sha256` binding value.
    pub fn manifest_sha256_hex(&self) -> &str {
        &self.manifest_sha256
    }
}

/// Run the frozen verification order over raw served bytes.
pub fn verify_and_parse(
    trust: &TrustStore,
    envelope_bytes: &[u8],
    manifest_bytes: &[u8],
) -> Result<VerifiedTarget, ProtocolError> {
    if manifest_bytes.len() > crate::compiled::MANIFEST_MAX_BYTES {
        return Err(ProtocolError::new(
            ErrorKind::ManifestTooLarge,
            format!(
                "manifest is {} bytes, limit is {}",
                manifest_bytes.len(),
                crate::compiled::MANIFEST_MAX_BYTES
            ),
        ));
    }

    // Steps 2: envelope schema, algorithm, keyId, canonical signature bytes.
    let auth = SignedEnvelopeV1::parse(envelope_bytes)?;

    // Step 3: exact compiled-key lookup; unknown keyId fails closed.
    let verifying_key = trust.lookup(&auth.key_id).ok_or_else(|| {
        ProtocolError::new(
            ErrorKind::UntrustedKey,
            format!("keyId {} is not in the compiled trust store", auth.key_id),
        )
    })?;

    // Step 4: the signature is over the exact raw manifest bytes.
    let signature = Signature::from_bytes(&auth.signature);
    trust
        .verify_strict(verifying_key, manifest_bytes, &signature)
        .map_err(|e| ProtocolError::new(ErrorKind::BadSignature, e.to_string()))?;

    // Step 5: encoding validation, then strict parse of the same bytes.
    let raw = RawManifest::from_bytes(manifest_bytes.to_vec())?;
    let manifest = parse_manifest(&raw)?;

    // Step 6: semantics, then the freeze digest.
    let validated = validate_manifest(manifest)?;
    let manifest_sha256 = manifest_digest_hex(&raw);

    Ok(VerifiedTarget {
        raw,
        manifest: validated,
        manifest_sha256,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    // TEST-ONLY key. Never a production trust root; never used for release
    // signing. Deterministic RFC 8032 signing keeps every test fixed.
    pub(crate) const TEST_SEED: [u8; 32] = *b"dtw-test-KEY-ONLY-not-for-releas";

    pub(crate) fn test_trust() -> TrustStore {
        let signing = SigningKey::from_bytes(&TEST_SEED);
        TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap()
    }

    pub(crate) fn sign(manifest: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(&TEST_SEED).sign(manifest).to_bytes()
    }

    pub(crate) fn envelope(key_id: &str, signature: &[u8]) -> Vec<u8> {
        format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{key_id}","signature":"{}"}}"#,
            STANDARD.encode(signature)
        )
        .into_bytes()
    }

    #[test]
    fn wrong_bytes_fail_with_bad_signature() {
        let trust = test_trust();
        let key_id = trust.entries()[0].key_id().to_string();
        let manifest = crate::manifest::tests::manifest_text("1.4.0");
        let signature = sign(manifest.as_bytes());
        // One byte of semantic whitespace: same JSON meaning, different
        // signature context — must fail, never repair.
        let mutated = format!("{manifest} ").into_bytes();
        assert_eq!(
            verify_and_parse(&trust, &envelope(&key_id, &signature), &mutated)
                .unwrap_err()
                .kind,
            ErrorKind::BadSignature
        );
    }

    /// Regression pin (audit F-2B): the manifest size bound is the first
    /// check of the frozen order at the verifier entry itself — even with a
    /// garbage envelope, the failure is ManifestTooLarge, proving the
    /// early-return fires before any envelope parsing.
    #[test]
    fn oversized_manifest_fails_at_the_verifier_entry() {
        let oversized = vec![b'a'; crate::compiled::MANIFEST_MAX_BYTES + 1];
        assert_eq!(
            verify_and_parse(&test_trust(), b"garbage", &oversized)
                .unwrap_err()
                .kind,
            ErrorKind::ManifestTooLarge
        );
    }

    /// Regression pin (audit F-2C): a duplicate key inside the selected
    /// windows-x64 object of a real manifest shape — signed exactly as
    /// served, so the signature itself verifies — is rejected fail-closed by
    /// the closed-struct parse.
    #[test]
    fn duplicate_key_in_selected_asset_fails_closed() {
        let trust = test_trust();
        let key_id = trust.entries()[0].key_id().to_string();
        let manifest = crate::manifest::tests::manifest_text("1.4.0").replace(
            r#""size":3000000,"sha256""#,
            r#""size":3000000,"size":3000000,"sha256""#,
        );
        assert_ne!(manifest, crate::manifest::tests::manifest_text("1.4.0"));
        let signature = sign(manifest.as_bytes());
        assert_eq!(
            verify_and_parse(&trust, &envelope(&key_id, &signature), manifest.as_bytes())
                .unwrap_err()
                .kind,
            ErrorKind::ManifestMalformed
        );
    }
}
