//! Train operator (ATOC) code type.

use std::fmt;

/// Error returned when parsing an invalid ATOC code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid ATOC code: {reason}")]
pub struct InvalidAtocCode {
    reason: &'static str,
}

/// A valid 2-letter ATOC (Association of Train Operating Companies) operator code.
///
/// ATOC codes identify train operating companies (e.g., "GW" for Great Western Railway,
/// "VT" for Avanti West Coast). They are always 2 uppercase ASCII letters.
///
/// # Examples
///
/// ```
/// use train_server::domain::AtocCode;
///
/// let gw = AtocCode::parse("GW").unwrap();
/// assert_eq!(gw.as_str(), "GW");
///
/// // Lowercase is rejected
/// assert!(AtocCode::parse("gw").is_err());
///
/// // Wrong length is rejected
/// assert!(AtocCode::parse("G").is_err());
/// assert!(AtocCode::parse("GWR").is_err());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AtocCode([u8; 2]);

impl AtocCode {
    /// Parse an ATOC code from a string.
    ///
    /// The input must be exactly 2 uppercase ASCII letters (A-Z).
    pub fn parse(s: &str) -> Result<Self, InvalidAtocCode> {
        let bytes = s.as_bytes();

        if bytes.len() != 2 {
            return Err(InvalidAtocCode {
                reason: "must be exactly 2 characters",
            });
        }

        for &b in bytes {
            if !b.is_ascii_uppercase() {
                return Err(InvalidAtocCode {
                    reason: "must be uppercase ASCII letters A-Z",
                });
            }
        }

        Ok(AtocCode([bytes[0], bytes[1]]))
    }

    /// Returns the ATOC code as a string slice.
    pub fn as_str(&self) -> &str {
        // SAFETY: We only store valid ASCII uppercase letters
        std::str::from_utf8(&self.0).unwrap()
    }
}

impl fmt::Debug for AtocCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AtocCode({})", self.as_str())
    }
}

impl fmt::Display for AtocCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_atoc_codes() {
        // Real UK operator codes
        assert!(AtocCode::parse("GW").is_ok()); // Great Western Railway
        assert!(AtocCode::parse("VT").is_ok()); // Avanti West Coast
        assert!(AtocCode::parse("SR").is_ok()); // ScotRail
        assert!(AtocCode::parse("SE").is_ok()); // Southeastern
        assert!(AtocCode::parse("XC").is_ok()); // CrossCountry
        assert!(AtocCode::parse("EM").is_ok()); // East Midlands Railway
        assert!(AtocCode::parse("GR").is_ok()); // LNER (Grand Central uses GR too)

        // Edge cases
        assert!(AtocCode::parse("AA").is_ok());
        assert!(AtocCode::parse("ZZ").is_ok());
    }

    #[test]
    fn reject_lowercase() {
        assert!(AtocCode::parse("gw").is_err());
        assert!(AtocCode::parse("Gw").is_err());
        assert!(AtocCode::parse("gW").is_err());
    }

    #[test]
    fn reject_wrong_length() {
        assert!(AtocCode::parse("").is_err());
        assert!(AtocCode::parse("G").is_err());
        assert!(AtocCode::parse("GWR").is_err());
        assert!(AtocCode::parse("GWRC").is_err());
    }

    #[test]
    fn reject_non_letters() {
        assert!(AtocCode::parse("G1").is_err());
        assert!(AtocCode::parse("1W").is_err());
        assert!(AtocCode::parse("12").is_err());
        assert!(AtocCode::parse("G ").is_err());
        assert!(AtocCode::parse("G-").is_err());
    }

    #[test]
    fn as_str_roundtrip() {
        let code = AtocCode::parse("GW").unwrap();
        assert_eq!(code.as_str(), "GW");
    }

    #[test]
    fn display() {
        let code = AtocCode::parse("VT").unwrap();
        assert_eq!(format!("{}", code), "VT");
    }

    #[test]
    fn debug() {
        let code = AtocCode::parse("SR").unwrap();
        assert_eq!(format!("{:?}", code), "AtocCode(SR)");
    }

    #[test]
    fn equality() {
        let a = AtocCode::parse("GW").unwrap();
        let b = AtocCode::parse("GW").unwrap();
        let c = AtocCode::parse("VT").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_consistent_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(AtocCode::parse("GW").unwrap());
        assert!(set.contains(&AtocCode::parse("GW").unwrap()));
        assert!(!set.contains(&AtocCode::parse("VT").unwrap()));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating valid ATOC codes: 2 uppercase ASCII letters
    fn valid_atoc_string() -> impl Strategy<Value = String> {
        proptest::string::string_regex("[A-Z]{2}")
            .unwrap()
            .prop_filter("must be 2 chars", |s| s.len() == 2)
    }

    proptest! {
        /// Roundtrip: parse then as_str returns the original
        #[test]
        fn roundtrip(s in valid_atoc_string()) {
            let code = AtocCode::parse(&s).unwrap();
            prop_assert_eq!(code.as_str(), s.as_str());
        }

        /// Any valid ATOC code can be parsed
        #[test]
        fn valid_always_parses(s in valid_atoc_string()) {
            prop_assert!(AtocCode::parse(&s).is_ok());
        }

        /// Lowercase letters are always rejected
        #[test]
        fn lowercase_rejected(s in "[a-z]{2}") {
            prop_assert!(AtocCode::parse(&s).is_err());
        }

        /// Wrong-length strings are always rejected
        #[test]
        fn wrong_length_rejected(s in "[A-Z]{0,1}|[A-Z]{3,10}") {
            prop_assert!(AtocCode::parse(&s).is_err());
        }

        /// Strings with digits are rejected
        #[test]
        fn digits_rejected(s in "[A-Z0-9]{2}".prop_filter("has digit", |s| s.chars().any(|c| c.is_ascii_digit()))) {
            prop_assert!(AtocCode::parse(&s).is_err());
        }
    }
}
