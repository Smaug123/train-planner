//! Domain error types.
//!
//! These errors represent validation failures and data inconsistencies
//! in the domain layer. They are distinct from API/IO errors.

use super::Crs;

/// Domain-level errors for validation and data consistency.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DomainError {
    /// Missing required time data for an operation
    #[error("missing required time data: {0}")]
    MissingTime(String),

    /// Call index is out of bounds for the service
    #[error("invalid call index: out of bounds")]
    InvalidCallIndex,

    /// Invalid leg construction (e.g., alight before board)
    #[error("invalid leg: {0}")]
    InvalidLeg(&'static str),

    /// Consecutive legs don't connect and aren't walkable
    #[error("stations {0} and {1} are not connected (no walk path)")]
    StationsNotConnected(Crs, Crs),

    /// Journey has no segments
    #[error("journey must have at least one segment")]
    EmptyJourney,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = DomainError::MissingTime("departure".into());
        assert_eq!(err.to_string(), "missing required time data: departure");

        let err = DomainError::InvalidCallIndex;
        assert_eq!(err.to_string(), "invalid call index: out of bounds");

        let err = DomainError::InvalidLeg("alight must be after board");
        assert_eq!(err.to_string(), "invalid leg: alight must be after board");

        let from = Crs::parse("PAD").unwrap();
        let to = Crs::parse("EUS").unwrap();
        let err = DomainError::StationsNotConnected(from, to);
        assert_eq!(
            err.to_string(),
            "stations PAD and EUS are not connected (no walk path)"
        );

        let err = DomainError::EmptyJourney;
        assert_eq!(err.to_string(), "journey must have at least one segment");
    }
}
