//! The one canonical semantic version policy for Maintenance Protocol 1.
//!
//! Receipts, the manual bootstrap downgrade check, and PE ProductVersion
//! comparison all go through this module, so they can never diverge on what a
//! valid version is or how two versions order. Protocol 1 delivers stable
//! numeric dotted versions only; future manifest versions reuse this type.

use crate::{Error, ErrorKind, Result};

/// A stable Protocol 1 version: exactly three numeric components, each a
/// 16-bit decimal without leading zeros (`0` is allowed, `01` is rejected).
/// The 16-bit bound matches the PE `VS_FIXEDFILEINFO` fields, so every
/// version comparable here is representable both as text and as PE metadata.
/// Prerelease/build suffixes are not accepted: protocol 1 delivers stable
/// versions only, and any future channel extension is a protocol change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u16,
    minor: u16,
    patch: u16,
}

impl Version {
    /// Strict parse. Anything that is not exactly `M.m.p` with digit-only,
    /// leading-zero-free, u16-range components fails closed — including
    /// prerelease suffixes, whitespace, signs, and overflow.
    pub fn parse(text: &str) -> Result<Self> {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.len() != 3 {
            return Err(Error::new(
                ErrorKind::VersionMalformed,
                format!("Version must be three numeric components: {text:?}"),
            ));
        }
        let mut fields = [0u16; 3];
        for (field, part) in fields.iter_mut().zip(parts) {
            let well_formed = !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'));
            let value = if well_formed {
                part.parse::<u16>().ok()
            } else {
                None
            };
            *field = value.ok_or_else(|| {
                Error::new(
                    ErrorKind::VersionMalformed,
                    format!("Invalid version component {part:?}"),
                )
            })?;
        }
        Ok(Self {
            major: fields[0],
            minor: fields[1],
            patch: fields[2],
        })
    }
}

/// True when `installed` is strictly newer than `candidate`, i.e. installing
/// or updating to `candidate` would be a downgrade. Unparseable input fails
/// closed: the caller refuses the operation, never silently allows it.
pub fn is_downgrade(installed: &str, candidate: &str) -> bool {
    match (Version::parse(installed), Version::parse(candidate)) {
        (Ok(a), Ok(b)) => a > b,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn stable_versions_parse_and_order_semantically() {
        assert_eq!(parse("1.1.0"), Version { major: 1, minor: 1, patch: 0 });
        assert!(parse("1.10.0") > parse("1.9.0"));
        assert!(parse("1.10.0") > parse("1.2.0"));
        assert!(parse("2.0.0") > parse("1.99.99"));
        assert!(parse("0.0.0") < parse("0.0.1"));
        assert_eq!(parse("1.2.0"), parse("1.2.0"));
    }

    #[test]
    fn malformed_prerelease_leading_zero_overflow_and_short_fail_closed() {
        for text in [
            "1.2",          // too few components
            "1.2.0.0",      // too many components
            "1.2.0-beta",   // prerelease: protocol 1 delivers stable only
            "1.2.0+build",  // build metadata: same
            "01.2.0",       // leading zero
            "1.02.0",       // leading zero
            "65536.0.0",    // exceeds the PE 16-bit component bound
            "banana",       // not numeric
            "1.x.0",        // partially numeric
            "",             // empty
            "1..0",         // empty component
            "1.2.0\n",      // trailing whitespace is not a digit
            "+1.2.0",       // sign prefix is not a digit form
            " 1.2.0",       // leading whitespace
        ] {
            assert!(
                Version::parse(text).is_err(),
                "{text:?} must be rejected"
            );
        }
    }

    #[test]
    fn downgrade_detection_fails_closed() {
        assert!(is_downgrade("1.2.0", "1.1.0"));
        assert!(!is_downgrade("1.1.0", "1.2.0"));
        assert!(!is_downgrade("1.1.0", "1.1.0")); // equal is reinstall, not downgrade
        assert!(is_downgrade("1.10.0", "1.9.0"));
        // Unparseable input refuses the operation (reports as downgrade) —
        // never a silent allow.
        assert!(is_downgrade("1.2", "1.1.0"));
        assert!(is_downgrade("banana", "1.1.0"));
        assert!(is_downgrade("1.1.0", "banana"));
    }
}
