//! Train leg type.
//!
//! A `Leg` represents a single train journey segment from boarding
//! to alighting. It uses `Arc<Service>` for cheap cloning in BFS search.

use std::sync::Arc;

use super::{Call, CallIndex, Crs, DomainError, RailTime, Service};

/// A leg of a journey (one train).
///
/// Uses `Arc<Service>` for cheap cloning in BFS search algorithms.
/// Times are validated at construction to guarantee `departure_time()`
/// and `arrival_time()` never fail.
///
/// # Invariants
///
/// - `alight_idx > board_idx` (must travel forward on the train)
/// - Both indices are valid for the service's calls
/// - Departure time exists at board index
/// - Arrival time exists at alight index
#[derive(Debug, Clone)]
pub struct Leg {
    service: Arc<Service>,
    board_idx: CallIndex,
    alight_idx: CallIndex,
    // Cached validated times (guaranteed present - validated at construction)
    departure: RailTime,
    arrival: RailTime,
}

impl Leg {
    /// Construct a leg, validating that required times exist and indices are valid.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - `alight_idx <= board_idx` (must travel forward)
    /// - Either index is out of bounds
    /// - Required departure/arrival times are missing
    ///
    /// # Examples
    ///
    /// ```
    /// use train_server::domain::{Leg, Service, ServiceRef, Call, CallIndex, Crs, RailTime};
    /// use std::sync::Arc;
    /// use chrono::NaiveDate;
    ///
    /// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
    /// let pad = Crs::parse("PAD").unwrap();
    /// let rdg = Crs::parse("RDG").unwrap();
    ///
    /// // Create a simple service with two stops
    /// let mut call1 = Call::new(pad, "London Paddington".into());
    /// call1.booked_departure = Some(RailTime::parse_hhmm("10:00", date).unwrap());
    ///
    /// let mut call2 = Call::new(rdg, "Reading".into());
    /// call2.booked_arrival = Some(RailTime::parse_hhmm("10:25", date).unwrap());
    ///
    /// let service = Arc::new(Service {
    ///     service_ref: ServiceRef::new("ABC".into(), pad),
    ///     headcode: None,
    ///     operator: "GWR".into(),
    ///     operator_code: None,
    ///     calls: vec![call1, call2],
    ///     board_station_idx: CallIndex(0),
    /// });
    ///
    /// let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();
    /// assert_eq!(leg.departure_time().to_string(), "10:00");
    /// assert_eq!(leg.arrival_time().to_string(), "10:25");
    /// ```
    pub fn new(
        service: Arc<Service>,
        board_idx: CallIndex,
        alight_idx: CallIndex,
    ) -> Result<Self, DomainError> {
        if alight_idx.0 <= board_idx.0 {
            return Err(DomainError::InvalidLeg(
                "alight index must be after board index",
            ));
        }

        let board_call = service
            .calls
            .get(board_idx.0)
            .ok_or(DomainError::InvalidCallIndex)?;
        let alight_call = service
            .calls
            .get(alight_idx.0)
            .ok_or(DomainError::InvalidCallIndex)?;

        let departure = board_call
            .expected_departure()
            .ok_or_else(|| DomainError::MissingTime("boarding departure".into()))?;
        let arrival = alight_call
            .expected_arrival()
            .ok_or_else(|| DomainError::MissingTime("alighting arrival".into()))?;

        Ok(Leg {
            service,
            board_idx,
            alight_idx,
            departure,
            arrival,
        })
    }

    /// Returns the service this leg is on.
    pub fn service(&self) -> &Arc<Service> {
        &self.service
    }

    /// Returns the boarding call index.
    pub fn board_idx(&self) -> CallIndex {
        self.board_idx
    }

    /// Returns the alighting call index.
    pub fn alight_idx(&self) -> CallIndex {
        self.alight_idx
    }

    /// Returns the boarding call.
    pub fn board_call(&self) -> &Call {
        // Safe: validated at construction
        &self.service.calls[self.board_idx.0]
    }

    /// Returns the alighting call.
    pub fn alight_call(&self) -> &Call {
        // Safe: validated at construction
        &self.service.calls[self.alight_idx.0]
    }

    /// Returns the departure time (guaranteed present).
    pub fn departure_time(&self) -> RailTime {
        self.departure
    }

    /// Returns the arrival time (guaranteed present).
    pub fn arrival_time(&self) -> RailTime {
        self.arrival
    }

    /// Returns the boarding platform, if known.
    pub fn board_platform(&self) -> Option<&str> {
        self.board_call().platform.as_deref()
    }

    /// Returns the alighting platform, if known.
    pub fn alight_platform(&self) -> Option<&str> {
        self.alight_call().platform.as_deref()
    }

    /// Returns the boarding station CRS.
    pub fn board_station(&self) -> &Crs {
        &self.board_call().station
    }

    /// Returns the alighting station CRS.
    pub fn alight_station(&self) -> &Crs {
        &self.alight_call().station
    }

    /// Returns the boarding station name.
    pub fn board_station_name(&self) -> &str {
        &self.board_call().station_name
    }

    /// Returns the alighting station name.
    pub fn alight_station_name(&self) -> &str {
        &self.alight_call().station_name
    }

    /// Returns the journey duration.
    pub fn duration(&self) -> chrono::Duration {
        self.arrival.signed_duration_since(self.departure)
    }

    /// Returns the number of intermediate stops (excluding board and alight).
    pub fn intermediate_stop_count(&self) -> usize {
        self.alight_idx.0 - self.board_idx.0 - 1
    }

    /// Returns all calls for this leg (from board to alight, inclusive).
    pub fn calls(&self) -> &[Call] {
        &self.service.calls[self.board_idx.0..=self.alight_idx.0]
    }

    /// Returns true if this leg has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.board_call().is_cancelled || self.alight_call().is_cancelled
    }
}

impl PartialEq for Leg {
    fn eq(&self, other: &Self) -> bool {
        // Two legs are equal if they're on the same service (by reference)
        // and have the same board/alight indices
        Arc::ptr_eq(&self.service, &other.service)
            && self.board_idx == other.board_idx
            && self.alight_idx == other.alight_idx
    }
}

impl Eq for Leg {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ServiceRef;
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

    fn make_service() -> Arc<Service> {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
            Call::new(crs("SWI"), "Swindon".into()),
            Call::new(crs("BRI"), "Bristol Temple Meads".into()),
        ];

        // Add times
        calls[0].booked_departure = Some(time("10:00"));
        calls[0].platform = Some("1".into());
        calls[1].booked_arrival = Some(time("10:25"));
        calls[1].booked_departure = Some(time("10:27"));
        calls[2].booked_arrival = Some(time("10:52"));
        calls[2].booked_departure = Some(time("10:54"));
        calls[3].booked_arrival = Some(time("11:30"));
        calls[3].platform = Some("3".into());

        Arc::new(Service {
            service_ref: ServiceRef::new("ABC123".into(), crs("PAD")),
            headcode: None,
            operator: "Great Western Railway".into(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        })
    }

    #[test]
    fn leg_construction_valid() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();

        assert_eq!(leg.departure_time(), time("10:00"));
        assert_eq!(leg.arrival_time(), time("11:30"));
    }

    #[test]
    fn leg_board_alight_indices() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(1), CallIndex(3)).unwrap();

        assert_eq!(leg.board_idx(), CallIndex(1));
        assert_eq!(leg.alight_idx(), CallIndex(3));
    }

    #[test]
    fn leg_stations() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();

        assert_eq!(leg.board_station(), &crs("PAD"));
        assert_eq!(leg.alight_station(), &crs("BRI"));
        assert_eq!(leg.board_station_name(), "London Paddington");
        assert_eq!(leg.alight_station_name(), "Bristol Temple Meads");
    }

    #[test]
    fn leg_platforms() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();

        assert_eq!(leg.board_platform(), Some("1"));
        assert_eq!(leg.alight_platform(), Some("3"));
    }

    #[test]
    fn leg_duration() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();

        // 10:00 to 11:30 = 90 minutes
        assert_eq!(leg.duration(), chrono::Duration::minutes(90));
    }

    #[test]
    fn leg_intermediate_stops() {
        let service = make_service();

        // PAD to BRI: RDG and SWI are intermediate
        let leg = Leg::new(service.clone(), CallIndex(0), CallIndex(3)).unwrap();
        assert_eq!(leg.intermediate_stop_count(), 2);

        // PAD to RDG: no intermediate stops
        let leg = Leg::new(service.clone(), CallIndex(0), CallIndex(1)).unwrap();
        assert_eq!(leg.intermediate_stop_count(), 0);

        // RDG to BRI: SWI is intermediate
        let leg = Leg::new(service, CallIndex(1), CallIndex(3)).unwrap();
        assert_eq!(leg.intermediate_stop_count(), 1);
    }

    #[test]
    fn leg_calls() {
        let service = make_service();
        let leg = Leg::new(service, CallIndex(1), CallIndex(3)).unwrap();

        let calls = leg.calls();
        assert_eq!(calls.len(), 3); // RDG, SWI, BRI
        assert_eq!(calls[0].station, crs("RDG"));
        assert_eq!(calls[1].station, crs("SWI"));
        assert_eq!(calls[2].station, crs("BRI"));
    }

    #[test]
    fn leg_invalid_alight_before_board() {
        let service = make_service();
        let result = Leg::new(service, CallIndex(2), CallIndex(1));

        assert!(matches!(result, Err(DomainError::InvalidLeg(_))));
    }

    #[test]
    fn leg_invalid_same_index() {
        let service = make_service();
        let result = Leg::new(service, CallIndex(1), CallIndex(1));

        assert!(matches!(result, Err(DomainError::InvalidLeg(_))));
    }

    #[test]
    fn leg_invalid_board_out_of_bounds() {
        let service = make_service();
        let result = Leg::new(service, CallIndex(10), CallIndex(11));

        assert!(matches!(result, Err(DomainError::InvalidCallIndex)));
    }

    #[test]
    fn leg_invalid_alight_out_of_bounds() {
        let service = make_service();
        let result = Leg::new(service, CallIndex(0), CallIndex(10));

        assert!(matches!(result, Err(DomainError::InvalidCallIndex)));
    }

    #[test]
    fn leg_missing_departure_time() {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
        ];
        // No departure time at PAD
        calls[1].booked_arrival = Some(time("10:25"));

        let service = Arc::new(Service {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        });

        let result = Leg::new(service, CallIndex(0), CallIndex(1));
        assert!(matches!(result, Err(DomainError::MissingTime(_))));
    }

    #[test]
    fn leg_missing_arrival_time() {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
        ];
        calls[0].booked_departure = Some(time("10:00"));
        // No arrival time at RDG

        let service = Arc::new(Service {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        });

        let result = Leg::new(service, CallIndex(0), CallIndex(1));
        assert!(matches!(result, Err(DomainError::MissingTime(_))));
    }

    #[test]
    fn leg_equality() {
        let service = make_service();
        let leg1 = Leg::new(service.clone(), CallIndex(0), CallIndex(2)).unwrap();
        let leg2 = Leg::new(service.clone(), CallIndex(0), CallIndex(2)).unwrap();
        let leg3 = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();

        assert_eq!(leg1, leg2);
        assert_ne!(leg1, leg3);
    }

    #[test]
    fn leg_is_cancelled() {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
        ];
        calls[0].booked_departure = Some(time("10:00"));
        calls[1].booked_arrival = Some(time("10:25"));

        let service = Arc::new(Service {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        });

        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();
        assert!(!leg.is_cancelled());
    }

    #[test]
    fn leg_with_realtime_times() {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
        ];
        calls[0].booked_departure = Some(time("10:00"));
        calls[0].realtime_departure = Some(time("10:05")); // Delayed
        calls[1].booked_arrival = Some(time("10:25"));
        calls[1].realtime_arrival = Some(time("10:30")); // Delayed

        let service = Arc::new(Service {
            service_ref: ServiceRef::new("ABC".into(), crs("PAD")),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        });

        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();

        // Should use realtime times
        assert_eq!(leg.departure_time(), time("10:05"));
        assert_eq!(leg.arrival_time(), time("10:30"));
    }
}
