//! The provisioned production signing keys (PUBLIC material only).
//!
//! This is the single source of truth for the production trust root. It is
//! a small, hand-reviewed, intentionally committed Rust table: each entry
//! carries one raw 32-byte Ed25519 **public** key plus the key id declared
//! at provisioning time. Provisioning is a reviewed release gate
//! (protocol v1, "Trust store and key rotation"): only public material may
//! ever enter this file — a private seed, or any material that is not
//! exactly the 32-byte raw public key, must never be committed here.
//!
//! The offline release signer (`src-tauri/release-signer`) prints the exact
//! table entry for a provisioned public key via its `provision-entry`
//! subcommand; the operator pastes that entry here, reviews the diff, and
//! the consistency tests below pin it: a declared key id that does not
//! equal the derived SHA-256 of the raw bytes, a raw key that is not a
//! valid Ed25519 public key, or a duplicate key id all fail the build's
//! test gate — and the runtime store construction fails closed rather than
//! ever trusting an unvalidated entry.

use crate::error::TrustStoreError;
use crate::trust::{derive_key_id, TrustStore};

/// One provisioned production trust root.
pub(crate) struct ProvisionedKey<'a> {
    /// The key id recorded at provisioning time: lowercase 64-hex
    /// SHA-256 of `raw` (the frozen derivation). Declared separately so a
    /// provisioning mistake is caught by the consistency tests, not only
    /// silently re-derived.
    pub declared_key_id: &'a str,
    /// The raw 32-byte Ed25519 public key. PUBLIC material only.
    pub raw: [u8; 32],
}

/// The provisioned production keys, in provisioning order.
///
/// **Currently empty (fail-closed):** no real v1.2 production signing key
/// has been through the key ceremony yet, so the production trust store is
/// empty and every candidate is untrusted. Populating this table is the
/// release-signing provisioning gate described in the module docs and in
/// `docs/release-signing.md`. Never add test keys here.
pub(crate) const PRODUCTION_KEYS: &[ProvisionedKey] = &[];

/// Build the production trust store from the provisioned table.
///
/// Every entry is validated here too, not only by the tests: a declared key
/// id that does not equal the frozen derivation over its own raw bytes, a
/// duplicate key id, or invalid key material is store corruption and fails
/// closed — the store is never built from an entry that contradicts its own
/// declaration.
pub(crate) fn store_from(keys: &[ProvisionedKey]) -> Result<TrustStore, TrustStoreError> {
    for key in keys {
        if derive_key_id(&key.raw) != key.declared_key_id {
            return Err(TrustStoreError::InvalidKeyMaterial {
                detail:
                    "provisioned entry's declared key id does not match the derived key id of its raw public key"
                        .to_string(),
            });
        }
    }
    let raws: Vec<[u8; 32]> = keys.iter().map(|key| key.raw).collect();
    TrustStore::from_raw_keys(&raws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trust::is_lowercase_hex_64;

    #[test]
    fn empty_table_provisions_a_fail_closed_store() {
        // The current release posture: no provisioned production key, so
        // every candidate is untrusted.
        assert!(PRODUCTION_KEYS.is_empty());
        assert!(store_from(PRODUCTION_KEYS).unwrap().is_empty());
    }

    #[test]
    fn provisioned_table_is_self_consistent() {
        for key in PRODUCTION_KEYS {
            // Declared id shape: exactly 64 lowercase hex characters.
            assert!(
                is_lowercase_hex_64(key.declared_key_id),
                "declared key id {:?} is not 64 lowercase hex characters",
                key.declared_key_id
            );
            // Declared id value: the frozen derivation over the raw bytes.
            assert_eq!(
                derive_key_id(&key.raw),
                key.declared_key_id,
                "declared key id does not match the derived SHA-256 of the raw public key"
            );
        }
        // Duplicate key ids are a SHA-256 collision and must fail closed.
        if let [first, rest @ ..] = PRODUCTION_KEYS {
            let mut all = vec![first.raw];
            all.extend(rest.iter().map(|key| key.raw));
            all.push(first.raw);
            assert!(
                TrustStore::from_raw_keys(&all).is_err(),
                "provisioned table contains a duplicate key id"
            );
        }
    }

    #[test]
    fn store_from_accepts_a_valid_key_and_fails_on_contradictions() {
        // Mechanism check with TEST-ONLY material (never a production
        // trust root): a valid entry builds a working store; a duplicated
        // entry and a mis-declared key id both fail closed.
        let signing = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let raw = signing.verifying_key().to_bytes();
        let derived = derive_key_id(&raw);
        let entry = ProvisionedKey {
            declared_key_id: derived.as_str(),
            raw,
        };
        let store = store_from(&[entry]).unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.lookup(&derived).is_some());

        let duplicate = [
            ProvisionedKey {
                declared_key_id: derived.as_str(),
                raw,
            },
            ProvisionedKey {
                declared_key_id: derived.as_str(),
                raw,
            },
        ];
        assert!(matches!(
            store_from(&duplicate),
            Err(TrustStoreError::DuplicateKeyId { .. })
        ));

        let wrong = "0".repeat(64);
        let mismatched = [ProvisionedKey {
            declared_key_id: wrong.as_str(),
            raw,
        }];
        assert!(matches!(
            store_from(&mismatched),
            Err(TrustStoreError::InvalidKeyMaterial { .. })
        ));
    }
}
