//! The `SignedEnvelopeV1` untrusted authorization document.
//!
//! The envelope is parsed strictly (closed struct, duplicate keys rejected,
//! trailing input rejected) and validates exactly five things: its own
//! schemaVersion, the exact algorithm, the keyId shape, the canonical
//! signature encoding, and the size bound. Parsing the envelope only ever
//! *selects* an already-compiled key and the one frozen algorithm; it cannot
//! import keys, choose algorithms, or carry transport metadata.

use crate::b64::canonical_signature_bytes;
use crate::compiled::ENVELOPE_MAX_BYTES;
use crate::error::{ErrorKind, ProtocolError};
use crate::json::strict_parse;
use crate::trust::is_lowercase_hex_64;
use serde::Deserialize;

/// The frozen envelope wire shape (schema 1). Unknown fields are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelopeV1 {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    algorithm: String,
    #[serde(rename = "keyId")]
    key_id: String,
    signature: String,
}

/// The authorization facts extracted from a validated envelope: which
/// compiled key to use and the exact 64 signature bytes.
#[derive(Debug, Clone)]
pub struct EnvelopeAuth {
    pub key_id: String,
    pub signature: [u8; 64],
}

impl SignedEnvelopeV1 {
    /// Strict-parse and validate an envelope from raw served bytes.
    pub fn parse(bytes: &[u8]) -> Result<EnvelopeAuth, ProtocolError> {
        if bytes.len() > ENVELOPE_MAX_BYTES {
            return Err(ProtocolError::new(
                ErrorKind::EnvelopeTooLarge,
                format!(
                    "envelope is {} bytes, limit is {ENVELOPE_MAX_BYTES}",
                    bytes.len()
                ),
            ));
        }
        let envelope: SignedEnvelopeV1 = strict_parse(bytes)
            .map_err(|e| ProtocolError::new(ErrorKind::EnvelopeMalformed, e.to_string()))?;

        if envelope.schema_version != crate::compiled::SUPPORTED_SCHEMA_VERSION {
            return Err(ProtocolError::new(
                ErrorKind::EnvelopeSchemaUnsupported,
                format!(
                    "envelope schemaVersion {} is not supported",
                    envelope.schema_version
                ),
            ));
        }
        // Exact, case-sensitive algorithm match: unknown algorithms fail
        // closed and can never reach a crypto backend.
        if envelope.algorithm != "Ed25519" {
            return Err(ProtocolError::new(
                ErrorKind::UnsupportedAlgorithm,
                format!(
                    "envelope algorithm {:?} is not supported",
                    envelope.algorithm
                ),
            ));
        }
        if !is_lowercase_hex_64(&envelope.key_id) {
            return Err(ProtocolError::new(
                ErrorKind::KeyIdMalformed,
                "keyId must be 64 lowercase hex characters".to_string(),
            ));
        }
        let signature = canonical_signature_bytes(&envelope.signature)?;
        Ok(EnvelopeAuth {
            key_id: envelope.key_id,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;

    fn valid_envelope() -> Vec<u8> {
        format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}"}}"#,
            "a".repeat(64),
            STANDARD.encode([0u8; 64])
        )
        .into_bytes()
    }

    #[test]
    fn valid_envelope_yields_key_and_signature() {
        let auth = SignedEnvelopeV1::parse(&valid_envelope()).unwrap();
        assert_eq!(auth.signature, [0u8; 64]);
        assert_eq!(auth.key_id, "a".repeat(64));
    }

    #[test]
    fn schema_algorithm_keyid_and_signature_fail_closed() {
        // schemaVersion drift.
        let bytes = String::from_utf8(valid_envelope())
            .unwrap()
            .replace(r#""schemaVersion":1"#, r#""schemaVersion":2"#);
        assert_eq!(
            SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
            ErrorKind::EnvelopeSchemaUnsupported
        );

        // Algorithm is exact: case differences fail closed.
        for algorithm in ["ed25519", "ED25519", "RSA", ""] {
            let bytes = String::from_utf8(valid_envelope())
                .unwrap()
                .replace("Ed25519", algorithm);
            assert_eq!(
                SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
                ErrorKind::UnsupportedAlgorithm,
                "{algorithm:?}"
            );
        }

        // keyId shape.
        for key_id in [
            "ABCDEF".to_string(),
            "a".repeat(63),
            "g".repeat(64),
            String::new(),
        ] {
            let bytes = String::from_utf8(valid_envelope())
                .unwrap()
                .replace(&"a".repeat(64), &key_id);
            assert_eq!(
                SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
                ErrorKind::KeyIdMalformed,
                "{key_id:?}"
            );
        }

        // Noncanonical signature encoding.
        let signature = STANDARD.encode([0u8; 64]);
        let stripped = signature.trim_end_matches('=');
        let bytes = String::from_utf8(valid_envelope())
            .unwrap()
            .replace(&signature, stripped);
        assert_eq!(
            SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
            ErrorKind::SignatureEncoding
        );
    }

    #[test]
    fn unknown_fields_duplicate_keys_and_trailing_rejected() {
        // Unknown field.
        let bytes = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","signature":"{}","providerUrl":"https://example.invalid"}}"#,
            "a".repeat(64),
            STANDARD.encode([0u8; 64])
        );
        assert_eq!(
            SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
            ErrorKind::EnvelopeMalformed
        );

        // Duplicate key.
        let bytes = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","keyId":"{}","keyId":"{}","signature":"{}"}}"#,
            "a".repeat(64),
            "b".repeat(64),
            STANDARD.encode([0u8; 64])
        );
        assert_eq!(
            SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
            ErrorKind::EnvelopeMalformed
        );

        // Trailing input (trailing whitespace is not trailing input).
        let mut bytes = valid_envelope();
        bytes.extend_from_slice(b" ");
        assert!(SignedEnvelopeV1::parse(&bytes).is_ok());
        bytes.extend_from_slice(b"x");
        assert_eq!(
            SignedEnvelopeV1::parse(&bytes).unwrap_err().kind,
            ErrorKind::EnvelopeMalformed
        );
    }

    #[test]
    fn oversized_envelope_is_rejected_before_parsing() {
        let mut bytes = valid_envelope();
        bytes.resize(ENVELOPE_MAX_BYTES + 1, b'x');
        assert_eq!(
            SignedEnvelopeV1::parse(&bytes).unwrap_err().kind,
            ErrorKind::EnvelopeTooLarge
        );
    }

    #[test]
    fn missing_required_field_is_rejected() {
        let bytes = format!(
            r#"{{"schemaVersion":1,"algorithm":"Ed25519","signature":"{}"}}"#,
            STANDARD.encode([0u8; 64])
        );
        assert_eq!(
            SignedEnvelopeV1::parse(bytes.as_bytes()).unwrap_err().kind,
            ErrorKind::EnvelopeMalformed
        );
    }
}
