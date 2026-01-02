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

        let mut call1 = Call::new(from.clone(), from_name.into());
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
