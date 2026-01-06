//! RTT service UID type.

use std::fmt;

/// Error returned when parsing an invalid service UID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid service UID: {reason}")]
pub struct InvalidServiceUid {
    reason: &'static str,
}

/// An RTT (Realtime Trains) service unique identifier.
///
/// Service UIDs are opaque identifiers assigned by RTT to uniquely identify
/// a train service. The only validation is that they must be non-empty.
///
/// # Examples
///
/// ```
/// use train_server::domain::ServiceUid;
///
/// let uid = ServiceUid::new("P12345".to_string()).unwrap();
/// assert_eq!(uid.as_str(), "P12345");
///
/// // Empty strings are rejected
/// assert!(ServiceUid::new("".to_string()).is_err());
/// ```
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ServiceUid(String);

impl ServiceUid {
    /// Create a new service UID from a string.
    ///
    /// Returns an error if the string is empty.
    pub fn new(s: String) -> Result<Self, InvalidServiceUid> {
        if s.is_empty() {
            return Err(InvalidServiceUid {
                reason: "service UID cannot be empty",
            });
        }
        Ok(ServiceUid(s))
    }

    /// Returns the service UID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the ServiceUid and returns the inner String.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for ServiceUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServiceUid({})", self.0)
    }
}

impl fmt::Display for ServiceUid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_valid_uid() {
        assert!(ServiceUid::new("P12345".to_string()).is_ok());
        assert!(ServiceUid::new("Q67890".to_string()).is_ok());
        assert!(ServiceUid::new("a".to_string()).is_ok());
        assert!(ServiceUid::new("X".to_string()).is_ok());
        // RTT UIDs can contain various characters
        assert!(ServiceUid::new("P12345-A".to_string()).is_ok());
        assert!(ServiceUid::new("123".to_string()).is_ok());
    }

    #[test]
    fn reject_empty() {
        assert!(ServiceUid::new("".to_string()).is_err());
    }

    #[test]
    fn as_str_roundtrip() {
        let uid = ServiceUid::new("P12345".to_string()).unwrap();
        assert_eq!(uid.as_str(), "P12345");
    }

    #[test]
    fn into_inner() {
        let uid = ServiceUid::new("P12345".to_string()).unwrap();
        assert_eq!(uid.into_inner(), "P12345".to_string());
    }

    #[test]
    fn display() {
        let uid = ServiceUid::new("Q67890".to_string()).unwrap();
        assert_eq!(format!("{}", uid), "Q67890");
    }

    #[test]
    fn debug() {
        let uid = ServiceUid::new("R11111".to_string()).unwrap();
        assert_eq!(format!("{:?}", uid), "ServiceUid(R11111)");
    }

    #[test]
    fn equality() {
        let a = ServiceUid::new("P12345".to_string()).unwrap();
        let b = ServiceUid::new("P12345".to_string()).unwrap();
        let c = ServiceUid::new("Q67890".to_string()).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_consistent_with_eq() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ServiceUid::new("P12345".to_string()).unwrap());
        assert!(set.contains(&ServiceUid::new("P12345".to_string()).unwrap()));
        assert!(!set.contains(&ServiceUid::new("Q67890".to_string()).unwrap()));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any non-empty string can be used as a service UID
        #[test]
        fn nonempty_always_valid(s in ".+") {
            prop_assert!(ServiceUid::new(s).is_ok());
        }

        /// Roundtrip: new then as_str returns the original
        #[test]
        fn roundtrip(s in ".+") {
            let uid = ServiceUid::new(s.clone()).unwrap();
            prop_assert_eq!(uid.as_str(), s.as_str());
        }

        /// Empty string is always rejected
        #[test]
        fn empty_rejected(_seed in 0u32..100u32) {
            prop_assert!(ServiceUid::new("".to_string()).is_err());
        }
    }
}
