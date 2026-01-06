//! RealTimeTrains URL generation.
//!
//! Generates links to RealTimeTrains for service verification.
//! Since Darwin doesn't provide train UIDs, we link to RTT's search
//! page rather than directly to a service.

use chrono::NaiveDate;

use crate::domain::{Crs, RailTime};

/// Generate an RTT search URL for services at a station around a given time.
///
/// This creates a URL to RTT's detailed search page, showing departures
/// from the station within a time window around the specified time.
///
/// # Arguments
///
/// * `station` - The station to search from
/// * `date` - The date of travel
/// * `time` - The approximate departure time
/// * `window_mins` - Minutes either side of `time` to include (default: 15)
///
/// # Example
///
/// ```ignore
/// // For a train departing Woodbridge at 10:23 on 2026-01-03:
/// let url = rtt_search_url(&crs("WDB"), date, time, 15);
/// // Returns: "https://www.realtimetrains.co.uk/search/detailed/WDB/2026-01-03/1008-1038"
/// ```
pub fn rtt_search_url(station: &Crs, date: NaiveDate, time: RailTime, window_mins: u16) -> String {
    let mins = (time.hour() * 60 + time.minute()) as u16;
    let start_mins = mins.saturating_sub(window_mins);
    let end_mins = (mins + window_mins).min(1439); // Cap at 23:59

    format!(
        "https://www.realtimetrains.co.uk/search/detailed/{}/{}/{:02}{:02}-{:02}{:02}",
        station.as_str(),
        date.format("%Y-%m-%d"),
        start_mins / 60,
        start_mins % 60,
        end_mins / 60,
        end_mins % 60,
    )
}

/// Generate an RTT search URL with a default 15-minute window.
pub fn rtt_search_url_default(station: &Crs, date: NaiveDate, time: RailTime) -> String {
    rtt_search_url(station, date, time, 15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 3).unwrap()
    }

    fn time(h: u32, m: u32) -> RailTime {
        let t = NaiveTime::from_hms_opt(h, m, 0).unwrap();
        RailTime::new(date(), t)
    }

    #[test]
    fn basic_url() {
        let url = rtt_search_url(&crs("WDB"), date(), time(10, 23), 15);
        assert_eq!(
            url,
            "https://www.realtimetrains.co.uk/search/detailed/WDB/2026-01-03/1008-1038"
        );
    }

    #[test]
    fn early_morning_clamps_to_zero() {
        let url = rtt_search_url(&crs("PAD"), date(), time(0, 10), 15);
        // Start should be 00:00, not -5 minutes
        assert!(url.contains("/0000-0025"));
    }

    #[test]
    fn late_night_clamps_to_2359() {
        let url = rtt_search_url(&crs("PAD"), date(), time(23, 50), 15);
        // End should be 23:59, not 00:05 next day
        assert!(url.contains("/2335-2359"));
    }

    #[test]
    fn custom_window() {
        let url = rtt_search_url(&crs("WDB"), date(), time(12, 0), 30);
        assert!(url.contains("/1130-1230"));
    }

    #[test]
    fn default_window() {
        let url = rtt_search_url_default(&crs("WDB"), date(), time(10, 23));
        // Same as 15-minute window
        assert_eq!(
            url,
            "https://www.realtimetrains.co.uk/search/detailed/WDB/2026-01-03/1008-1038"
        );
    }
}
