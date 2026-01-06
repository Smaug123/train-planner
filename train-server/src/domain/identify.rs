//! Train identification types.
//!
//! Types for identifying the user's current train based on observable
//! information like the next station and terminus.

use super::Crs;

/// User's criteria for identifying their current train.
///
/// The user provides information they can observe while on the train:
/// - The next station (from announcements or displays)
/// - The terminus/final destination (from displays)
///
/// We use this to query the next station's departure board and filter
/// to matching services.
#[derive(Debug, Clone)]
pub struct IdentifyTrainRequest {
    /// Next station the train will call at (required).
    ///
    /// This is where we query the departure board, since the train
    /// should appear as "departing soon" from this station.
    pub next_station: Crs,

    /// Final destination of the train (optional).
    ///
    /// If provided, we filter to services whose last calling point
    /// matches this station. Combined with next_station, this often
    /// uniquely identifies the train.
    pub terminus: Option<Crs>,
}

impl IdentifyTrainRequest {
    /// Create a new identification request.
    pub fn new(next_station: Crs, terminus: Option<Crs>) -> Self {
        Self {
            next_station,
            terminus,
        }
    }

    /// Create a request with just the next station.
    pub fn next_station_only(next_station: Crs) -> Self {
        Self {
            next_station,
            terminus: None,
        }
    }

    /// Create a request with both next station and terminus.
    pub fn with_terminus(next_station: Crs, terminus: Crs) -> Self {
        Self {
            next_station,
            terminus: Some(terminus),
        }
    }
}

/// How confidently we matched the train.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchConfidence {
    /// Both next_station and terminus matched.
    Exact,
    /// Only departing from next_station soon (no terminus filter applied).
    NextStationOnly,
}

impl MatchConfidence {
    /// Human-readable description of the confidence level.
    pub fn description(&self) -> &'static str {
        match self {
            MatchConfidence::Exact => "Matches next stop and terminus",
            MatchConfidence::NextStationOnly => "Matches next stop only",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    #[test]
    fn request_new() {
        let req = IdentifyTrainRequest::new(crs("WDB"), Some(crs("IPS")));
        assert_eq!(req.next_station, crs("WDB"));
        assert_eq!(req.terminus, Some(crs("IPS")));
    }

    #[test]
    fn request_next_station_only() {
        let req = IdentifyTrainRequest::next_station_only(crs("WDB"));
        assert_eq!(req.next_station, crs("WDB"));
        assert!(req.terminus.is_none());
    }

    #[test]
    fn request_with_terminus() {
        let req = IdentifyTrainRequest::with_terminus(crs("WDB"), crs("IPS"));
        assert_eq!(req.next_station, crs("WDB"));
        assert_eq!(req.terminus, Some(crs("IPS")));
    }

    #[test]
    fn confidence_ordering() {
        // Exact should be "better" (less than) NextStationOnly
        assert!(MatchConfidence::Exact < MatchConfidence::NextStationOnly);
    }

    #[test]
    fn confidence_description() {
        assert!(!MatchConfidence::Exact.description().is_empty());
        assert!(!MatchConfidence::NextStationOnly.description().is_empty());
    }
}
