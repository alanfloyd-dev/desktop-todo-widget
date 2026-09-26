//! Helper-side signed-update verification boundary.
//!
//! The maintenance helper re-verifies every signature and manifest itself
//! through the shared pure protocol core, with its own compiled trust store,
//! over the exact raw served/persisted bytes. A verification claim from the
//! main application is coordination, never evidence: this module exposes no
//! API that could accept one, because `verify_signed_target` takes only raw
//! bytes and a trust store — the same boundary the future handoff
//! validation (`--expected-manifest-sha256`) will sit behind.

use desktop_todo_update_core::{ProtocolError, TrustStore, VerifiedTarget};

/// The helper's own verification over raw bytes: compiled-key lookup,
/// canonical signature decoding, Ed25519 verification over the exact bytes,
/// strict manifest parsing, and semantic validation.
pub fn verify_signed_target(
    trust: &TrustStore,
    envelope_bytes: &[u8],
    manifest_bytes: &[u8],
) -> Result<VerifiedTarget, ProtocolError> {
    desktop_todo_update_core::verify_and_parse(trust, envelope_bytes, manifest_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_todo_update_core::{derive_key_id, ErrorKind};

    /// Decode 64 lowercase hex characters into the raw 32-byte public key.
    fn hex_to_raw_key(text: &str) -> [u8; 32] {
        assert_eq!(text.len(), 64);
        let mut raw = [0u8; 32];
        for (index, pair) in text.as_bytes().chunks(2).enumerate() {
            raw[index] = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        raw
    }

    fn golden(name: &str) -> Vec<u8> {
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../update-core/tests/golden/{name}"
        )))
        .expect("shared golden fixture must exist")
    }

    /// Architecture gate: the helper verifies raw persisted bytes through
    /// its own dependency edge, without any main-application involvement.
    /// The test passes only raw bytes and a compiled trust store built from
    /// the committed TEST-ONLY public key — there is no "verified" input to
    /// mock. (The TEST-ONLY key is never a production trust root; the
    /// production store stays empty until the release-signing provisioning
    /// gate.)
    #[test]
    fn helper_verifies_golden_bytes_independently() {
        let manifest = golden("manifest-A.json");
        let envelope = golden("envelope-A.json");
        let raw_key = hex_to_raw_key(&String::from_utf8(golden("key-1-public.hex")).unwrap());
        let trust = TrustStore::from_raw_keys(&[raw_key]).unwrap();
        // The keyId inside the envelope is the SHA-256 of the raw key —
        // the same derivation the compiled store performs.
        assert!(String::from_utf8(envelope.clone())
            .unwrap()
            .contains(&derive_key_id(&raw_key)));

        let target = verify_signed_target(&trust, &envelope, &manifest).unwrap();
        assert_eq!(target.manifest().version().to_string(), "1.2.0");
        assert_eq!(target.manifest_sha256_hex().len(), 64);

        // The same bytes with one mutation fail under the helper's own
        // verification — a swapped session directory can only cause refusal.
        let mut tampered = manifest.clone();
        tampered.insert(tampered.len() - 1, b' ');
        let error = verify_signed_target(&trust, &envelope, &tampered).unwrap_err();
        assert_eq!(error.kind, ErrorKind::BadSignature);
    }
}
