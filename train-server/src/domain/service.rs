//! Train service types.
//!
//! A `Service` represents a complete train journey with all its calling points.
//! `ServiceRef` provides an ephemeral reference to a service on Darwin,
//! and `ServiceCandidate` holds summary info from departure board searches.

use super::{AtocCode, Call, CallIndex, Crs, Headcode, RailTime};

/// Ephemeral Darwin service reference.
///
/// Only valid while the service appears on its board station's departure board.
/// NOT suitable for persistent storage or bookmarking - Darwin service IDs
/// have approximately 2 minute lifetime and are scoped to the board request.
///
/// # Important
///
/// This is fundamentally different from RTT's stable service UIDs. Darwin's
/// `serviceId` is ephemeral and cannot be used to construct stable URLs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceRef {
    /// The opaque Darwin service ID (from departure board)
    pub darwin_id: String,
    /// Station where we got this service from (for cache keying)
    pub board_crs: Crs,
}

impl ServiceRef {
    /// Creates a new service reference.
    pub fn new(darwin_id: String, board_crs: Crs) -> Self {
        Self {
            darwin_id,
            board_crs,
        }
    }
}

/// Candidate service from departure board search.
///
/// Contains summary information displayed on departure boards, before
/// we fetch full calling point details.
#[derive(Debug, Clone)]
pub struct ServiceCandidate {
    /// Reference to fetch full service details
    pub service_ref: ServiceRef,
    /// Train headcode (e.g., "1A23") if available
    pub headcode: Option<Headcode>,
    /// Scheduled departure time at the board station
    pub scheduled_departure: RailTime,
    /// Estimated/actual departure (if available)
    pub expected_departure: Option<RailTime>,
    /// Service destination(s) as display string
    pub destination: String,
    /// Primary destination CRS (if parseable)
    pub destination_crs: Option<Crs>,
    /// Operator name (e.g., "Great Western Railway")
    pub operator: String,
    /// ATOC operator code (e.g., "GW")
    pub operator_code: Option<AtocCode>,
    /// Platform number/letter (if known)
    pub platform: Option<String>,
    /// Whether this service is cancelled
    pub is_cancelled: bool,
}

impl ServiceCandidate {
    /// Returns the best available departure time (expected if available, else scheduled).
    pub fn departure_time(&self) -> RailTime {
        self.expected_departure.unwrap_or(self.scheduled_departure)
    }

    /// Returns true if the service is delayed (expected later than scheduled).
    pub fn is_delayed(&self) -> bool {
        self.expected_departure
            .is_some_and(|exp| exp > self.scheduled_departure)
    }

    /// Returns the delay duration if delayed.
    pub fn delay(&self) -> Option<chrono::Duration> {
        self.expected_departure.and_then(|exp| {
            if exp > self.scheduled_departure {
                Some(exp.signed_duration_since(self.scheduled_departure))
            } else {
                None
            }
        })
    }
}

/// A complete train service with full calling point data.
///
/// Contains merged previous and subsequent calling points in chronological
/// order. The `board_station_idx` indicates which station's board this
/// service was fetched from.
#[derive(Debug, Clone)]
pub struct Service {
    /// Reference for this service
    pub service_ref: ServiceRef,
    /// Train headcode (e.g., "1A23") if available
    pub headcode: Option<Headcode>,
    /// Operator name
    pub operator: String,
    /// ATOC operator code
    pub operator_code: Option<AtocCode>,
    /// All calling points (previous + current + subsequent, chronological)
    pub calls: Vec<Call>,
    /// Index of the board station in the calls list
    pub board_station_idx: CallIndex,
}

impl Service {
    /// Returns calls from the given index onwards (inclusive).
    ///
    /// Returns an empty slice if the index is out of bounds.
    pub fn calls_from_index(&self, idx: CallIndex) -> &[Call] {
        self.calls.get(idx.0..).unwrap_or(&[])
    }

    /// Returns calls up to and including the given index.
    ///
    /// Returns an empty slice if the index is out of bounds.
    pub fn calls_up_to_index(&self, idx: CallIndex) -> &[Call] {
        self.calls.get(..=idx.0).unwrap_or(&[])
    }

    /// Find the first call at a station at or after the given index.
    ///
    /// Returns both the index and the call, allowing unambiguous leg construction.
    /// This handles services that call at the same station multiple times.
    pub fn find_call(&self, station: &Crs, after: CallIndex) -> Option<(CallIndex, &Call)> {
        self.calls
            .iter()
            .enumerate()
            .skip(after.0)
            .find(|(_, call)| &call.station == station)
            .map(|(i, call)| (CallIndex(i), call))
    }

    /// Find all calls at a station.
    ///
    /// For services that call at the same station multiple times (loops,
    /// turnbacks), this returns all occurrences.
    pub fn all_calls_at(&self, station: &Crs) -> Vec<(CallIndex, &Call)> {
        self.calls
            .iter()
            .enumerate()
            .filter(|(_, call)| &call.station == station)
            .map(|(i, call)| (CallIndex(i), call))
            .collect()
    }

    /// Does this service call at the given station at or after the given index?
    pub fn calls_at(&self, station: &Crs, after: CallIndex) -> bool {
        self.find_call(station, after).is_some()
    }

    /// Returns the first calling point (origin).
    pub fn origin_call(&self) -> Option<(CallIndex, &Call)> {
        self.calls.first().map(|c| (CallIndex(0), c))
    }

    /// Returns the last calling point (destination).
    pub fn destination_call(&self) -> Option<(CallIndex, &Call)> {
        let len = self.calls.len();
        if len > 0 {
            self.calls.last().map(|c| (CallIndex(len - 1), c))
        } else {
            None
        }
    }

    /// Origin station name for display, or "Unknown" if empty.
    pub fn origin_name(&self) -> &str {
        self.origin_call()
            .map(|(_, c)| c.station_name.as_str())
            .unwrap_or("Unknown")
    }

    /// Destination station name for display, or "Unknown" if empty.
    pub fn destination_name(&self) -> &str {
        self.destination_call()
            .map(|(_, c)| c.station_name.as_str())
            .unwrap_or("Unknown")
    }

    /// Returns the board station call.
    pub fn board_station_call(&self) -> Option<&Call> {
        self.calls.get(self.board_station_idx.0)
    }

    /// Returns true if the entire service is cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.calls.iter().all(|c| c.is_cancelled)
    }

    /// Returns the number of calling points.
    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Returns true if there are no calling points.
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn time(s: &str) -> RailTime {
        RailTime::parse_hhmm(s, date()).unwrap()
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    fn make_call(station: &str, name: &str) -> Call {
        Call::new(crs(station), name.into())
    }

    fn make_service() -> Service {
        let mut calls = vec![
            make_call("PAD", "London Paddington"),
            make_call("RDG", "Reading"),
            make_call("SWI", "Swindon"),
            make_call("BRI", "Bristol Temple Meads"),
        ];

        // Add some times
        calls[0].booked_departure = Some(time("10:00"));
        calls[1].booked_arrival = Some(time("10:25"));
        calls[1].booked_departure = Some(time("10:27"));
        calls[2].booked_arrival = Some(time("10:52"));
        calls[2].booked_departure = Some(time("10:54"));
        calls[3].booked_arrival = Some(time("11:30"));

        Service {
            service_ref: ServiceRef::new("ABC123".into(), crs("PAD")),
            headcode: Headcode::parse("1A23"),
            operator: "Great Western Railway".into(),
            operator_code: AtocCode::parse("GW").ok(),
            calls,
            board_station_idx: CallIndex(0),
        }
    }

    // ServiceRef tests

    #[test]
    fn service_ref_new() {
        let sr = ServiceRef::new("ABC123".into(), crs("PAD"));
        assert_eq!(sr.darwin_id, "ABC123");
        assert_eq!(sr.board_crs, crs("PAD"));
    }

    #[test]
    fn service_ref_equality() {
        let sr1 = ServiceRef::new("ABC".into(), crs("PAD"));
        let sr2 = ServiceRef::new("ABC".into(), crs("PAD"));
        let sr3 = ServiceRef::new("DEF".into(), crs("PAD"));
        let sr4 = ServiceRef::new("ABC".into(), crs("RDG"));

        assert_eq!(sr1, sr2);
        assert_ne!(sr1, sr3);
        assert_ne!(sr1, sr4);
    }

    #[test]
    fn service_ref_hash() {
        use std::collections::HashSet;

        let mut set = HashSet::new();
        set.insert(ServiceRef::new("ABC".into(), crs("PAD")));

        assert!(set.contains(&ServiceRef::new("ABC".into(), crs("PAD"))));
        assert!(!set.contains(&ServiceRef::new("DEF".into(), crs("PAD"))));
    }

    // ServiceCandidate tests

    #[test]
    fn candidate_departure_time() {
        let candidate = ServiceCandidate {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            scheduled_departure: time("10:00"),
            expected_departure: None,
            destination: "Bristol".into(),
            destination_crs: Some(crs("BRI")),
            operator: "GWR".into(),
            operator_code: None,
            platform: Some("1".into()),
            is_cancelled: false,
        };

        // Without expected, returns scheduled
        assert_eq!(candidate.departure_time(), time("10:00"));
    }

    #[test]
    fn candidate_departure_time_with_expected() {
        let candidate = ServiceCandidate {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            scheduled_departure: time("10:00"),
            expected_departure: Some(time("10:05")),
            destination: "Bristol".into(),
            destination_crs: Some(crs("BRI")),
            operator: "GWR".into(),
            operator_code: None,
            platform: Some("1".into()),
            is_cancelled: false,
        };

        // With expected, returns expected
        assert_eq!(candidate.departure_time(), time("10:05"));
    }

    #[test]
    fn candidate_delay() {
        let mut candidate = ServiceCandidate {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            scheduled_departure: time("10:00"),
            expected_departure: None,
            destination: "Bristol".into(),
            destination_crs: None,
            operator: "GWR".into(),
            operator_code: None,
            platform: None,
            is_cancelled: false,
        };

        // No delay when no expected
        assert!(!candidate.is_delayed());
        assert!(candidate.delay().is_none());

        // No delay when on time
        candidate.expected_departure = Some(time("10:00"));
        assert!(!candidate.is_delayed());
        assert!(candidate.delay().is_none());

        // No delay when early
        candidate.expected_departure = Some(time("09:58"));
        assert!(!candidate.is_delayed());
        assert!(candidate.delay().is_none());

        // Delayed when late
        candidate.expected_departure = Some(time("10:10"));
        assert!(candidate.is_delayed());
        assert_eq!(candidate.delay(), Some(chrono::Duration::minutes(10)));
    }

    // Service tests

    #[test]
    fn service_calls_from_index() {
        let service = make_service();

        let from_start = service.calls_from_index(CallIndex(0));
        assert_eq!(from_start.len(), 4);

        let from_reading = service.calls_from_index(CallIndex(1));
        assert_eq!(from_reading.len(), 3);
        assert_eq!(from_reading[0].station, crs("RDG"));

        let from_end = service.calls_from_index(CallIndex(3));
        assert_eq!(from_end.len(), 1);
        assert_eq!(from_end[0].station, crs("BRI"));

        // Out of bounds returns empty
        let out_of_bounds = service.calls_from_index(CallIndex(10));
        assert!(out_of_bounds.is_empty());
    }

    #[test]
    fn service_calls_up_to_index() {
        let service = make_service();

        let up_to_reading = service.calls_up_to_index(CallIndex(1));
        assert_eq!(up_to_reading.len(), 2);
        assert_eq!(up_to_reading[0].station, crs("PAD"));
        assert_eq!(up_to_reading[1].station, crs("RDG"));

        // Out of bounds returns empty
        let out_of_bounds = service.calls_up_to_index(CallIndex(10));
        assert!(out_of_bounds.is_empty());
    }

    #[test]
    fn service_find_call() {
        let service = make_service();

        // Find from start
        let (idx, call) = service.find_call(&crs("RDG"), CallIndex(0)).unwrap();
        assert_eq!(idx, CallIndex(1));
        assert_eq!(call.station_name, "Reading");

        // Find from specific index
        let result = service.find_call(&crs("PAD"), CallIndex(1));
        assert!(result.is_none()); // PAD is before index 1

        // Station not in service
        let result = service.find_call(&crs("XXX"), CallIndex(0));
        assert!(result.is_none());
    }

    #[test]
    fn service_all_calls_at() {
        let service = make_service();

        let reading_calls = service.all_calls_at(&crs("RDG"));
        assert_eq!(reading_calls.len(), 1);
        assert_eq!(reading_calls[0].0, CallIndex(1));

        let unknown_calls = service.all_calls_at(&crs("XXX"));
        assert!(unknown_calls.is_empty());
    }

    #[test]
    fn service_calls_at() {
        let service = make_service();

        assert!(service.calls_at(&crs("RDG"), CallIndex(0)));
        assert!(service.calls_at(&crs("BRI"), CallIndex(0)));
        assert!(!service.calls_at(&crs("PAD"), CallIndex(1)));
        assert!(!service.calls_at(&crs("XXX"), CallIndex(0)));
    }

    #[test]
    fn service_origin_destination() {
        let service = make_service();

        let (origin_idx, origin) = service.origin_call().unwrap();
        assert_eq!(origin_idx, CallIndex(0));
        assert_eq!(origin.station, crs("PAD"));

        let (dest_idx, dest) = service.destination_call().unwrap();
        assert_eq!(dest_idx, CallIndex(3));
        assert_eq!(dest.station, crs("BRI"));

        assert_eq!(service.origin_name(), "London Paddington");
        assert_eq!(service.destination_name(), "Bristol Temple Meads");
    }

    #[test]
    fn service_empty() {
        let empty = Service {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls: vec![],
            board_station_idx: CallIndex(0),
        };

        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert!(empty.origin_call().is_none());
        assert!(empty.destination_call().is_none());
        assert_eq!(empty.origin_name(), "Unknown");
        assert_eq!(empty.destination_name(), "Unknown");
    }

    #[test]
    fn service_board_station_call() {
        let mut service = make_service();

        // Board at PAD (index 0)
        let call = service.board_station_call().unwrap();
        assert_eq!(call.station, crs("PAD"));

        // Change board to Reading (index 1)
        service.board_station_idx = CallIndex(1);
        let call = service.board_station_call().unwrap();
        assert_eq!(call.station, crs("RDG"));
    }

    #[test]
    fn service_len() {
        let service = make_service();
        assert_eq!(service.len(), 4);
        assert!(!service.is_empty());
    }

    #[test]
    fn service_is_cancelled() {
        let mut service = make_service();

        // Not cancelled initially
        assert!(!service.is_cancelled());

        // Partially cancelled is not fully cancelled
        service.calls[0].is_cancelled = true;
        assert!(!service.is_cancelled());

        // Fully cancelled
        for call in &mut service.calls {
            call.is_cancelled = true;
        }
        assert!(service.is_cancelled());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Generate a valid CRS code string from an index
    fn crs_from_index(i: usize) -> Crs {
        // Generate 3-letter codes: AAA, AAB, AAC, ..., AAZ, ABA, ...
        let c1 = b'A' + ((i / 676) % 26) as u8;
        let c2 = b'A' + ((i / 26) % 26) as u8;
        let c3 = b'A' + (i % 26) as u8;
        let s = format!("{}{}{}", c1 as char, c2 as char, c3 as char);
        Crs::parse(&s).unwrap()
    }

    proptest! {
        /// Finding a call that exists always succeeds
        #[test]
        fn find_call_existing(num_calls in 2usize..10, target_idx in 0usize..10) {
            if target_idx < num_calls {
                let calls: Vec<Call> = (0..num_calls)
                    .map(|i| {
                        Call::new(crs_from_index(i), format!("Station {}", i))
                    })
                    .collect();

                let service = Service {
                    service_ref: ServiceRef::new("TEST".into(), crs_from_index(0)),
                    headcode: None,
                    operator: "Test".into(),
                    operator_code: None,
                    calls,
                    board_station_idx: CallIndex(0),
                };

                let target_crs = crs_from_index(target_idx);

                // Should find from index 0
                let result = service.find_call(&target_crs, CallIndex(0));
                prop_assert!(result.is_some());

                let (found_idx, found_call) = result.unwrap();
                prop_assert_eq!(found_idx.0, target_idx);
                prop_assert_eq!(found_call.station, target_crs);
            }
        }

        /// calls_from_index returns correct length
        #[test]
        fn calls_from_index_length(num_calls in 1usize..20, start_idx in 0usize..20) {
            let calls: Vec<Call> = (0..num_calls)
                .map(|i| {
                    Call::new(crs_from_index(i), format!("Station {}", i))
                })
                .collect();

            let service = Service {
                service_ref: ServiceRef::new("TEST".into(), crs_from_index(0)),
                headcode: None,
                operator: "Test".into(),
                operator_code: None,
                calls,
                board_station_idx: CallIndex(0),
            };

            let result = service.calls_from_index(CallIndex(start_idx));

            if start_idx >= num_calls {
                prop_assert!(result.is_empty());
            } else {
                prop_assert_eq!(result.len(), num_calls - start_idx);
            }
        }
    }
}
