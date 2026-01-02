//! Calling point types for train services.
//!
//! A `Call` represents a station stop on a train service, with scheduled
//! and realtime arrival/departure times. A `CallIndex` provides an
//! unambiguous position within a service's calling pattern.

use super::{Crs, RailTime};

/// Index of a call within a service's calling pattern.
///
/// Used instead of `Crs` to disambiguate services that call at the same
/// station multiple times (loops, turnbacks, out-and-back workings).
///
/// # Examples
///
/// ```
/// use train_server::domain::CallIndex;
///
/// let idx = CallIndex(0);
/// assert_eq!(idx.0, 0);
///
/// // CallIndex is Copy, so it's cheap to pass around
/// let idx2 = idx;
/// assert_eq!(idx, idx2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallIndex(pub usize);

impl CallIndex {
    /// Returns the next index.
    pub fn next(self) -> Self {
        CallIndex(self.0 + 1)
    }

    /// Returns the previous index, if any.
    pub fn prev(self) -> Option<Self> {
        self.0.checked_sub(1).map(CallIndex)
    }
}

impl std::fmt::Display for CallIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<usize> for CallIndex {
    fn from(value: usize) -> Self {
        CallIndex(value)
    }
}

impl From<CallIndex> for usize {
    fn from(value: CallIndex) -> Self {
        value.0
    }
}

/// A station call on a train service.
///
/// Represents a single stop with scheduled ("booked") times and realtime
/// estimates or actuals. Darwin provides:
/// - `st` (scheduled time) → `booked_*`
/// - `et` (estimated time) or `at` (actual time) → `realtime_*`
///
/// # Time Semantics
///
/// - For origin stations: only departure times are meaningful
/// - For destination stations: only arrival times are meaningful
/// - For intermediate stations: both arrival and departure may be present
/// - Realtime times override booked times when available
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Station CRS code
    pub station: Crs,
    /// Station display name
    pub station_name: String,
    /// Platform number/letter (if known)
    pub platform: Option<String>,
    /// Scheduled arrival time
    pub booked_arrival: Option<RailTime>,
    /// Scheduled departure time
    pub booked_departure: Option<RailTime>,
    /// Realtime (estimated or actual) arrival time
    pub realtime_arrival: Option<RailTime>,
    /// Realtime (estimated or actual) departure time
    pub realtime_departure: Option<RailTime>,
    /// Whether this call is cancelled
    pub is_cancelled: bool,
}

impl Call {
    /// Creates a new call with the given station and times.
    pub fn new(station: Crs, station_name: String) -> Self {
        Self {
            station,
            station_name,
            platform: None,
            booked_arrival: None,
            booked_departure: None,
            realtime_arrival: None,
            realtime_departure: None,
            is_cancelled: false,
        }
    }

    /// Returns the best available arrival time (realtime if available, else booked).
    ///
    /// # Examples
    ///
    /// ```
    /// use train_server::domain::{Call, Crs, RailTime};
    /// use chrono::NaiveDate;
    ///
    /// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
    /// let crs = Crs::parse("PAD").unwrap();
    ///
    /// let mut call = Call::new(crs, "London Paddington".into());
    /// call.booked_arrival = Some(RailTime::parse_hhmm("14:30", date).unwrap());
    ///
    /// // Without realtime, returns booked
    /// assert_eq!(call.expected_arrival().unwrap().to_string(), "14:30");
    ///
    /// // With realtime, returns realtime
    /// call.realtime_arrival = Some(RailTime::parse_hhmm("14:35", date).unwrap());
    /// assert_eq!(call.expected_arrival().unwrap().to_string(), "14:35");
    /// ```
    pub fn expected_arrival(&self) -> Option<RailTime> {
        self.realtime_arrival.or(self.booked_arrival)
    }

    /// Returns the best available departure time (realtime if available, else booked).
    pub fn expected_departure(&self) -> Option<RailTime> {
        self.realtime_departure.or(self.booked_departure)
    }

    /// Returns the booked arrival time.
    pub fn booked_arrival(&self) -> Option<RailTime> {
        self.booked_arrival
    }

    /// Returns the booked departure time.
    pub fn booked_departure(&self) -> Option<RailTime> {
        self.booked_departure
    }

    /// Returns true if the arrival is delayed (realtime later than booked).
    pub fn is_arrival_delayed(&self) -> bool {
        match (self.realtime_arrival, self.booked_arrival) {
            (Some(rt), Some(booked)) => rt > booked,
            _ => false,
        }
    }

    /// Returns true if the departure is delayed (realtime later than booked).
    pub fn is_departure_delayed(&self) -> bool {
        match (self.realtime_departure, self.booked_departure) {
            (Some(rt), Some(booked)) => rt > booked,
            _ => false,
        }
    }

    /// Returns the arrival delay as a duration, if delayed.
    pub fn arrival_delay(&self) -> Option<chrono::Duration> {
        match (self.realtime_arrival, self.booked_arrival) {
            (Some(rt), Some(booked)) if rt > booked => Some(rt.signed_duration_since(booked)),
            _ => None,
        }
    }

    /// Returns the departure delay as a duration, if delayed.
    pub fn departure_delay(&self) -> Option<chrono::Duration> {
        match (self.realtime_departure, self.booked_departure) {
            (Some(rt), Some(booked)) if rt > booked => Some(rt.signed_duration_since(booked)),
            _ => None,
        }
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

    // CallIndex tests

    #[test]
    fn call_index_next() {
        let idx = CallIndex(5);
        assert_eq!(idx.next(), CallIndex(6));
    }

    #[test]
    fn call_index_prev() {
        let idx = CallIndex(5);
        assert_eq!(idx.prev(), Some(CallIndex(4)));

        let idx = CallIndex(0);
        assert_eq!(idx.prev(), None);
    }

    #[test]
    fn call_index_ordering() {
        let idx1 = CallIndex(1);
        let idx2 = CallIndex(5);
        let idx3 = CallIndex(5);

        assert!(idx1 < idx2);
        assert_eq!(idx2, idx3);
    }

    #[test]
    fn call_index_display() {
        let idx = CallIndex(42);
        assert_eq!(idx.to_string(), "42");
    }

    #[test]
    fn call_index_from_usize() {
        let idx: CallIndex = 10.into();
        assert_eq!(idx.0, 10);

        let val: usize = idx.into();
        assert_eq!(val, 10);
    }

    // Call tests

    #[test]
    fn call_new() {
        let call = Call::new(crs("PAD"), "London Paddington".into());

        assert_eq!(call.station, crs("PAD"));
        assert_eq!(call.station_name, "London Paddington");
        assert!(call.platform.is_none());
        assert!(call.booked_arrival.is_none());
        assert!(call.booked_departure.is_none());
        assert!(call.realtime_arrival.is_none());
        assert!(call.realtime_departure.is_none());
        assert!(!call.is_cancelled);
    }

    #[test]
    fn expected_arrival_prefers_realtime() {
        let mut call = Call::new(crs("PAD"), "London Paddington".into());
        call.booked_arrival = Some(time("14:30"));

        // Without realtime, returns booked
        assert_eq!(call.expected_arrival(), Some(time("14:30")));

        // With realtime, returns realtime
        call.realtime_arrival = Some(time("14:35"));
        assert_eq!(call.expected_arrival(), Some(time("14:35")));
    }

    #[test]
    fn expected_departure_prefers_realtime() {
        let mut call = Call::new(crs("PAD"), "London Paddington".into());
        call.booked_departure = Some(time("14:30"));

        // Without realtime, returns booked
        assert_eq!(call.expected_departure(), Some(time("14:30")));

        // With realtime, returns realtime
        call.realtime_departure = Some(time("14:35"));
        assert_eq!(call.expected_departure(), Some(time("14:35")));
    }

    #[test]
    fn is_delayed() {
        let mut call = Call::new(crs("PAD"), "London Paddington".into());
        call.booked_arrival = Some(time("14:30"));
        call.booked_departure = Some(time("14:32"));

        // Not delayed when no realtime
        assert!(!call.is_arrival_delayed());
        assert!(!call.is_departure_delayed());

        // Not delayed when on time
        call.realtime_arrival = Some(time("14:30"));
        call.realtime_departure = Some(time("14:32"));
        assert!(!call.is_arrival_delayed());
        assert!(!call.is_departure_delayed());

        // Delayed when late
        call.realtime_arrival = Some(time("14:35"));
        call.realtime_departure = Some(time("14:40"));
        assert!(call.is_arrival_delayed());
        assert!(call.is_departure_delayed());

        // Not delayed when early
        call.realtime_arrival = Some(time("14:28"));
        call.realtime_departure = Some(time("14:30"));
        assert!(!call.is_arrival_delayed());
        assert!(!call.is_departure_delayed());
    }

    #[test]
    fn delay_duration() {
        let mut call = Call::new(crs("PAD"), "London Paddington".into());
        call.booked_arrival = Some(time("14:30"));
        call.booked_departure = Some(time("14:32"));

        // No delay when no realtime
        assert!(call.arrival_delay().is_none());
        assert!(call.departure_delay().is_none());

        // No delay when on time
        call.realtime_arrival = Some(time("14:30"));
        call.realtime_departure = Some(time("14:32"));
        assert!(call.arrival_delay().is_none());
        assert!(call.departure_delay().is_none());

        // Delay when late
        call.realtime_arrival = Some(time("14:35"));
        call.realtime_departure = Some(time("14:42"));
        assert_eq!(call.arrival_delay(), Some(chrono::Duration::minutes(5)));
        assert_eq!(call.departure_delay(), Some(chrono::Duration::minutes(10)));

        // No delay when early
        call.realtime_arrival = Some(time("14:28"));
        call.realtime_departure = Some(time("14:30"));
        assert!(call.arrival_delay().is_none());
        assert!(call.departure_delay().is_none());
    }

    #[test]
    fn call_equality() {
        let call1 = {
            let mut c = Call::new(crs("PAD"), "London Paddington".into());
            c.booked_departure = Some(time("14:30"));
            c
        };

        let call2 = {
            let mut c = Call::new(crs("PAD"), "London Paddington".into());
            c.booked_departure = Some(time("14:30"));
            c
        };

        let call3 = {
            let mut c = Call::new(crs("PAD"), "London Paddington".into());
            c.booked_departure = Some(time("14:31"));
            c
        };

        assert_eq!(call1, call2);
        assert_ne!(call1, call3);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// CallIndex next/prev are inverses (when prev succeeds)
        #[test]
        fn call_index_next_prev_inverse(idx in 1usize..1000) {
            let ci = CallIndex(idx);
            prop_assert_eq!(ci.next().prev(), Some(ci));
        }

        /// CallIndex ordering is consistent with inner value
        #[test]
        fn call_index_ordering_consistent(a in 0usize..1000, b in 0usize..1000) {
            let ci_a = CallIndex(a);
            let ci_b = CallIndex(b);

            prop_assert_eq!(ci_a.cmp(&ci_b), a.cmp(&b));
        }
    }
}
