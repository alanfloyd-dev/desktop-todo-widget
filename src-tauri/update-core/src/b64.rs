//! Canonical Base64 validation for signature strings (protocol v1 freeze).
//!
//! The signature text must be RFC 4648 standard-alphabet Base64 with exact
//! canonical padding, no whitespace, decoding to exactly 64 bytes, and the
//! decoded bytes must re-encode to the original string byte-for-byte. Any
//! input a tolerant decoder would accept but that fails this round-trip is
//! rejected; there is no trim, normalize, repair, or accept-and-reencode.

use crate::error::{ErrorKind, ProtocolError};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

/// Decode a protocol signature string under the canonical-Base64 freeze.
/// Returns the 64 raw signature bytes or a `SignatureEncoding` error.
pub fn canonical_signature_bytes(text: &str) -> Result<[u8; 64], ProtocolError> {
    let reject = |detail: String| Err(ProtocolError::new(ErrorKind::SignatureEncoding, detail));

    // No whitespace of any kind, not even a trailing newline.
    if text.bytes().any(|b| b.is_ascii_whitespace()) {
        return reject("signature base64 contains whitespace".into());
    }
    // Padding is required: the length must be a multiple of four.
    if !text.len().is_multiple_of(4) {
        return reject(format!(
            "signature base64 length {} is not a multiple of four (padding required)",
            text.len()
        ));
    }
    // Standard alphabet only, canonical trailing bits, canonical padding.
    let decoded = match STANDARD.decode(text) {
        Ok(bytes) => bytes,
        Err(e) => return reject(format!("signature base64 is not canonical: {e}")),
    };
    let Ok(bytes) = <[u8; 64]>::try_from(decoded.as_slice()) else {
        return reject(format!(
            "signature base64 decodes to {} bytes, expected 64",
            decoded.len()
        ));
    };
    // Canonical round-trip: re-encoding must reproduce the input exactly.
    // This rejects any alternative equivalent encoding the decoder tolerates.
    let reencoded = STANDARD.encode(bytes);
    if reencoded.as_bytes() != text.as_bytes() {
        return reject("signature base64 is not the canonical encoding of its bytes".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_round_trips() {
        let text = STANDARD.encode([0u8; 64]);
        assert_eq!(text.len(), 88);
        assert_eq!(canonical_signature_bytes(&text).unwrap(), [0u8; 64]);
        let text = STANDARD.encode([0xFB; 64]);
        assert_eq!(canonical_signature_bytes(&text).unwrap(), [0xFB; 64]);
    }

    #[test]
    fn missing_or_extra_padding_is_rejected() {
        let canonical = STANDARD.encode([0u8; 64]);
        let stripped = canonical.trim_end_matches('=').to_string();
        assert_eq!(stripped.len(), 86); // would decode under a lenient decoder
        assert!(canonical_signature_bytes(&stripped).is_err());
        let mut extra = canonical.clone();
        extra.push_str("==");
        assert!(canonical_signature_bytes(&extra).is_err());
    }

    #[test]
    fn whitespace_anywhere_is_rejected() {
        let canonical = STANDARD.encode([0u8; 64]);
        for mutated in [
            format!(" {canonical}"),
            format!("{canonical}\n"),
            format!("{canonical} "),
            format!("{canonical}\r\n"),
            format!("\t{canonical}"),
        ] {
            assert!(canonical_signature_bytes(&mutated).is_err(), "{mutated:?}");
        }
    }

    #[test]
    fn url_safe_alphabet_is_rejected() {
        // 0xFB forces '+' and '/' into the standard-alphabet output.
        let canonical = STANDARD.encode([0xFB; 64]);
        assert!(canonical.contains('+') || canonical.contains('/'));
        let url_safe = canonical.replace('+', "-").replace('/', "_");
        assert_ne!(url_safe, canonical);
        assert!(canonical_signature_bytes(&url_safe).is_err());
    }

    #[test]
    fn noncanonical_trailing_bits_are_rejected() {
        // Replace the last data character ('A' = 0) with 'B' (= 1): the final
        // quantum then carries non-zero spare bits, which the strict engine
        // rejects — and the canonical round-trip would reject it too.
        let canonical = STANDARD.encode([0u8; 64]);
        let mut mutated = canonical.clone();
        let last_data = canonical.len() - 2; // first '=' of the tail
        mutated.replace_range(last_data..last_data + 1, "B");
        assert_ne!(mutated, canonical);
        assert!(canonical_signature_bytes(&mutated).is_err());
    }

    #[test]
    fn invalid_characters_are_rejected() {
        let mut invalid = STANDARD.encode([0u8; 64]);
        invalid.replace_range(0..1, "!");
        assert!(canonical_signature_bytes(&invalid).is_err());
    }

    #[test]
    fn wrong_decoded_length_is_rejected() {
        // 63 bytes encode canonically to 84 characters, still a multiple of
        // four, so the length check alone would not catch it.
        let short = STANDARD.encode([0u8; 63]);
        assert_eq!(short.len(), 84);
        assert!(canonical_signature_bytes(&short).is_err());
    }
}
