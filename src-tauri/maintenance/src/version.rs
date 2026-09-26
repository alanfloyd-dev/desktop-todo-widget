//! The one canonical semantic version policy for Maintenance Protocol 1.
//!
//! The implementation is the shared pure protocol core
//! (`desktop-todo-update-core/src/version.rs`), extracted in Phase 2B so the
//! signed-update core and this crate can never diverge into two slightly
//! different parsers (protocol v1: "Protocol 1 introduces no semver crate
//! and no second comparator"). Receipts, the manual bootstrap downgrade
//! check, and PE ProductVersion comparison all go through this module; the
//! tests below pin the re-exported surface.

pub use desktop_todo_update_core::version::{is_downgrade, Version};

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn stable_versions_parse_and_order_semantically() {
        assert_eq!(parse("1.1.0"), Version::new(1, 1, 0));
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
