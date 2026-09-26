//! The compiled, read-only `TrustedKeyStore` (protocol v1 freeze).
//!
//! Trust roots are compiled into the binaries. Runtime manifests, providers,
//! sessions, and the frontend can never add, replace, enable, or disable a
//! key; a manifest referencing an id absent from the store fails closed. Key
//! identity is the SHA-256 digest of the raw 32-byte Ed25519 public key,
//! lowercase hex — a derived, self-describing identifier. Lookup is by exact
//! key id only. Both the product binary and the maintenance helper embed the
//! same store, and each verifies with its own copy.

use crate::error::TrustStoreError;
use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};

#[cfg(test)]
use ed25519_dalek::SigningKey;

/// A compiled trust-root entry: the raw Ed25519 public key and its
/// self-describing key id.
#[derive(Debug, Clone)]
pub struct TrustedKey {
    raw: [u8; 32],
    verifying_key: VerifyingKey,
    key_id: String,
}

impl TrustedKey {
    /// The raw 32-byte Ed25519 public key.
    pub fn raw_public_key(&self) -> &[u8; 32] {
        &self.raw
    }

    /// The SHA-256 digest of the raw public key, lowercase hex.
    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

/// Derive the protocol key id: SHA-256 over the raw 32-byte public key,
/// lowercase hex, 64 characters.
pub fn derive_key_id(raw_public_key: &[u8; 32]) -> String {
    let digest = Sha256::digest(raw_public_key);
    hex_lower(&digest)
}

/// Build one trusted key from raw public-key bytes.
pub fn trusted_key_from_raw(raw: [u8; 32]) -> Result<TrustedKey, TrustStoreError> {
    let verifying_key =
        VerifyingKey::from_bytes(&raw).map_err(|e| TrustStoreError::InvalidKeyMaterial {
            detail: e.to_string(),
        })?;
    let key_id = derive_key_id(&raw);
    Ok(TrustedKey {
        raw,
        verifying_key,
        key_id,
    })
}

/// The compiled trust store. Read-only after construction; duplicate ids are
/// a SHA-256 collision and fail closed at construction, never at lookup.
#[derive(Debug, Clone)]
pub struct TrustStore {
    entries: Vec<TrustedKey>,
}

impl TrustStore {
    /// A store with no trust roots. Every candidate is untrusted; this is the
    /// correct fail-closed state before release-signing provisioning.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Compile a store from raw Ed25519 public keys. Duplicate key ids fail
    /// closed.
    pub fn from_raw_keys(keys: &[[u8; 32]]) -> Result<Self, TrustStoreError> {
        let mut store = Self::empty();
        for raw in keys {
            let key = trusted_key_from_raw(*raw)?;
            if store.lookup(key.key_id()).is_some() {
                return Err(TrustStoreError::DuplicateKeyId {
                    key_id: key.key_id().to_string(),
                });
            }
            store.entries.push(key);
        }
        Ok(store)
    }

    /// Exact key-id lookup. Any id not present is untrusted; there is no
    /// prefix, case-insensitive, or alias matching.
    pub fn lookup(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.entries
            .iter()
            .find(|entry| entry.key_id == key_id)
            .map(|entry| &entry.verifying_key)
    }

    /// Number of compiled trust roots.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate the compiled entries (diagnostics only — never a source of
    /// runtime key import).
    pub fn entries(&self) -> &[TrustedKey] {
        &self.entries
    }

    /// Verify an Ed25519 signature over exact message bytes with strict
    /// (malleability-rejecting) semantics.
    pub(crate) fn verify_strict(
        &self,
        verifying_key: &VerifyingKey,
        message: &[u8],
        signature: &ed25519_dalek::Signature,
    ) -> Result<(), ed25519_dalek::SignatureError> {
        verifying_key.verify_strict(message, signature)
    }
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from_digit((byte >> 4) as u32, 16).expect("hex digit"));
        text.push(char::from_digit((byte & 0xf) as u32, 16).expect("hex digit"));
    }
    text
}

/// True when `text` is exactly 64 lowercase hex characters.
pub(crate) fn is_lowercase_hex_64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_id_is_sha256_of_raw_public_key_lowercase_hex() {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let raw = signing.verifying_key().to_bytes();
        let key_id = derive_key_id(&raw);
        assert_eq!(key_id.len(), 64);
        assert!(is_lowercase_hex_64(&key_id));
        // Independent recomputation of the digest.
        let expected: String = Sha256::digest(raw)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(key_id, expected);
    }

    #[test]
    fn lookup_is_exact_and_unknown_ids_fail_closed() {
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let store = TrustStore::from_raw_keys(&[signing.verifying_key().to_bytes()]).unwrap();
        let key_id = derive_key_id(&signing.verifying_key().to_bytes());
        assert!(store.lookup(&key_id).is_some());
        assert!(store.lookup(&key_id.to_uppercase()).is_none());
        assert!(store.lookup(&"0".repeat(64)).is_none());
        assert!(store.lookup("").is_none());
    }

    #[test]
    fn duplicate_key_ids_fail_closed_at_construction() {
        let raw = SigningKey::from_bytes(&[3u8; 32])
            .verifying_key()
            .to_bytes();
        let err = TrustStore::from_raw_keys(&[raw, raw]).unwrap_err();
        assert_eq!(
            err,
            TrustStoreError::DuplicateKeyId {
                key_id: derive_key_id(&raw)
            }
        );
    }
}
