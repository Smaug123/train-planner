//! Journey types.
//!
//! A `Journey` represents a complete trip from origin to destination,
//! potentially including multiple train legs and walks between stations.

use chrono::Duration;

use super::{Crs, DomainError, Leg, RailTime};

/// A walk between nearby stations.
///
/// Represents an interchange walk (e.g., King's Cross to St Pancras).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Walk {
    /// Origin station
    pub from: Crs,
    /// Destination station
    pub to: Crs,
    /// Walking duration
    pub duration: Duration,
}

impl Walk {
    /// Creates a new walk between stations.
    pub fn new(from: Crs, to: Crs, duration: Duration) -> Self {
        Self { from, to, duration }
    }

    /// Returns the origin station name for display.
    pub fn from_name(&self) -> &str {
        self.from.as_str()
    }

    /// Returns the destination station name for display.
    pub fn to_name(&self) -> &str {
        self.to.as_str()
    }
}

/// A segment of a journey: either a train leg or a walk.
#[derive(Debug, Clone)]
pub enum Segment {
    /// A train journey segment
    Train(Leg),
    /// A walk between stations
    Walk(Walk),
}

impl Segment {
    /// Returns the origin station of this segment.
    pub fn origin(&self) -> &Crs {
        match self {
            Segment::Train(leg) => leg.board_station(),
            Segment::Walk(walk) => &walk.from,
        }
    }

    /// Returns the destination station of this segment.
    pub fn destination(&self) -> &Crs {
        match self {
            Segment::Train(leg) => leg.alight_station(),
            Segment::Walk(walk) => &walk.to,
        }
    }

    /// Returns the duration of this segment.
    pub fn duration(&self) -> Duration {
        match self {
            Segment::Train(leg) => leg.duration(),
            Segment::Walk(walk) => walk.duration,
        }
    }

    /// Returns true if this is a train segment.
    pub fn is_train(&self) -> bool {
        matches!(self, Segment::Train(_))
    }

    /// Returns true if this is a walk segment.
    pub fn is_walk(&self) -> bool {
        matches!(self, Segment::Walk(_))
    }

    /// Returns the leg if this is a train segment.
    pub fn as_leg(&self) -> Option<&Leg> {
        match self {
            Segment::Train(leg) => Some(leg),
            Segment::Walk(_) => None,
        }
    }

    /// Returns the walk if this is a walk segment.
    pub fn as_walk(&self) -> Option<&Walk> {
        match self {
            Segment::Train(_) => None,
            Segment::Walk(walk) => Some(walk),
        }
    }
}

/// A complete journey from origin to destination.
///
/// A journey consists of one or more segments (trains and walks).
/// Segments alternate: Train, Walk, Train, Walk, ... with walks only
/// between consecutive trains.
///
/// # Invariants
///
/// - At least one segment
/// - First and last segments are trains (walks only connect trains)
/// - Consecutive segments connect (destination of one = origin of next)
#[derive(Debug, Clone)]
pub struct Journey {
    segments: Vec<Segment>,
}

impl Journey {
    /// Constructs a journey from pre-validated segments.
    ///
    /// # Errors
    ///
    /// Returns `Err` if:
    /// - Segments list is empty
    /// - Segments don't connect (destination != next origin)
    ///
    /// # Examples
    ///
    /// ```
    /// use train_server::domain::{Journey, Segment, Leg, Service, ServiceRef, Call, CallIndex, Crs, RailTime};
    /// use std::sync::Arc;
    /// use chrono::NaiveDate;
    ///
    /// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
    /// let pad = Crs::parse("PAD").unwrap();
    /// let rdg = Crs::parse("RDG").unwrap();
    ///
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
    /// let journey = Journey::new(vec![Segment::Train(leg)]).unwrap();
    ///
    /// assert_eq!(journey.segment_count(), 1);
    /// ```
    pub fn new(segments: Vec<Segment>) -> Result<Self, DomainError> {
        if segments.is_empty() {
            return Err(DomainError::EmptyJourney);
        }

        // Validate segments connect
        for window in segments.windows(2) {
            let prev_dest = window[0].destination();
            let next_origin = window[1].origin();
            if prev_dest != next_origin {
                return Err(DomainError::StationsNotConnected(*prev_dest, *next_origin));
            }
        }

        Ok(Journey { segments })
    }

    /// Constructs a journey from legs, inserting walks where needed.
    ///
    /// This is a convenience constructor that looks up walk durations
    /// and inserts Walk segments between consecutive legs that don't
    /// share a station.
    ///
    /// # Arguments
    ///
    /// * `legs` - The train legs in order
    /// * `walk_duration` - Function to get walk duration between stations,
    ///   returns `None` if stations aren't walkable
    ///
    /// # Errors
    ///
    /// Returns `Err` if consecutive legs don't connect and aren't walkable.
    pub fn from_legs<F>(legs: Vec<Leg>, walk_duration: F) -> Result<Self, DomainError>
    where
        F: Fn(&Crs, &Crs) -> Option<Duration>,
    {
        if legs.is_empty() {
            return Err(DomainError::EmptyJourney);
        }

        let mut segments = Vec::with_capacity(legs.len() * 2);

        for (i, leg) in legs.into_iter().enumerate() {
            if i > 0 {
                // Check if we need a walk between previous leg and this one
                if let Some(Segment::Train(prev_leg)) = segments.last() {
                    let prev_alight = prev_leg.alight_station();
                    let curr_board = leg.board_station();

                    if prev_alight != curr_board {
                        let duration = walk_duration(prev_alight, curr_board)
                            .ok_or(DomainError::StationsNotConnected(*prev_alight, *curr_board))?;
                        segments.push(Segment::Walk(Walk::new(
                            *prev_alight,
                            *curr_board,
                            duration,
                        )));
                    }
                }
            }
            segments.push(Segment::Train(leg));
        }

        Ok(Journey { segments })
    }

    /// Returns all segments in order.
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Returns the number of segments.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Returns the number of train legs (excluding walks).
    pub fn leg_count(&self) -> usize {
        self.segments.iter().filter(|s| s.is_train()).count()
    }

    /// Returns the number of changes (legs - 1, or 0 for direct).
    pub fn change_count(&self) -> usize {
        self.leg_count().saturating_sub(1)
    }

    /// Returns all train legs in order.
    pub fn legs(&self) -> impl Iterator<Item = &Leg> {
        self.segments.iter().filter_map(|s| s.as_leg())
    }

    /// Returns all walks in order.
    pub fn walks(&self) -> impl Iterator<Item = &Walk> {
        self.segments.iter().filter_map(|s| s.as_walk())
    }

    /// Returns the origin station.
    pub fn origin(&self) -> &Crs {
        // Safe: validated non-empty at construction
        self.segments.first().unwrap().origin()
    }

    /// Returns the destination station.
    pub fn destination(&self) -> &Crs {
        // Safe: validated non-empty at construction
        self.segments.last().unwrap().destination()
    }

    /// Returns the departure time (from first leg).
    pub fn departure_time(&self) -> RailTime {
        // Safe: first segment must be a train (validated at construction from legs)
        self.legs().next().unwrap().departure_time()
    }

    /// Returns the arrival time (from last leg).
    pub fn arrival_time(&self) -> RailTime {
        // Safe: last segment must be a train
        self.legs().last().unwrap().arrival_time()
    }

    /// Returns the total journey duration.
    pub fn total_duration(&self) -> Duration {
        self.arrival_time()
            .signed_duration_since(self.departure_time())
    }

    /// Returns the total walking time.
    pub fn total_walk_duration(&self) -> Duration {
        self.walks().map(|w| w.duration).sum()
    }

    /// Returns true if this is a direct journey (no changes).
    pub fn is_direct(&self) -> bool {
        self.leg_count() == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Call, CallIndex, Service, ServiceRef};
    use chrono::NaiveDate;
    use std::sync::Arc;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn time(s: &str) -> RailTime {
        RailTime::parse_hhmm(s, date()).unwrap()
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    fn make_service(
        from_crs: &str,
        from_name: &str,
        to_crs: &str,
        to_name: &str,
        dep: &str,
        arr: &str,
    ) -> Arc<Service> {
        let from = crs(from_crs);
        let to = crs(to_crs);

        let mut call1 = Call::new(from, from_name.into());
        call1.booked_departure = Some(time(dep));

        let mut call2 = Call::new(to, to_name.into());
        call2.booked_arrival = Some(time(arr));

        Arc::new(Service {
            service_ref: ServiceRef::new("SVC".into(), from),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls: vec![call1, call2],
            board_station_idx: CallIndex(0),
        })
    }

    // Walk tests

    #[test]
    fn walk_new() {
        let walk = Walk::new(crs("KGX"), crs("STP"), Duration::minutes(5));

        assert_eq!(walk.from, crs("KGX"));
        assert_eq!(walk.to, crs("STP"));
        assert_eq!(walk.duration, Duration::minutes(5));
    }

    // Segment tests

    #[test]
    fn segment_train() {
        let service = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();
        let segment = Segment::Train(leg);

        assert!(segment.is_train());
        assert!(!segment.is_walk());
        assert!(segment.as_leg().is_some());
        assert!(segment.as_walk().is_none());
        assert_eq!(segment.origin(), &crs("PAD"));
        assert_eq!(segment.destination(), &crs("RDG"));
    }

    #[test]
    fn segment_walk() {
        let walk = Walk::new(crs("KGX"), crs("STP"), Duration::minutes(5));
        let segment = Segment::Walk(walk);

        assert!(!segment.is_train());
        assert!(segment.is_walk());
        assert!(segment.as_leg().is_none());
        assert!(segment.as_walk().is_some());
        assert_eq!(segment.origin(), &crs("KGX"));
        assert_eq!(segment.destination(), &crs("STP"));
        assert_eq!(segment.duration(), Duration::minutes(5));
    }

    // Journey tests

    #[test]
    fn journey_single_leg() {
        let service = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();

        let journey = Journey::new(vec![Segment::Train(leg)]).unwrap();

        assert_eq!(journey.segment_count(), 1);
        assert_eq!(journey.leg_count(), 1);
        assert_eq!(journey.change_count(), 0);
        assert!(journey.is_direct());
        assert_eq!(journey.origin(), &crs("PAD"));
        assert_eq!(journey.destination(), &crs("RDG"));
        assert_eq!(journey.departure_time(), time("10:00"));
        assert_eq!(journey.arrival_time(), time("10:25"));
        assert_eq!(journey.total_duration(), Duration::minutes(25));
    }

    #[test]
    fn journey_with_change_same_station() {
        // PAD -> RDG, then RDG -> SWI
        let service1 = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let service2 = make_service("RDG", "Reading", "SWI", "Swindon", "10:35", "11:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        let journey = Journey::new(vec![Segment::Train(leg1), Segment::Train(leg2)]).unwrap();

        assert_eq!(journey.segment_count(), 2);
        assert_eq!(journey.leg_count(), 2);
        assert_eq!(journey.change_count(), 1);
        assert!(!journey.is_direct());
        assert_eq!(journey.origin(), &crs("PAD"));
        assert_eq!(journey.destination(), &crs("SWI"));
    }

    #[test]
    fn journey_with_walk() {
        // KGX -> CAM, walk to STP, STP -> EUS
        let service1 = make_service("KGX", "King's Cross", "CAM", "Cambridge", "10:00", "11:00");
        let service2 = make_service("STP", "St Pancras", "EUS", "Euston", "11:15", "11:20");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        let walk = Walk::new(crs("CAM"), crs("STP"), Duration::minutes(5));

        let journey = Journey::new(vec![
            Segment::Train(leg1),
            Segment::Walk(walk),
            Segment::Train(leg2),
        ])
        .unwrap();

        assert_eq!(journey.segment_count(), 3);
        assert_eq!(journey.leg_count(), 2);
        assert_eq!(journey.change_count(), 1);
        assert_eq!(journey.total_walk_duration(), Duration::minutes(5));
    }

    #[test]
    fn journey_from_legs_direct() {
        let service = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();

        // No walk needed for single leg
        let journey = Journey::from_legs(vec![leg], |_, _| None).unwrap();

        assert_eq!(journey.leg_count(), 1);
        assert!(journey.is_direct());
    }

    #[test]
    fn journey_from_legs_same_station() {
        let service1 = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let service2 = make_service("RDG", "Reading", "SWI", "Swindon", "10:35", "11:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        // No walk needed - same station
        let journey = Journey::from_legs(vec![leg1, leg2], |_, _| None).unwrap();

        assert_eq!(journey.segment_count(), 2); // No walk inserted
        assert_eq!(journey.leg_count(), 2);
    }

    #[test]
    fn journey_from_legs_with_walk() {
        let service1 = make_service("PAD", "Paddington", "KGX", "King's Cross", "10:00", "10:30");
        let service2 = make_service("STP", "St Pancras", "LEI", "Leicester", "10:45", "12:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        // Walk from KGX to STP
        let journey = Journey::from_legs(vec![leg1, leg2], |from, to| {
            if from.as_str() == "KGX" && to.as_str() == "STP" {
                Some(Duration::minutes(5))
            } else {
                None
            }
        })
        .unwrap();

        assert_eq!(journey.segment_count(), 3); // Leg, Walk, Leg
        assert_eq!(journey.leg_count(), 2);
        assert_eq!(journey.walks().count(), 1);
    }

    #[test]
    fn journey_from_legs_not_walkable() {
        let service1 = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let service2 = make_service("EUS", "Euston", "MAN", "Manchester", "11:00", "13:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        // RDG to EUS not walkable
        let result = Journey::from_legs(vec![leg1, leg2], |_, _| None);

        assert!(matches!(
            result,
            Err(DomainError::StationsNotConnected(_, _))
        ));
    }

    #[test]
    fn journey_empty_segments() {
        let result = Journey::new(vec![]);
        assert!(matches!(result, Err(DomainError::EmptyJourney)));
    }

    #[test]
    fn journey_disconnected_segments() {
        let service1 = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let service2 = make_service("EUS", "Euston", "MAN", "Manchester", "11:00", "13:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        // RDG doesn't connect to EUS
        let result = Journey::new(vec![Segment::Train(leg1), Segment::Train(leg2)]);

        assert!(matches!(
            result,
            Err(DomainError::StationsNotConnected(_, _))
        ));
    }

    #[test]
    fn journey_legs_iterator() {
        let service1 = make_service("PAD", "Paddington", "RDG", "Reading", "10:00", "10:25");
        let service2 = make_service("RDG", "Reading", "SWI", "Swindon", "10:35", "11:00");

        let leg1 = Leg::new(service1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(service2, CallIndex(0), CallIndex(1)).unwrap();

        let journey = Journey::new(vec![Segment::Train(leg1), Segment::Train(leg2)]).unwrap();

        let legs: Vec<_> = journey.legs().collect();
        assert_eq!(legs.len(), 2);
        assert_eq!(legs[0].board_station(), &crs("PAD"));
        assert_eq!(legs[1].board_station(), &crs("RDG"));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::domain::{Call, CallIndex, Service, ServiceRef};
    use chrono::{NaiveDate, NaiveTime};
    use proptest::prelude::*;
    use std::sync::Arc;

    fn fixed_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_time(hour: u32, min: u32) -> RailTime {
        let time = NaiveTime::from_hms_opt(hour % 24, min % 60, 0).unwrap();
        RailTime::new(fixed_date(), time)
    }

    fn crs_from_idx(i: usize) -> Crs {
        let c1 = b'A' + ((i / 676) % 26) as u8;
        let c2 = b'A' + ((i / 26) % 26) as u8;
        let c3 = b'A' + (i % 26) as u8;
        let s = format!("{}{}{}", c1 as char, c2 as char, c3 as char);
        Crs::parse(&s).unwrap()
    }

    /// Generate a valid service from station i to station i+1 with given times.
    fn make_simple_service(station_idx: usize, dep_mins: u16, duration_mins: u16) -> Arc<Service> {
        let from_crs = crs_from_idx(station_idx);
        let to_crs = crs_from_idx(station_idx + 1);

        let dep_hour = (dep_mins / 60) as u32 % 24;
        let dep_min = (dep_mins % 60) as u32;
        let arr_mins = dep_mins + duration_mins;
        let arr_hour = (arr_mins / 60) as u32 % 24;
        let arr_min = (arr_mins % 60) as u32;

        let mut call1 = Call::new(from_crs, format!("Station {}", station_idx));
        call1.booked_departure = Some(make_time(dep_hour, dep_min));

        let mut call2 = Call::new(to_crs, format!("Station {}", station_idx + 1));
        call2.booked_arrival = Some(make_time(arr_hour, arr_min));

        Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{}", station_idx), from_crs),
            headcode: None,
            operator: "Test".into(),
            operator_code: None,
            calls: vec![call1, call2],
            board_station_idx: CallIndex(0),
        })
    }

    /// Strategy for generating a chain of connected legs.
    /// Returns a vec of (station_idx, departure_mins, duration_mins) tuples.
    fn connected_legs_params(max_legs: usize) -> impl Strategy<Value = Vec<(usize, u16, u16)>> {
        (1usize..=max_legs).prop_flat_map(|num_legs| {
            proptest::collection::vec(
                (
                    0usize..100, // station_idx (will be adjusted to be sequential)
                    0u16..1300,  // dep_mins
                    10u16..120,  // duration
                ),
                num_legs,
            )
        })
    }

    /// Build a journey from leg parameters, ensuring times are sequential.
    fn build_journey_from_params(params: &[(usize, u16, u16)]) -> Option<Journey> {
        if params.is_empty() {
            return None;
        }

        let mut legs = Vec::new();
        // Use the first param's dep_mins but constrain to avoid wrapping
        let mut current_time_mins = params[0].1.min(600); // Start no later than 10:00

        for (current_station, &(_, _, duration)) in params.iter().enumerate() {
            // Ensure time progresses and doesn't exceed day boundary
            // to avoid times wrapping and confusing the comparisons
            if current_time_mins + duration >= 1440 {
                // Would wrap past midnight - reject this case
                return None;
            }

            let service = make_simple_service(current_station, current_time_mins, duration);
            let leg = Leg::new(service, CallIndex(0), CallIndex(1)).ok()?;
            legs.push(leg);

            // Next leg starts at next station, after some connection time
            current_time_mins = current_time_mins + duration + 10; // 10 min connection
        }

        Journey::from_legs(legs, |_, _| None).ok()
    }

    proptest! {
        /// Property: departure_time <= arrival_time for all valid journeys.
        #[test]
        fn departure_before_arrival(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                prop_assert!(
                    journey.departure_time() <= journey.arrival_time(),
                    "Departure {:?} should be <= arrival {:?}",
                    journey.departure_time(),
                    journey.arrival_time()
                );
            }
        }

        /// Property: change_count == leg_count - 1 for journeys with only trains.
        #[test]
        fn change_count_is_legs_minus_one(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                let expected_changes = journey.leg_count().saturating_sub(1);
                prop_assert_eq!(
                    journey.change_count(),
                    expected_changes,
                    "change_count {} != leg_count {} - 1",
                    journey.change_count(),
                    journey.leg_count()
                );
            }
        }

        /// Property: total_duration == arrival_time - departure_time.
        #[test]
        fn duration_equals_time_difference(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                let expected = journey.arrival_time()
                    .signed_duration_since(journey.departure_time());
                prop_assert_eq!(
                    journey.total_duration(),
                    expected,
                    "total_duration {:?} != arrival - departure {:?}",
                    journey.total_duration(),
                    expected
                );
            }
        }

        /// Property: origin() == first segment's origin.
        #[test]
        fn origin_is_first_segment_origin(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                prop_assert_eq!(
                    journey.origin(),
                    journey.segments().first().unwrap().origin()
                );
            }
        }

        /// Property: destination() == last segment's destination.
        #[test]
        fn destination_is_last_segment_destination(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                prop_assert_eq!(
                    journey.destination(),
                    journey.segments().last().unwrap().destination()
                );
            }
        }

        /// Property: leg_count equals number of train segments.
        #[test]
        fn leg_count_equals_train_segments(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                let train_count = journey.segments()
                    .iter()
                    .filter(|s| s.is_train())
                    .count();
                prop_assert_eq!(journey.leg_count(), train_count);
            }
        }

        /// Property: consecutive segments connect (destination == next origin).
        #[test]
        fn segments_are_connected(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                for window in journey.segments().windows(2) {
                    prop_assert_eq!(
                        window[0].destination(),
                        window[1].origin(),
                        "Segment ending at {:?} should connect to segment starting at {:?}",
                        window[0].destination(),
                        window[1].origin()
                    );
                }
            }
        }

        /// Property: is_direct iff leg_count == 1.
        #[test]
        fn is_direct_iff_single_leg(params in connected_legs_params(4)) {
            if let Some(journey) = build_journey_from_params(&params) {
                prop_assert_eq!(
                    journey.is_direct(),
                    journey.leg_count() == 1,
                    "is_direct() = {} but leg_count = {}",
                    journey.is_direct(),
                    journey.leg_count()
                );
            }
        }
    }

    // Test with instrumentation to verify property test coverage
    #[test]
    fn journey_properties_distribution() {
        use proptest::test_runner::{Config, TestRunner};
        use std::cell::Cell;

        let mut runner = TestRunner::new(Config::with_cases(200));
        let single_leg = Cell::new(0u32);
        let multi_leg = Cell::new(0u32);
        let total = Cell::new(0u32);

        let _ = runner.run(&connected_legs_params(4), |params| {
            if let Some(journey) = build_journey_from_params(&params) {
                total.set(total.get() + 1);
                if journey.leg_count() == 1 {
                    single_leg.set(single_leg.get() + 1);
                } else {
                    multi_leg.set(multi_leg.get() + 1);
                }
            }
            Ok(())
        });

        // Verify we're testing both single and multi-leg journeys
        assert!(single_leg.get() > 0, "Should test some single-leg journeys");
        assert!(multi_leg.get() > 0, "Should test some multi-leg journeys");
        println!(
            "Journey distribution: {} single-leg, {} multi-leg out of {} valid",
            single_leg.get(),
            multi_leg.get(),
            total.get()
        );
    }
}
