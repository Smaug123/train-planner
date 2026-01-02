//! Rail time handling for Darwin API.
//!
//! Darwin provides times as "HH:MM" strings. This module provides types for
//! working with these times in a date-aware manner, handling overnight
//! services that cross midnight.

use chrono::{Duration, NaiveDate, NaiveTime, Timelike};
use std::cmp::Ordering;
use std::fmt;
use std::ops::Add;

/// Error returned when parsing an invalid time string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid time: {reason}")]
pub struct TimeError {
    reason: &'static str,
}

impl TimeError {
    fn new(reason: &'static str) -> Self {
        Self { reason }
    }
}

/// A date-aware time for rail services.
///
/// Rail times need to track both the time of day and the date, because
/// overnight services cross midnight. Two times at "01:30" might be on
/// different dates.
///
/// # Examples
///
/// ```
/// use train_server::domain::RailTime;
/// use chrono::NaiveDate;
///
/// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
/// let time = RailTime::parse_hhmm("14:30", date).unwrap();
/// assert_eq!(time.to_string(), "14:30");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RailTime {
    date: NaiveDate,
    time: NaiveTime,
}

impl RailTime {
    /// Create a new RailTime from date and time components.
    pub fn new(date: NaiveDate, time: NaiveTime) -> Self {
        Self { date, time }
    }

    /// Parse a time from "HH:MM" format with a given base date.
    ///
    /// # Examples
    ///
    /// ```
    /// use train_server::domain::RailTime;
    /// use chrono::NaiveDate;
    ///
    /// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
    ///
    /// // Valid times
    /// assert!(RailTime::parse_hhmm("00:00", date).is_ok());
    /// assert!(RailTime::parse_hhmm("23:59", date).is_ok());
    /// assert!(RailTime::parse_hhmm("14:30", date).is_ok());
    ///
    /// // Invalid formats
    /// assert!(RailTime::parse_hhmm("1430", date).is_err());
    /// assert!(RailTime::parse_hhmm("14:3", date).is_err());
    /// assert!(RailTime::parse_hhmm("25:00", date).is_err());
    /// ```
    pub fn parse_hhmm(s: &str, date: NaiveDate) -> Result<Self, TimeError> {
        // Must be exactly 5 characters: HH:MM
        if s.len() != 5 {
            return Err(TimeError::new("expected HH:MM format"));
        }

        let bytes = s.as_bytes();

        // Check colon position
        if bytes[2] != b':' {
            return Err(TimeError::new("expected colon at position 2"));
        }

        // Parse hours
        let hour =
            parse_two_digits(&bytes[0..2]).ok_or_else(|| TimeError::new("invalid hour digits"))?;
        if hour > 23 {
            return Err(TimeError::new("hour must be 0-23"));
        }

        // Parse minutes
        let minute = parse_two_digits(&bytes[3..5])
            .ok_or_else(|| TimeError::new("invalid minute digits"))?;
        if minute > 59 {
            return Err(TimeError::new("minute must be 0-59"));
        }

        let time = NaiveTime::from_hms_opt(hour, minute, 0)
            .ok_or_else(|| TimeError::new("invalid time"))?;

        Ok(Self { date, time })
    }

    /// Returns the date component.
    pub fn date(&self) -> NaiveDate {
        self.date
    }

    /// Returns the time component.
    pub fn time(&self) -> NaiveTime {
        self.time
    }

    /// Returns the hour (0-23).
    pub fn hour(&self) -> u32 {
        self.time.hour()
    }

    /// Returns the minute (0-59).
    pub fn minute(&self) -> u32 {
        self.time.minute()
    }

    /// Converts to a NaiveDateTime.
    pub fn to_datetime(&self) -> chrono::NaiveDateTime {
        self.date.and_time(self.time)
    }

    /// Add a duration to this time.
    ///
    /// This properly handles crossing midnight by advancing the date.
    ///
    /// # Examples
    ///
    /// ```
    /// use train_server::domain::RailTime;
    /// use chrono::{Duration, NaiveDate};
    ///
    /// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
    /// let time = RailTime::parse_hhmm("23:30", date).unwrap();
    ///
    /// // Adding 1 hour crosses midnight
    /// let later = time + Duration::hours(1);
    /// assert_eq!(later.to_string(), "00:30");
    /// assert_eq!(later.date(), NaiveDate::from_ymd_opt(2024, 3, 16).unwrap());
    /// ```
    pub fn checked_add(&self, duration: Duration) -> Option<Self> {
        let dt = self.to_datetime().checked_add_signed(duration)?;
        Some(Self {
            date: dt.date(),
            time: dt.time(),
        })
    }

    /// Subtract a duration from this time.
    pub fn checked_sub(&self, duration: Duration) -> Option<Self> {
        let dt = self.to_datetime().checked_sub_signed(duration)?;
        Some(Self {
            date: dt.date(),
            time: dt.time(),
        })
    }

    /// Returns the duration between two times.
    ///
    /// Returns a negative duration if `other` is before `self`.
    pub fn signed_duration_since(&self, other: Self) -> Duration {
        self.to_datetime()
            .signed_duration_since(other.to_datetime())
    }
}

impl Add<Duration> for RailTime {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        self.checked_add(rhs).expect("time overflow")
    }
}

impl Ord for RailTime {
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_datetime().cmp(&other.to_datetime())
    }
}

impl PartialOrd for RailTime {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Debug for RailTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RailTime({} {:02}:{:02})",
            self.date,
            self.hour(),
            self.minute()
        )
    }
}

impl fmt::Display for RailTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.hour(), self.minute())
    }
}

/// Parse two ASCII digit bytes into a u32.
fn parse_two_digits(bytes: &[u8]) -> Option<u32> {
    if bytes.len() != 2 {
        return None;
    }
    let d1 = (bytes[0] as char).to_digit(10)?;
    let d2 = (bytes[1] as char).to_digit(10)?;
    Some(d1 * 10 + d2)
}

/// Threshold for detecting midnight rollover in time sequences.
///
/// If a time appears more than 6 hours before the previous time in the
/// sequence, we assume it has rolled over to the next day.
const ROLLOVER_THRESHOLD_HOURS: i64 = 6;

/// Parse a sequence of times with rollover detection for overnight services.
///
/// Darwin provides calling point times as "HH:MM" strings in chronological
/// order. For overnight services that cross midnight, times will appear to
/// go backwards (e.g., "23:30", "00:15"). This function detects such
/// rollovers and assigns the correct date to each time.
///
/// The rollover detection uses a threshold: if a time appears more than
/// 6 hours earlier than the previous time, it's assumed to be on the next day.
///
/// # Arguments
///
/// * `times` - A sequence of optional time strings. `None` entries are preserved.
/// * `base_date` - The date to use for the first time in the sequence.
///
/// # Examples
///
/// ```
/// use train_server::domain::parse_time_sequence;
/// use chrono::NaiveDate;
///
/// let date = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
///
/// // Normal daytime service - all on same day
/// let times = vec![Some("10:00"), Some("10:30"), Some("11:00")];
/// let parsed = parse_time_sequence(&times, date).unwrap();
/// assert_eq!(parsed[0].unwrap().date(), date);
/// assert_eq!(parsed[1].unwrap().date(), date);
/// assert_eq!(parsed[2].unwrap().date(), date);
///
/// // Overnight service - crosses midnight
/// let times = vec![Some("23:00"), Some("23:30"), Some("00:15"), Some("01:00")];
/// let parsed = parse_time_sequence(&times, date).unwrap();
/// assert_eq!(parsed[0].unwrap().date(), date);
/// assert_eq!(parsed[1].unwrap().date(), date);
/// let next_day = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();
/// assert_eq!(parsed[2].unwrap().date(), next_day);  // Rolled over
/// assert_eq!(parsed[3].unwrap().date(), next_day);
/// ```
pub fn parse_time_sequence(
    times: &[Option<&str>],
    base_date: NaiveDate,
) -> Result<Vec<Option<RailTime>>, TimeError> {
    let mut result = Vec::with_capacity(times.len());
    let mut current_date = base_date;
    let mut prev_time: Option<NaiveTime> = None;

    for time_opt in times {
        match time_opt {
            None => {
                result.push(None);
            }
            Some(time_str) => {
                let parsed = RailTime::parse_hhmm(time_str, base_date)?;
                let time = parsed.time();

                // Check for rollover: if this time is more than 6 hours before
                // the previous time, we've crossed midnight
                if let Some(prev) = prev_time {
                    let prev_minutes = prev.hour() as i64 * 60 + prev.minute() as i64;
                    let curr_minutes = time.hour() as i64 * 60 + time.minute() as i64;
                    let diff_minutes = curr_minutes - prev_minutes;

                    // If current time is more than 6 hours "before" previous,
                    // assume we crossed midnight
                    if diff_minutes < -(ROLLOVER_THRESHOLD_HOURS * 60) {
                        current_date = current_date
                            .succ_opt()
                            .ok_or_else(|| TimeError::new("date overflow"))?;
                    }
                }

                result.push(Some(RailTime::new(current_date, time)));
                prev_time = Some(time);
            }
        }
    }

    Ok(result)
}

/// Parse a sequence of times going backwards in time (for previous calling points).
///
/// Darwin provides previous calling points in reverse chronological order
/// (most recent first, going backwards to origin). This function handles
/// that by detecting when times appear significantly later than the previous,
/// indicating we've crossed midnight going backwards.
///
/// # Arguments
///
/// * `times` - A sequence of optional time strings, in reverse chronological order.
/// * `base_date` - The date of the most recent (first) time in the sequence.
///
/// # Examples
///
/// ```
/// use train_server::domain::parse_time_sequence_reverse;
/// use chrono::NaiveDate;
///
/// let date = NaiveDate::from_ymd_opt(2024, 3, 16).unwrap();
///
/// // Previous stops going backwards, crossing midnight
/// // Order: current station at 00:30, then 00:00, 23:30, 23:00
/// let times = vec![Some("00:30"), Some("00:00"), Some("23:30"), Some("23:00")];
/// let parsed = parse_time_sequence_reverse(&times, date).unwrap();
///
/// assert_eq!(parsed[0].unwrap().date(), date);  // 00:30 on 16th
/// assert_eq!(parsed[1].unwrap().date(), date);  // 00:00 on 16th
/// let prev_day = NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
/// assert_eq!(parsed[2].unwrap().date(), prev_day);  // 23:30 on 15th
/// assert_eq!(parsed[3].unwrap().date(), prev_day);  // 23:00 on 15th
/// ```
pub fn parse_time_sequence_reverse(
    times: &[Option<&str>],
    base_date: NaiveDate,
) -> Result<Vec<Option<RailTime>>, TimeError> {
    let mut result = Vec::with_capacity(times.len());
    let mut current_date = base_date;
    let mut prev_time: Option<NaiveTime> = None;

    for time_opt in times {
        match time_opt {
            None => {
                result.push(None);
            }
            Some(time_str) => {
                let parsed = RailTime::parse_hhmm(time_str, base_date)?;
                let time = parsed.time();

                // Check for rollover going backwards: if this time is more than
                // 6 hours after the previous time, we've crossed midnight backwards
                if let Some(prev) = prev_time {
                    let prev_minutes = prev.hour() as i64 * 60 + prev.minute() as i64;
                    let curr_minutes = time.hour() as i64 * 60 + time.minute() as i64;
                    let diff_minutes = curr_minutes - prev_minutes;

                    // If current time is more than 6 hours "after" previous,
                    // and we're going backwards, we crossed midnight
                    if diff_minutes > ROLLOVER_THRESHOLD_HOURS * 60 {
                        current_date = current_date
                            .pred_opt()
                            .ok_or_else(|| TimeError::new("date underflow"))?;
                    }
                }

                result.push(Some(RailTime::new(current_date, time)));
                prev_time = Some(time);
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn parse_valid_times() {
        let d = date(2024, 3, 15);

        let t = RailTime::parse_hhmm("00:00", d).unwrap();
        assert_eq!(t.hour(), 0);
        assert_eq!(t.minute(), 0);

        let t = RailTime::parse_hhmm("23:59", d).unwrap();
        assert_eq!(t.hour(), 23);
        assert_eq!(t.minute(), 59);

        let t = RailTime::parse_hhmm("14:30", d).unwrap();
        assert_eq!(t.hour(), 14);
        assert_eq!(t.minute(), 30);
    }

    #[test]
    fn parse_invalid_format() {
        let d = date(2024, 3, 15);

        // Wrong length
        assert!(RailTime::parse_hhmm("1430", d).is_err());
        assert!(RailTime::parse_hhmm("14:3", d).is_err());
        assert!(RailTime::parse_hhmm("14:300", d).is_err());

        // Missing colon
        assert!(RailTime::parse_hhmm("14-30", d).is_err());
        assert!(RailTime::parse_hhmm("14.30", d).is_err());

        // Non-digit characters
        assert!(RailTime::parse_hhmm("ab:cd", d).is_err());
        assert!(RailTime::parse_hhmm("1a:30", d).is_err());
    }

    #[test]
    fn parse_invalid_values() {
        let d = date(2024, 3, 15);

        // Hour out of range
        assert!(RailTime::parse_hhmm("24:00", d).is_err());
        assert!(RailTime::parse_hhmm("25:00", d).is_err());

        // Minute out of range
        assert!(RailTime::parse_hhmm("12:60", d).is_err());
        assert!(RailTime::parse_hhmm("12:99", d).is_err());
    }

    #[test]
    fn display_format() {
        let d = date(2024, 3, 15);

        assert_eq!(
            RailTime::parse_hhmm("00:00", d).unwrap().to_string(),
            "00:00"
        );
        assert_eq!(
            RailTime::parse_hhmm("09:05", d).unwrap().to_string(),
            "09:05"
        );
        assert_eq!(
            RailTime::parse_hhmm("23:59", d).unwrap().to_string(),
            "23:59"
        );
    }

    #[test]
    fn ordering() {
        let d1 = date(2024, 3, 15);
        let d2 = date(2024, 3, 16);

        let t1 = RailTime::parse_hhmm("10:00", d1).unwrap();
        let t2 = RailTime::parse_hhmm("11:00", d1).unwrap();
        let t3 = RailTime::parse_hhmm("09:00", d2).unwrap();

        // Same day ordering
        assert!(t1 < t2);
        assert!(t2 > t1);

        // Cross-day: later date wins even with earlier time
        assert!(t3 > t1);
        assert!(t3 > t2);
    }

    #[test]
    fn add_duration() {
        let d = date(2024, 3, 15);

        // Simple addition
        let t = RailTime::parse_hhmm("10:00", d).unwrap();
        let t2 = t + Duration::hours(2);
        assert_eq!(t2.to_string(), "12:00");
        assert_eq!(t2.date(), d);

        // Add minutes
        let t = RailTime::parse_hhmm("10:30", d).unwrap();
        let t2 = t + Duration::minutes(45);
        assert_eq!(t2.to_string(), "11:15");
    }

    #[test]
    fn add_duration_crosses_midnight() {
        let d = date(2024, 3, 15);
        let t = RailTime::parse_hhmm("23:30", d).unwrap();

        let t2 = t + Duration::hours(1);
        assert_eq!(t2.to_string(), "00:30");
        assert_eq!(t2.date(), date(2024, 3, 16));
    }

    #[test]
    fn duration_between() {
        let d = date(2024, 3, 15);

        let t1 = RailTime::parse_hhmm("10:00", d).unwrap();
        let t2 = RailTime::parse_hhmm("12:30", d).unwrap();

        let dur = t2.signed_duration_since(t1);
        assert_eq!(dur, Duration::hours(2) + Duration::minutes(30));

        let dur_neg = t1.signed_duration_since(t2);
        assert_eq!(dur_neg, -(Duration::hours(2) + Duration::minutes(30)));
    }

    #[test]
    fn equality() {
        let d = date(2024, 3, 15);

        let t1 = RailTime::parse_hhmm("14:30", d).unwrap();
        let t2 = RailTime::parse_hhmm("14:30", d).unwrap();
        let t3 = RailTime::parse_hhmm("14:31", d).unwrap();

        assert_eq!(t1, t2);
        assert_ne!(t1, t3);
    }

    #[test]
    fn hash_consistent() {
        use std::collections::HashSet;
        let d = date(2024, 3, 15);

        let mut set = HashSet::new();
        set.insert(RailTime::parse_hhmm("14:30", d).unwrap());

        assert!(set.contains(&RailTime::parse_hhmm("14:30", d).unwrap()));
        assert!(!set.contains(&RailTime::parse_hhmm("14:31", d).unwrap()));
    }

    // Time sequence parsing tests

    #[test]
    fn sequence_same_day() {
        let d = date(2024, 3, 15);
        let times = vec![Some("10:00"), Some("10:30"), Some("11:00"), Some("12:00")];

        let parsed = parse_time_sequence(&times, d).unwrap();

        assert_eq!(parsed.len(), 4);
        for p in &parsed {
            assert_eq!(p.unwrap().date(), d);
        }

        // Check times are in order
        assert_eq!(parsed[0].unwrap().to_string(), "10:00");
        assert_eq!(parsed[1].unwrap().to_string(), "10:30");
        assert_eq!(parsed[2].unwrap().to_string(), "11:00");
        assert_eq!(parsed[3].unwrap().to_string(), "12:00");
    }

    #[test]
    fn sequence_crosses_midnight() {
        let d = date(2024, 3, 15);
        let times = vec![Some("23:00"), Some("23:30"), Some("00:15"), Some("01:00")];

        let parsed = parse_time_sequence(&times, d).unwrap();
        let next_day = date(2024, 3, 16);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
        assert_eq!(parsed[2].unwrap().date(), next_day);
        assert_eq!(parsed[3].unwrap().date(), next_day);

        // Verify chronological ordering
        assert!(parsed[0].unwrap() < parsed[1].unwrap());
        assert!(parsed[1].unwrap() < parsed[2].unwrap());
        assert!(parsed[2].unwrap() < parsed[3].unwrap());
    }

    #[test]
    fn sequence_with_none_values() {
        let d = date(2024, 3, 15);
        let times = vec![Some("10:00"), None, Some("11:00"), None, Some("12:00")];

        let parsed = parse_time_sequence(&times, d).unwrap();

        assert_eq!(parsed.len(), 5);
        assert!(parsed[0].is_some());
        assert!(parsed[1].is_none());
        assert!(parsed[2].is_some());
        assert!(parsed[3].is_none());
        assert!(parsed[4].is_some());
    }

    #[test]
    fn sequence_none_across_midnight() {
        let d = date(2024, 3, 15);
        // None value spans the midnight crossing
        let times = vec![Some("23:30"), None, Some("00:30")];

        let parsed = parse_time_sequence(&times, d).unwrap();
        let next_day = date(2024, 3, 16);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert!(parsed[1].is_none());
        assert_eq!(parsed[2].unwrap().date(), next_day);
    }

    #[test]
    fn sequence_empty() {
        let d = date(2024, 3, 15);
        let times: Vec<Option<&str>> = vec![];

        let parsed = parse_time_sequence(&times, d).unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn sequence_all_none() {
        let d = date(2024, 3, 15);
        let times: Vec<Option<&str>> = vec![None, None, None];

        let parsed = parse_time_sequence(&times, d).unwrap();
        assert_eq!(parsed.len(), 3);
        assert!(parsed.iter().all(|p| p.is_none()));
    }

    #[test]
    fn sequence_small_time_gap_no_rollover() {
        let d = date(2024, 3, 15);
        // Going from 10:00 to 08:00 is only 2 hours "back", not rollover
        // (This would be unusual in practice, but tests the threshold)
        let times = vec![Some("10:00"), Some("08:00")];

        let parsed = parse_time_sequence(&times, d).unwrap();

        // Both should be on the same day (no rollover for <6 hour gap)
        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
    }

    #[test]
    fn sequence_exactly_at_threshold() {
        let d = date(2024, 3, 15);
        // Going from 12:00 to 06:00 is exactly 6 hours back
        // (at the threshold, should NOT rollover)
        let times = vec![Some("12:00"), Some("06:00")];

        let parsed = parse_time_sequence(&times, d).unwrap();

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
    }

    #[test]
    fn sequence_just_over_threshold() {
        let d = date(2024, 3, 15);
        // Going from 12:00 to 05:59 is >6 hours back
        // (just over threshold, SHOULD rollover)
        let times = vec![Some("12:00"), Some("05:59")];

        let parsed = parse_time_sequence(&times, d).unwrap();
        let next_day = date(2024, 3, 16);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), next_day);
    }

    // Reverse sequence tests

    #[test]
    fn sequence_reverse_same_day() {
        let d = date(2024, 3, 15);
        let times = vec![Some("12:00"), Some("11:30"), Some("11:00"), Some("10:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();

        assert_eq!(parsed.len(), 4);
        for p in &parsed {
            assert_eq!(p.unwrap().date(), d);
        }
    }

    #[test]
    fn sequence_reverse_crosses_midnight() {
        let d = date(2024, 3, 16);
        // Going backwards: 00:30 on 16th, then 00:00, then 23:30, 23:00 on 15th
        let times = vec![Some("00:30"), Some("00:00"), Some("23:30"), Some("23:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();
        let prev_day = date(2024, 3, 15);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
        assert_eq!(parsed[2].unwrap().date(), prev_day);
        assert_eq!(parsed[3].unwrap().date(), prev_day);

        // Verify reverse chronological ordering
        assert!(parsed[0].unwrap() > parsed[1].unwrap());
        assert!(parsed[1].unwrap() > parsed[2].unwrap());
        assert!(parsed[2].unwrap() > parsed[3].unwrap());
    }

    #[test]
    fn sequence_reverse_with_none() {
        let d = date(2024, 3, 16);
        let times = vec![Some("00:30"), None, Some("23:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();
        let prev_day = date(2024, 3, 15);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert!(parsed[1].is_none());
        assert_eq!(parsed[2].unwrap().date(), prev_day);
    }

    #[test]
    fn sequence_reverse_small_gap_no_rollover() {
        let d = date(2024, 3, 15);
        // Going from 08:00 to 10:00 going backwards is only 2 hours "forward"
        let times = vec![Some("08:00"), Some("10:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();

        // Both should be on the same day (no rollover for <6 hour gap)
        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
    }

    #[test]
    fn sequence_reverse_at_threshold() {
        let d = date(2024, 3, 15);
        // Going from 06:00 to 12:00 is exactly 6 hours forward
        let times = vec![Some("06:00"), Some("12:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), d);
    }

    #[test]
    fn sequence_reverse_just_over_threshold() {
        let d = date(2024, 3, 16);
        // Going from 05:59 to 12:00 is >6 hours forward
        let times = vec![Some("05:59"), Some("12:00")];

        let parsed = parse_time_sequence_reverse(&times, d).unwrap();
        let prev_day = date(2024, 3, 15);

        assert_eq!(parsed[0].unwrap().date(), d);
        assert_eq!(parsed[1].unwrap().date(), prev_day);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    prop_compose! {
        fn valid_time()(hour in 0u32..24, minute in 0u32..60) -> String {
            format!("{:02}:{:02}", hour, minute)
        }
    }

    prop_compose! {
        fn valid_date()(
            year in 2000i32..2100,
            month in 1u32..=12,
            day in 1u32..=28  // Safe for all months
        ) -> NaiveDate {
            NaiveDate::from_ymd_opt(year, month, day).unwrap()
        }
    }

    proptest! {
        /// Any valid HH:MM string parses successfully
        #[test]
        fn valid_hhmm_parses(time_str in valid_time(), date in valid_date()) {
            prop_assert!(RailTime::parse_hhmm(&time_str, date).is_ok());
        }

        /// Parse then display roundtrips
        #[test]
        fn parse_display_roundtrip(time_str in valid_time(), date in valid_date()) {
            let parsed = RailTime::parse_hhmm(&time_str, date).unwrap();
            prop_assert_eq!(parsed.to_string(), time_str);
        }

        /// Ordering is transitive
        #[test]
        fn ordering_transitive(
            h1 in 0u32..24, m1 in 0u32..60,
            h2 in 0u32..24, m2 in 0u32..60,
            h3 in 0u32..24, m3 in 0u32..60,
            date in valid_date()
        ) {
            let t1 = RailTime::new(date, NaiveTime::from_hms_opt(h1, m1, 0).unwrap());
            let t2 = RailTime::new(date, NaiveTime::from_hms_opt(h2, m2, 0).unwrap());
            let t3 = RailTime::new(date, NaiveTime::from_hms_opt(h3, m3, 0).unwrap());

            if t1 <= t2 && t2 <= t3 {
                prop_assert!(t1 <= t3);
            }
        }

        /// Adding then subtracting same duration returns original
        #[test]
        fn add_sub_identity(
            time_str in valid_time(),
            date in valid_date(),
            minutes in 0i64..1000
        ) {
            let t = RailTime::parse_hhmm(&time_str, date).unwrap();
            let dur = Duration::minutes(minutes);

            if let Some(added) = t.checked_add(dur) {
                if let Some(result) = added.checked_sub(dur) {
                    prop_assert_eq!(t, result);
                }
            }
        }

        /// Duration between is consistent with ordering
        #[test]
        fn duration_ordering_consistent(
            h1 in 0u32..24, m1 in 0u32..60,
            h2 in 0u32..24, m2 in 0u32..60,
            date in valid_date()
        ) {
            let t1 = RailTime::new(date, NaiveTime::from_hms_opt(h1, m1, 0).unwrap());
            let t2 = RailTime::new(date, NaiveTime::from_hms_opt(h2, m2, 0).unwrap());

            let dur = t2.signed_duration_since(t1);

            match t1.cmp(&t2) {
                Ordering::Less => prop_assert!(dur > Duration::zero()),
                Ordering::Greater => prop_assert!(dur < Duration::zero()),
                Ordering::Equal => prop_assert!(dur == Duration::zero()),
            }
        }

        /// Invalid hour is rejected
        #[test]
        fn invalid_hour_rejected(hour in 24u32..100, minute in 0u32..60, date in valid_date()) {
            let s = format!("{:02}:{:02}", hour, minute);
            prop_assert!(RailTime::parse_hhmm(&s, date).is_err());
        }

        /// Invalid minute is rejected
        #[test]
        fn invalid_minute_rejected(hour in 0u32..24, minute in 60u32..100, date in valid_date()) {
            let s = format!("{:02}:{:02}", hour, minute);
            prop_assert!(RailTime::parse_hhmm(&s, date).is_err());
        }

        /// Forward sequence preserves length
        #[test]
        fn sequence_preserves_length(
            times in prop::collection::vec(prop::option::of(valid_time()), 0..10),
            date in valid_date()
        ) {
            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence(&time_refs, date).unwrap();
            prop_assert_eq!(parsed.len(), times.len());
        }

        /// Reverse sequence preserves length
        #[test]
        fn sequence_reverse_preserves_length(
            times in prop::collection::vec(prop::option::of(valid_time()), 0..10),
            date in valid_date()
        ) {
            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence_reverse(&time_refs, date).unwrap();
            prop_assert_eq!(parsed.len(), times.len());
        }

        /// None values are preserved in forward sequence
        #[test]
        fn sequence_preserves_none(
            times in prop::collection::vec(prop::option::of(valid_time()), 0..10),
            date in valid_date()
        ) {
            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence(&time_refs, date).unwrap();

            for (i, (orig, result)) in times.iter().zip(parsed.iter()).enumerate() {
                prop_assert_eq!(
                    orig.is_some(),
                    result.is_some(),
                    "Mismatch at index {}: orig={:?}, result={:?}",
                    i, orig, result
                );
            }
        }

        /// None values are preserved in reverse sequence
        #[test]
        fn sequence_reverse_preserves_none(
            times in prop::collection::vec(prop::option::of(valid_time()), 0..10),
            date in valid_date()
        ) {
            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence_reverse(&time_refs, date).unwrap();

            for (i, (orig, result)) in times.iter().zip(parsed.iter()).enumerate() {
                prop_assert_eq!(
                    orig.is_some(),
                    result.is_some(),
                    "Mismatch at index {}: orig={:?}, result={:?}",
                    i, orig, result
                );
            }
        }

        /// Monotonic forward sequence stays on same day
        #[test]
        fn monotonic_forward_same_day(
            start_hour in 0u32..18,
            num_stops in 1usize..6,
            date in valid_date()
        ) {
            // Generate strictly increasing times within the same day
            let mut times = Vec::new();
            for i in 0..num_stops {
                let h = start_hour + i as u32;
                if h < 24 {
                    times.push(Some(format!("{:02}:00", h)));
                }
            }

            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence(&time_refs, date).unwrap();

            // All should be on the same day
            for p in parsed.iter().flatten() {
                prop_assert_eq!(p.date(), date);
            }
        }

        /// Monotonic reverse sequence stays on same day
        #[test]
        fn monotonic_reverse_same_day(
            start_hour in 6u32..24,
            num_stops in 1usize..6,
            date in valid_date()
        ) {
            // Generate strictly decreasing times within the same day
            let mut times = Vec::new();
            for i in 0..num_stops {
                let h = start_hour.saturating_sub(i as u32);
                times.push(Some(format!("{:02}:00", h)));
            }

            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence_reverse(&time_refs, date).unwrap();

            // All should be on the same day
            for p in parsed.iter().flatten() {
                prop_assert_eq!(p.date(), date);
            }
        }

        /// Forward sequence crossing midnight increments date exactly once
        #[test]
        fn forward_midnight_crossing(
            pre_midnight_hour in 22u32..24,
            post_midnight_hour in 0u32..4,
            date in valid_date()
        ) {
            let times = vec![
                Some(format!("{:02}:00", pre_midnight_hour)),
                Some(format!("{:02}:00", post_midnight_hour)),
            ];

            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence(&time_refs, date).unwrap();

            if let Some(next_day) = date.succ_opt() {
                prop_assert_eq!(parsed[0].unwrap().date(), date);
                prop_assert_eq!(parsed[1].unwrap().date(), next_day);

                // Verify chronological order
                prop_assert!(parsed[0].unwrap() < parsed[1].unwrap());
            }
        }

        /// Reverse sequence crossing midnight decrements date exactly once
        #[test]
        fn reverse_midnight_crossing(
            post_midnight_hour in 0u32..4,
            pre_midnight_hour in 22u32..24,
            date in valid_date()
        ) {
            let times = vec![
                Some(format!("{:02}:00", post_midnight_hour)),
                Some(format!("{:02}:00", pre_midnight_hour)),
            ];

            let time_refs: Vec<Option<&str>> = times.iter()
                .map(|o| o.as_deref())
                .collect();
            let parsed = parse_time_sequence_reverse(&time_refs, date).unwrap();

            if let Some(prev_day) = date.pred_opt() {
                prop_assert_eq!(parsed[0].unwrap().date(), date);
                prop_assert_eq!(parsed[1].unwrap().date(), prev_day);

                // Verify reverse chronological order
                prop_assert!(parsed[0].unwrap() > parsed[1].unwrap());
            }
        }
    }
}
