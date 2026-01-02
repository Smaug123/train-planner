//! Data transfer objects for web requests and responses.

use serde::{Deserialize, Serialize};

use crate::domain::{Journey, Leg, RailTime, Segment, Service, Walk};

/// Request to search for services.
#[derive(Debug, Deserialize)]
pub struct SearchServiceRequest {
    /// Origin station CRS code
    pub origin: String,

    /// Optional destination to filter results
    pub destination: Option<String>,

    /// Time in HH:MM format (defaults to now)
    pub time: Option<String>,

    /// Optional headcode to search for (e.g., "1A23")
    pub headcode: Option<String>,
}

/// A service in search results.
#[derive(Debug, Serialize)]
pub struct ServiceResult {
    /// Darwin service ID (ephemeral)
    pub service_id: String,

    /// Headcode (e.g., "1A23")
    pub headcode: Option<String>,

    /// Operator name
    pub operator: String,

    /// Destination name
    pub destination: String,

    /// Scheduled departure time
    pub scheduled_departure: String,

    /// Expected departure time (may differ from scheduled)
    pub expected_departure: Option<String>,

    /// Platform number
    pub platform: Option<String>,

    /// Whether the service is cancelled
    pub is_cancelled: bool,

    /// Calling points
    pub calls: Vec<CallResult>,
}

/// A calling point in a service.
#[derive(Debug, Serialize)]
pub struct CallResult {
    /// Station CRS code
    pub crs: String,

    /// Station name
    pub name: String,

    /// Scheduled arrival time
    pub scheduled_arrival: Option<String>,

    /// Scheduled departure time
    pub scheduled_departure: Option<String>,

    /// Expected arrival time
    pub expected_arrival: Option<String>,

    /// Expected departure time
    pub expected_departure: Option<String>,

    /// Platform
    pub platform: Option<String>,

    /// Whether this call is cancelled
    pub is_cancelled: bool,

    /// Index in the service calls (for journey planning)
    pub index: usize,
}

/// Response for service search.
#[derive(Debug, Serialize)]
pub struct SearchServiceResponse {
    /// Matching services
    pub services: Vec<ServiceResult>,
}

/// Request to plan a journey.
#[derive(Debug, Deserialize)]
pub struct PlanJourneyRequest {
    /// Darwin service ID of the current train
    pub service_id: String,

    /// Current position index in the service
    pub position: usize,

    /// Destination station CRS code
    pub destination: String,
}

/// A journey option.
#[derive(Debug, Serialize)]
pub struct JourneyResult {
    /// Journey segments
    pub segments: Vec<SegmentResult>,

    /// Departure time from origin
    pub departure_time: String,

    /// Arrival time at destination
    pub arrival_time: String,

    /// Total duration in minutes
    pub duration_mins: i64,

    /// Number of changes
    pub changes: usize,
}

/// A segment of a journey.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum SegmentResult {
    Train(LegResult),
    Walk(WalkResult),
}

/// A train leg in a journey.
#[derive(Debug, Serialize)]
pub struct LegResult {
    /// Operator name
    pub operator: String,

    /// Headcode
    pub headcode: Option<String>,

    /// Origin station
    pub origin: StationInfo,

    /// Destination station
    pub destination: StationInfo,

    /// Intermediate stops
    pub stops: Vec<StationInfo>,
}

/// A walking segment.
#[derive(Debug, Serialize)]
pub struct WalkResult {
    /// From station
    pub from: StationInfo,

    /// To station
    pub to: StationInfo,

    /// Duration in minutes
    pub duration_mins: i64,
}

/// Station information for display.
#[derive(Debug, Serialize)]
pub struct StationInfo {
    /// CRS code
    pub crs: String,

    /// Station name
    pub name: String,

    /// Time at this station
    pub time: Option<String>,

    /// Platform
    pub platform: Option<String>,
}

/// Response for journey planning.
#[derive(Debug, Serialize)]
pub struct PlanJourneyResponse {
    /// Found journey options, best first
    pub journeys: Vec<JourneyResult>,

    /// Number of routes explored
    pub routes_explored: usize,
}

/// Error response.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Error message
    pub error: String,
}

// Conversion implementations

impl ServiceResult {
    /// Create from a domain Service.
    pub fn from_service(service: &Service) -> Self {
        let calls: Vec<CallResult> = service
            .calls
            .iter()
            .enumerate()
            .map(|(i, c)| CallResult {
                crs: c.station.as_str().to_string(),
                name: c.station_name.clone(),
                scheduled_arrival: c.booked_arrival.map(|t| format_time(&t)),
                scheduled_departure: c.booked_departure.map(|t| format_time(&t)),
                expected_arrival: c.expected_arrival().map(|t| format_time(&t)),
                expected_departure: c.expected_departure().map(|t| format_time(&t)),
                platform: c.platform.clone(),
                is_cancelled: c.is_cancelled,
                index: i,
            })
            .collect();

        let destination = service
            .calls
            .last()
            .map(|c| c.station_name.clone())
            .unwrap_or_default();

        let scheduled_departure = service
            .calls
            .get(service.board_station_idx.0)
            .and_then(|c| c.booked_departure)
            .map(|t| format_time(&t))
            .unwrap_or_default();

        let expected_departure = service
            .calls
            .get(service.board_station_idx.0)
            .and_then(|c| c.expected_departure())
            .map(|t| format_time(&t));

        let platform = service
            .calls
            .get(service.board_station_idx.0)
            .and_then(|c| c.platform.clone());

        let is_cancelled = service
            .calls
            .get(service.board_station_idx.0)
            .is_some_and(|c| c.is_cancelled);

        Self {
            service_id: service.service_ref.darwin_id.clone(),
            headcode: service.headcode.as_ref().map(|h| h.to_string()),
            operator: service.operator.clone(),
            destination,
            scheduled_departure,
            expected_departure,
            platform,
            is_cancelled,
            calls,
        }
    }
}

impl JourneyResult {
    /// Create from a domain Journey.
    pub fn from_journey(journey: &Journey) -> Self {
        let segments: Vec<SegmentResult> = journey
            .segments()
            .iter()
            .map(|s| match s {
                Segment::Train(leg) => SegmentResult::Train(LegResult::from_leg(leg)),
                Segment::Walk(walk) => SegmentResult::Walk(WalkResult::from_walk(walk)),
            })
            .collect();

        Self {
            segments,
            departure_time: format_time(&journey.departure_time()),
            arrival_time: format_time(&journey.arrival_time()),
            duration_mins: journey.total_duration().num_minutes(),
            changes: journey.change_count(),
        }
    }
}

impl LegResult {
    /// Create from a domain Leg.
    pub fn from_leg(leg: &Leg) -> Self {
        let origin = StationInfo {
            crs: leg.board_call().station.as_str().to_string(),
            name: leg.board_call().station_name.clone(),
            time: leg
                .board_call()
                .expected_departure()
                .map(|t| format_time(&t)),
            platform: leg.board_call().platform.clone(),
        };

        let destination = StationInfo {
            crs: leg.alight_call().station.as_str().to_string(),
            name: leg.alight_call().station_name.clone(),
            time: leg
                .alight_call()
                .expected_arrival()
                .map(|t| format_time(&t)),
            platform: leg.alight_call().platform.clone(),
        };

        // Get intermediate stops (exclude board and alight)
        let all_calls = leg.calls();
        let stops: Vec<StationInfo> = if all_calls.len() > 2 {
            all_calls[1..all_calls.len() - 1]
                .iter()
                .map(|c| StationInfo {
                    crs: c.station.as_str().to_string(),
                    name: c.station_name.clone(),
                    time: c.expected_arrival().map(|t| format_time(&t)),
                    platform: c.platform.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };

        Self {
            operator: leg.service().operator.clone(),
            headcode: leg.service().headcode.as_ref().map(|h| h.to_string()),
            origin,
            destination,
            stops,
        }
    }
}

impl WalkResult {
    /// Create from a domain Walk.
    pub fn from_walk(walk: &Walk) -> Self {
        Self {
            from: StationInfo {
                crs: walk.from.as_str().to_string(),
                name: walk.from.as_str().to_string(), // We don't have the name
                time: None,
                platform: None,
            },
            to: StationInfo {
                crs: walk.to.as_str().to_string(),
                name: walk.to.as_str().to_string(), // We don't have the name
                time: None,
                platform: None,
            },
            duration_mins: walk.duration.num_minutes(),
        }
    }
}

/// Format a RailTime as "HH:MM".
fn format_time(time: &RailTime) -> String {
    time.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Call, CallIndex, Crs, Service, ServiceRef};
    use chrono::{Duration, NaiveDate, NaiveTime};
    use std::sync::Arc;

    fn fixed_date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_time(hour: u32, min: u32) -> RailTime {
        let time = NaiveTime::from_hms_opt(hour, min, 0).unwrap();
        RailTime::new(fixed_date(), time)
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    fn make_test_service() -> Service {
        let mut calls = vec![
            Call::new(crs("PAD"), "London Paddington".into()),
            Call::new(crs("RDG"), "Reading".into()),
            Call::new(crs("SWI"), "Swindon".into()),
            Call::new(crs("BRI"), "Bristol Temple Meads".into()),
        ];

        calls[0].booked_departure = Some(make_time(10, 0));
        calls[0].platform = Some("1".into());
        calls[1].booked_arrival = Some(make_time(10, 25));
        calls[1].booked_departure = Some(make_time(10, 27));
        calls[2].booked_arrival = Some(make_time(10, 52));
        calls[2].booked_departure = Some(make_time(10, 54));
        calls[3].booked_arrival = Some(make_time(11, 30));
        calls[3].platform = Some("3".into());

        Service {
            service_ref: ServiceRef::new("ABC123".into(), crs("PAD")),
            headcode: crate::domain::Headcode::parse("1A23"),
            operator: "Great Western Railway".into(),
            operator_code: crate::domain::AtocCode::parse("GW").ok(),
            calls,
            board_station_idx: CallIndex(0),
        }
    }

    #[test]
    fn service_result_from_service() {
        let service = make_test_service();
        let result = ServiceResult::from_service(&service);

        assert_eq!(result.service_id, "ABC123");
        assert_eq!(result.headcode, Some("1A23".to_string()));
        assert_eq!(result.operator, "Great Western Railway");
        assert_eq!(result.destination, "Bristol Temple Meads");
        assert_eq!(result.scheduled_departure, "10:00");
        assert_eq!(result.platform, Some("1".to_string()));
        assert!(!result.is_cancelled);
        assert_eq!(result.calls.len(), 4);
    }

    #[test]
    fn call_result_fields() {
        let service = make_test_service();
        let result = ServiceResult::from_service(&service);

        // Check first call (origin)
        let call0 = &result.calls[0];
        assert_eq!(call0.crs, "PAD");
        assert_eq!(call0.name, "London Paddington");
        assert_eq!(call0.scheduled_departure, Some("10:00".to_string()));
        assert_eq!(call0.scheduled_arrival, None);
        assert_eq!(call0.index, 0);

        // Check middle call
        let call1 = &result.calls[1];
        assert_eq!(call1.crs, "RDG");
        assert_eq!(call1.scheduled_arrival, Some("10:25".to_string()));
        assert_eq!(call1.scheduled_departure, Some("10:27".to_string()));
        assert_eq!(call1.index, 1);

        // Check last call (destination)
        let call3 = &result.calls[3];
        assert_eq!(call3.crs, "BRI");
        assert_eq!(call3.scheduled_arrival, Some("11:30".to_string()));
        assert_eq!(call3.scheduled_departure, None);
        assert_eq!(call3.index, 3);
    }

    #[test]
    fn leg_result_from_leg() {
        let service = Arc::new(make_test_service());
        let leg = Leg::new(service, CallIndex(0), CallIndex(3)).unwrap();
        let result = LegResult::from_leg(&leg);

        assert_eq!(result.operator, "Great Western Railway");
        assert_eq!(result.headcode, Some("1A23".to_string()));
        assert_eq!(result.origin.crs, "PAD");
        assert_eq!(result.origin.name, "London Paddington");
        assert_eq!(result.destination.crs, "BRI");
        assert_eq!(result.destination.name, "Bristol Temple Meads");

        // Should have 2 intermediate stops (RDG, SWI)
        assert_eq!(result.stops.len(), 2);
        assert_eq!(result.stops[0].crs, "RDG");
        assert_eq!(result.stops[1].crs, "SWI");
    }

    #[test]
    fn leg_result_direct() {
        // A direct leg with no intermediate stops
        let service = Arc::new(make_test_service());
        let leg = Leg::new(service, CallIndex(0), CallIndex(1)).unwrap();
        let result = LegResult::from_leg(&leg);

        assert_eq!(result.origin.crs, "PAD");
        assert_eq!(result.destination.crs, "RDG");
        assert!(result.stops.is_empty());
    }

    #[test]
    fn walk_result_from_walk() {
        let walk = Walk::new(crs("KGX"), crs("STP"), Duration::minutes(5));
        let result = WalkResult::from_walk(&walk);

        assert_eq!(result.from.crs, "KGX");
        assert_eq!(result.to.crs, "STP");
        assert_eq!(result.duration_mins, 5);
    }

    #[test]
    fn journey_result_from_journey() {
        let service1 = Arc::new(make_test_service());
        let leg = Leg::new(service1, CallIndex(0), CallIndex(3)).unwrap();
        let journey = Journey::new(vec![Segment::Train(leg)]).unwrap();
        let result = JourneyResult::from_journey(&journey);

        assert_eq!(result.departure_time, "10:00");
        assert_eq!(result.arrival_time, "11:30");
        assert_eq!(result.duration_mins, 90);
        assert_eq!(result.changes, 0);
        assert_eq!(result.segments.len(), 1);

        match &result.segments[0] {
            SegmentResult::Train(leg_result) => {
                assert_eq!(leg_result.origin.crs, "PAD");
                assert_eq!(leg_result.destination.crs, "BRI");
            }
            SegmentResult::Walk(_) => panic!("Expected Train segment"),
        }
    }

    #[test]
    fn format_time_test() {
        let time = make_time(14, 30);
        assert_eq!(format_time(&time), "14:30");

        let time = make_time(9, 5);
        assert_eq!(format_time(&time), "09:05");
    }
}

/// Tests that demonstrate bugs in the current implementation.
#[cfg(test)]
mod bug_tests {
    use super::*;
    use crate::domain::Crs;
    use chrono::Duration;

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    /// BUG: WalkResult uses CRS codes as station names.
    ///
    /// The Walk type only stores CRS codes, not station names.
    /// WalkResult::from_walk has to use CRS codes as names, which is
    /// unhelpful for display purposes.
    #[test]
    fn bug_walk_result_uses_crs_as_name() {
        let walk = Walk::new(crs("KGX"), crs("STP"), Duration::minutes(5));
        let result = WalkResult::from_walk(&walk);

        // The name should be the human-readable station name, not the CRS code
        // But because Walk doesn't store names, we get CRS codes instead
        assert_ne!(
            result.from.name, "King's Cross",
            "Expected station name, got CRS code instead"
        );
        assert_ne!(
            result.to.name, "St Pancras International",
            "Expected station name, got CRS code instead"
        );

        // This documents the actual (buggy) behavior:
        assert_eq!(
            result.from.name, "KGX",
            "Walk.from.name is CRS code, not name"
        );
        assert_eq!(result.to.name, "STP", "Walk.to.name is CRS code, not name");
    }

    /// BUG: WalkResult has no time information.
    ///
    /// Walks have a duration but no specific start/end times in the domain model.
    /// This means WalkResult can't show when the walk starts or ends.
    #[test]
    fn bug_walk_result_has_no_times() {
        let walk = Walk::new(crs("KGX"), crs("STP"), Duration::minutes(5));
        let result = WalkResult::from_walk(&walk);

        // We know the duration, but not when it happens
        assert!(result.from.time.is_none(), "Walk start time is unknown");
        assert!(result.to.time.is_none(), "Walk end time is unknown");

        // A proper implementation would calculate these based on the
        // arrival time of the previous leg and the walk duration
    }
}
