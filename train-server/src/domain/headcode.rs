//! Train headcode (train identity) type.

use std::fmt;

/// A validated train headcode (train identity).
///
/// Standard UK headcodes follow the format: digit, letter, two digits (e.g., "1A23").
/// The first digit indicates the train class, the letter indicates the route/destination area,
/// and the final two digits distinguish services.
///
/// Non-standard headcodes exist (charter trains, light engine movements, etc.) but are rare.
/// `Headcode::parse` returns `None` for these rather than an error, since they're not
/// invalid—just not in the standard format.
///
/// # Examples
///
/// ```
/// use train_server::domain::Headcode;
///
/// // Standard headcodes parse successfully
/// let hc = Headcode::parse("1A23").unwrap();
/// assert_eq!(hc.as_str(), "1A23");
///
/// // Non-standard formats return None
/// assert!(Headcode::parse("ABCD").is_none());
/// assert!(Headcode::parse("1234").is_none());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Headcode([u8; 4]);

impl Headcode {
    /// Parse a headcode from a string.
    ///
    /// Standard format: digit (0-9), uppercase letter (A-Z), two digits (00-99).
    /// Returns `None` for non-standard headcodes (which exist but are uncommon).
    pub fn parse(s: &str) -> Option<Self> {
        let bytes = s.as_bytes();

        if bytes.len() != 4 {
            return None;
        }

        // First character: digit
        if !bytes[0].is_ascii_digit() {
            return None;
        }

        // Second character: uppercase letter
        if !bytes[1].is_ascii_uppercase() {
            return None;
        }

        // Third and fourth characters: digits
        if !bytes[2].is_ascii_digit() || !bytes[3].is_ascii_digit() {
            return None;
        }

        Some(Headcode([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Returns the headcode as a string slice.
    pub fn as_str(&self) -> &str {
        // SAFETY: We only store valid ASCII characters
        std::str::from_utf8(&self.0).unwrap()
    }

    /// Returns the train class digit (first character).
    ///
    /// This indicates the type of service:
    /// - 0: Light locomotive
    /// - 1: Express passenger
    /// - 2: Ordinary passenger
    /// - 3: Parcels/mail
    /// - 4-6: Freight
    /// - 9: Eurostar (historically)
    pub fn class_digit(&self) -> char {
        self.0[0] as char
    }

    /// Returns the route letter (second character).
    pub fn route_letter(&self) -> char {
        self.0[1] as char
    }
}

impl fmt::Debug for Headcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Headcode({})", self.as_str())
    }
}

impl fmt::Display for Headcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_headcodes() {
        assert!(Headcode::parse("1A23").is_some());
        assert!(Headcode::parse("9Z99").is_some());
        assert!(Headcode::parse("0A00").is_some());
        assert!(Headcode::parse("2B45").is_some());
        assert!(Headcode::parse("5C67").is_some());
    }

    #[test]
    fn reject_all_letters() {
        assert!(Headcode::parse("ABCD").is_none());
        assert!(Headcode::parse("AAAA").is_none());
    }

    #[test]
    fn reject_all_digits() {
        assert!(Headcode::parse("1234").is_none());
        assert!(Headcode::parse("0000").is_none());
    }

    #[test]
    fn reject_wrong_length() {
        assert!(Headcode::parse("").is_none());
        assert!(Headcode::parse("1A2").is_none());
        assert!(Headcode::parse("1A234").is_none());
    }

    #[test]
    fn reject_wrong_positions() {
        // Letter in first position
        assert!(Headcode::parse("AA23").is_none());
        // Digit in second position
        assert!(Headcode::parse("1123").is_none());
        // Letter in third position
        assert!(Headcode::parse("1AA3").is_none());
        // Letter in fourth position
        assert!(Headcode::parse("1A2A").is_none());
    }

    #[test]
    fn reject_lowercase() {
        assert!(Headcode::parse("1a23").is_none());
    }

    #[test]
    fn as_str_roundtrip() {
        let hc = Headcode::parse("1A23").unwrap();
        assert_eq!(hc.as_str(), "1A23");
    }

    #[test]
    fn class_digit() {
        assert_eq!(Headcode::parse("1A23").unwrap().class_digit(), '1');
        assert_eq!(Headcode::parse("9Z99").unwrap().class_digit(), '9');
        assert_eq!(Headcode::parse("0A00").unwrap().class_digit(), '0');
    }

    #[test]
    fn route_letter() {
        assert_eq!(Headcode::parse("1A23").unwrap().route_letter(), 'A');
        assert_eq!(Headcode::parse("1Z23").unwrap().route_letter(), 'Z');
    }

    #[test]
    fn display() {
        let hc = Headcode::parse("2B45").unwrap();
        assert_eq!(format!("{}", hc), "2B45");
    }

    #[test]
    fn debug() {
        let hc = Headcode::parse("3C67").unwrap();
        assert_eq!(format!("{:?}", hc), "Headcode(3C67)");
    }

    #[test]
    fn equality() {
        let a = Headcode::parse("1A23").unwrap();
        let b = Headcode::parse("1A23").unwrap();
        let c = Headcode::parse("1A24").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating valid headcodes: digit, letter, two digits
    fn valid_headcode_string() -> impl Strategy<Value = String> {
        (
            proptest::char::range('0', '9'),
            proptest::char::range('A', 'Z'),
            proptest::char::range('0', '9'),
            proptest::char::range('0', '9'),
        )
            .prop_map(|(d1, l, d2, d3)| format!("{}{}{}{}", d1, l, d2, d3))
    }

    proptest! {
        /// Roundtrip: parse then as_str returns the original
        #[test]
        fn roundtrip(s in valid_headcode_string()) {
            let hc = Headcode::parse(&s).unwrap();
            prop_assert_eq!(hc.as_str(), s.as_str());
        }

        /// Any valid headcode can be parsed
        #[test]
        fn valid_always_parses(s in valid_headcode_string()) {
            prop_assert!(Headcode::parse(&s).is_some());
        }

        /// All-digit strings are rejected
        #[test]
        fn all_digits_rejected(s in "[0-9]{4}") {
            prop_assert!(Headcode::parse(&s).is_none());
        }

        /// All-letter strings are rejected
        #[test]
        fn all_letters_rejected(s in "[A-Z]{4}") {
            prop_assert!(Headcode::parse(&s).is_none());
        }

        /// Wrong-length strings are rejected
        #[test]
        fn wrong_length_rejected(s in "[0-9A-Z]{0,3}|[0-9A-Z]{5,10}") {
            prop_assert!(Headcode::parse(&s).is_none());
        }

        /// Lowercase in second position is rejected
        #[test]
        fn lowercase_letter_rejected(
            d1 in proptest::char::range('0', '9'),
            l in proptest::char::range('a', 'z'),
            d2 in proptest::char::range('0', '9'),
            d3 in proptest::char::range('0', '9'),
        ) {
            let s = format!("{}{}{}{}", d1, l, d2, d3);
            prop_assert!(Headcode::parse(&s).is_none());
        }
    }
}
