//! Strict RFC 3339 UTC timestamp validation for `publishedAt`.
//!
//! Protocol documents carry RFC 3339 UTC timestamps. This is a deliberately
//! small validator, not a date library: it accepts the canonical UTC forms
//! (`Z`, `z`, or the explicit `+00:00` offset) with optional fractional
//! seconds, validates calendar plausibility including leap years, and allows
//! the RFC 3339 leap second (`:60`). Any other offset means the instant is
//! not expressed in UTC and is rejected.

/// True when `text` is a well-formed RFC 3339 UTC timestamp.
pub fn is_rfc3339_utc(text: &str) -> bool {
    let b = text.as_bytes();
    // Minimum: YYYY-MM-DDTHH:MM:SSZ
    if b.len() < 20 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let slice = &b[range];
        if slice.iter().all(|c| c.is_ascii_digit()) {
            std::str::from_utf8(slice).ok()?.parse().ok()
        } else {
            None
        }
    };

    // Date: YYYY-MM-DD
    if b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    if !(1..=12).contains(&month) {
        return false;
    }
    if day == 0 || day > days_in_month(year, month) {
        return false;
    }

    // Separator: RFC 3339 allows the uppercase T (and lowercase t).
    if b[10] != b'T' && b[10] != b't' {
        return false;
    }

    // Time: HH:MM:SS
    if b[13] != b':' || b[16] != b':' {
        return false;
    }
    let (Some(hour), Some(minute), Some(second)) = (digits(11..13), digits(14..16), digits(17..19))
    else {
        return false;
    };
    if hour > 23 || minute > 59 || second > 60 {
        return false;
    }

    // Optional fractional seconds, then the UTC designator.
    let mut rest = &b[19..];
    if rest.first() == Some(&b'.') {
        let frac = &rest[1..];
        let n = frac
            .iter()
            .position(|c| !c.is_ascii_digit())
            .unwrap_or(frac.len());
        if n == 0 {
            return false;
        }
        rest = &frac[n..];
    }
    matches!(rest, [b'Z'] | [b'z'] | [b'+', b'0', b'0', b':', b'0', b'0'])
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_utc_forms_are_accepted() {
        assert!(is_rfc3339_utc("2026-10-01T00:00:00Z"));
        assert!(is_rfc3339_utc("2026-10-01T23:59:60Z")); // leap second
        assert!(is_rfc3339_utc("2024-02-29T12:00:00Z")); // leap day
        assert!(is_rfc3339_utc("2026-10-01T00:00:00.123456Z"));
        assert!(is_rfc3339_utc("2026-10-01T00:00:00+00:00"));
    }

    #[test]
    fn non_utc_or_malformed_timestamps_are_rejected() {
        for text in [
            "2026-10-01T00:00:00+01:00", // not UTC
            "2026-10-01T00:00:00-00:00", // unknown local offset
            "2026-10-01T00:00:00",       // missing designator
            "2026-10-01 00:00:00Z",      // space separator
            "2026-13-01T00:00:00Z",      // month out of range
            "2026-02-30T00:00:00Z",      // impossible day
            "2023-02-29T00:00:00Z",      // not a leap year
            "2026-10-01T24:00:00Z",      // hour out of range
            "2026-10-01T00:60:00Z",      // minute out of range
            "2026-10-01T00:00:61Z",      // beyond the leap second
            "2026-10-01T00:00:00.Z",     // empty fraction
            "2026-10-01T00:00:00Z ",     // trailing space
            "",                          // empty
            "2026-10-01T00:00:00.123Zextra",
        ] {
            assert!(!is_rfc3339_utc(text), "{text:?} must be rejected");
        }
    }
}
