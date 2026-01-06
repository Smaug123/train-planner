//! Station code types.

use std::fmt;

/// Error returned when parsing an invalid CRS code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid CRS code: {reason}")]
pub struct InvalidCrs {
    reason: &'static str,
}

/// A valid 3-letter CRS (Computer Reservation System) station code.
///
/// CRS codes are always 3 uppercase ASCII letters. This type guarantees
/// that any `Crs` value is valid by construction.
///
/// # Examples
///
/// ```
/// use train_server::domain::Crs;
///
/// let kgx = Crs::parse("KGX").unwrap();
/// assert_eq!(kgx.as_str(), "KGX");
///
/// // Lowercase is rejected
/// assert!(Crs::parse("kgx").is_err());
///
/// // Wrong length is rejected
/// assert!(Crs::parse("KG").is_err());
/// assert!(Crs::parse("KGXX").is_err());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Crs([u8; 3]);

impl Crs {
    /// Parse a CRS code from a string.
    ///
    /// The input must be exactly 3 uppercase ASCII letters (A-Z).
    pub fn parse(s: &str) -> Result<Self, InvalidCrs> {
        let bytes = s.as_bytes();

        if bytes.len() != 3 {
            return Err(InvalidCrs {
                reason: "must be exactly 3 characters",
            });
        }

        for &b in bytes {
            if !b.is_ascii_uppercase() {
                return Err(InvalidCrs {
                    reason: "must be uppercase ASCII letters A-Z",
                });
            }
        }

        Ok(Crs([bytes[0], bytes[1], bytes[2]]))
    }

    /// Returns the CRS code as a string slice.
    pub fn as_str(&self) -> &str {
        // SAFETY: We only store valid ASCII uppercase letters
        std::str::from_utf8(&self.0).unwrap()
    }
}

impl fmt::Debug for Crs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Crs({})", self.as_str())
    }
}

impl fmt::Display for Crs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_crs() {
        assert!(Crs::parse("KGX").is_ok());
        assert!(Crs::parse("PAD").is_ok());
        assert!(Crs::parse("EUS").is_ok());
        assert!(Crs::parse("AAA").is_ok());
        assert!(Crs::parse("ZZZ").is_ok());
    }

    #[test]
    fn reject_lowercase() {
        assert!(Crs::parse("kgx").is_err());
        assert!(Crs::parse("Kgx").is_err());
        assert!(Crs::parse("KGx").is_err());
    }

    #[test]
    fn reject_wrong_length() {
        assert!(Crs::parse("").is_err());
        assert!(Crs::parse("K").is_err());
        assert!(Crs::parse("KG").is_err());
        assert!(Crs::parse("KGXX").is_err());
        assert!(Crs::parse("KINGS").is_err());
    }

    #[test]
    fn reject_non_ascii() {
        assert!(Crs::parse("K1X").is_err());
        assert!(Crs::parse("K-X").is_err());
        assert!(Crs::parse("K X").is_err());
        assert!(Crs::parse("KÖX").is_err());
    }

    #[test]
    fn as_str_roundtrip() {
        let crs = Crs::parse("KGX").unwrap();
        assert_eq!(crs.as_str(), "KGX");
    }

    #[test]
    fn display() {
        let crs = Crs::parse("PAD").unwrap();
        assert_eq!(format!("{}", crs), "PAD");
    }

    #[test]
    fn debug() {
        let crs = Crs::parse("EUS").unwrap();
        assert_eq!(format!("{:?}", crs), "Crs(EUS)");
    }

    #[test]
    fn equality() {
        let a = Crs::parse("KGX").unwrap();
        let b = Crs::parse("KGX").unwrap();
        let c = Crs::parse("PAD").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_consistent_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Crs::parse("KGX").unwrap());
        assert!(set.contains(&Crs::parse("KGX").unwrap()));
        assert!(!set.contains(&Crs::parse("PAD").unwrap()));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating valid CRS codes: 3 uppercase ASCII letters
    fn valid_crs_string() -> impl Strategy<Value = String> {
        proptest::string::string_regex("[A-Z]{3}")
            .unwrap()
            .prop_filter("must be 3 chars", |s| s.len() == 3)
    }

    proptest! {
        /// Roundtrip: parse then as_str returns the original
        #[test]
        fn roundtrip(s in valid_crs_string()) {
            let crs = Crs::parse(&s).unwrap();
            prop_assert_eq!(crs.as_str(), s.as_str());
        }

        /// Any valid CRS can be parsed
        #[test]
        fn valid_always_parses(s in valid_crs_string()) {
            prop_assert!(Crs::parse(&s).is_ok());
        }

        /// Lowercase letters are always rejected
        #[test]
        fn lowercase_rejected(s in "[a-z]{3}") {
            prop_assert!(Crs::parse(&s).is_err());
        }

        /// Wrong-length strings are always rejected
        #[test]
        fn wrong_length_rejected(s in "[A-Z]{0,2}|[A-Z]{4,10}") {
            prop_assert!(Crs::parse(&s).is_err());
        }

        /// Strings with digits are rejected
        #[test]
        fn digits_rejected(s in "[A-Z0-9]{3}".prop_filter("has digit", |s| s.chars().any(|c| c.is_ascii_digit()))) {
            prop_assert!(Crs::parse(&s).is_err());
        }
    }
}
