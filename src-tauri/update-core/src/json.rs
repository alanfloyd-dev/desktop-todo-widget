//! Strict JSON parsing helpers.
//!
//! Protocol documents parse into closed structures that reject duplicate
//! object keys, missing required fields, unknown fields, and trailing input
//! (protocol v1, "Encoding and validation"). `serde_json` alone covers
//! missing/unknown fields (`deny_unknown_fields`) and trailing input, but it
//! silently lets a later duplicate object key win, so a structural pre-pass
//! rejects duplicates at every nesting level before the typed parse runs.

use serde::de::{Deserialize, Deserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

/// Parse a complete protocol document: duplicate keys rejected at every
/// object level, unknown fields rejected by the target type, trailing input
/// rejected.
pub(crate) fn strict_parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, serde_json::Error> {
    reject_duplicate_keys(bytes)?;
    serde_json::from_slice(bytes)
}

pub(crate) use serde::de::DeserializeOwned;

/// Walk the raw JSON once, rejecting any object that repeats a key.
pub(crate) fn reject_duplicate_keys(bytes: &[u8]) -> Result<(), serde_json::Error> {
    let mut de = serde_json::Deserializer::from_slice(bytes);
    NoDup::deserialize(&mut de)?;
    Ok(())
}

/// Marker type whose deserialization walks a value and fails on any
/// duplicate object key, recursing through objects, arrays, and scalars.
struct NoDup;

impl<'de> Deserialize<'de> for NoDup {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NoDupVisitor)?;
        Ok(NoDup)
    }
}

struct NoDupVisitor;

impl<'de> Visitor<'de> for NoDupVisitor {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<(), A::Error> {
        let mut seen: HashSet<Cow<'de, str>> = HashSet::new();
        while let Some(key) = access.next_key::<Cow<'de, str>>()? {
            if !seen.insert(key) {
                return Err(A::Error::custom("duplicate object key"));
            }
            access.next_value::<NoDup>()?;
        }
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut access: A) -> Result<(), A::Error> {
        while access.next_element::<NoDup>()?.is_some() {}
        Ok(())
    }

    // Scalars: serde_json's `deserialize_any` only produces the visits below
    // for numbers and simple values; they carry nothing to recurse into.
    fn visit_bool<E: DeError>(self, _v: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: DeError>(self, _v: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: DeError>(self, _v: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: DeError>(self, _v: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: DeError>(self, _v: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: DeError>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E: DeError>(self) -> Result<(), E> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        a: u32,
    }

    #[test]
    fn duplicate_keys_are_rejected_at_every_object_level() {
        for text in [
            r#"{"a":1,"a":2}"#,
            r#"{"a":1,"b":{"c":1,"c":2}}"#,
            r#"{"a":1,"b":[{"c":1},{"c":2,"c":3}]}"#,
        ] {
            assert!(reject_duplicate_keys(text.as_bytes()).is_err(), "{text}");
        }
        assert!(reject_duplicate_keys(br#"{"a":1,"b":{"c":2}}"#).is_ok());
    }

    #[test]
    fn strict_parse_rejects_duplicates_unknown_fields_and_trailing_input() {
        assert_eq!(
            strict_parse::<Probe>(br#"{"a":1}"#).unwrap(),
            Probe { a: 1 }
        );
        assert!(strict_parse::<Probe>(br#"{"a":1,"a":2}"#).is_err());
        assert!(strict_parse::<Probe>(br#"{"a":1,"x":2}"#).is_err());
        assert!(strict_parse::<Probe>(br#"{"a":1} trailing"#).is_err());
        // Trailing whitespace after the document is not trailing input.
        assert_eq!(
            strict_parse::<Probe>(br#"{"a":1}   "#).unwrap(),
            Probe { a: 1 }
        );
    }
}
