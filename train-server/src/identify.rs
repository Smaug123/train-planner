//! Train identification logic.
//!
//! This module contains the core logic for identifying a user's current train
//! based on observable information (next station, terminus).

use std::sync::Arc;

use crate::darwin::ConvertedService;
use crate::domain::{Crs, MatchConfidence};

/// A matched train with its confidence level.
#[derive(Debug, Clone)]
pub struct TrainMatch {
    /// The matched service.
    pub service: Arc<ConvertedService>,
    /// How confidently we matched the train.
    pub confidence: MatchConfidence,
}

/// Filter and rank services based on identification criteria.
///
/// Given a list of services from a departure board and optional terminus filter,
/// returns matching services ranked by confidence and departure time.
///
/// # Arguments
///
/// * `services` - Services from the next station's departure board
/// * `terminus` - Optional terminus to filter by (if provided, only services
///   terminating at this station are included)
///
/// # Returns
///
/// Services that match the criteria, sorted by confidence (exact matches first),
/// then by departure time.
pub fn filter_and_rank_matches(
    services: &[Arc<ConvertedService>],
    terminus: Option<&Crs>,
) -> Vec<TrainMatch> {
    let mut matches: Vec<TrainMatch> = services
        .iter()
        .filter_map(|svc| {
            // If terminus specified, check it matches final calling point
            if let Some(term) = terminus {
                let dest = svc.service.destination_call()?;
                if &dest.1.station != term {
                    return None;
                }
            }

            let confidence = if terminus.is_some() {
                MatchConfidence::Exact
            } else {
                MatchConfidence::NextStationOnly
            };

            Some(TrainMatch {
                service: Arc::clone(svc),
                confidence,
            })
        })
        .collect();

    // Sort: exact matches first, then by departure time
    matches.sort_by(|a, b| {
        a.confidence.cmp(&b.confidence).then_with(|| {
            let a_dep = a
                .service
                .candidate
                .expected_departure
                .or(Some(a.service.candidate.scheduled_departure));
            let b_dep = b
                .service
                .candidate
                .expected_departure
                .or(Some(b.service.candidate.scheduled_departure));
            a_dep.cmp(&b_dep)
        })
    });

    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        AtocCode, Call, CallIndex, Headcode, RailTime, Service, ServiceCandidate, ServiceRef,
    };
    use chrono::{NaiveDate, NaiveTime};

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 3).unwrap()
    }

    fn time(h: u32, m: u32) -> RailTime {
        let t = NaiveTime::from_hms_opt(h, m, 0).unwrap();
        RailTime::new(date(), t)
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    /// Create a mock service with the given calling points.
    /// The first station is where we're querying from (board station),
    /// and the last station is the terminus.
    fn mock_service(
        id: &str,
        headcode: &str,
        stations: &[(&str, &str)], // (crs, name) pairs
        departure_time: RailTime,
    ) -> Arc<ConvertedService> {
        let calls: Vec<Call> = stations
            .iter()
            .enumerate()
            .map(|(i, (crs_str, name))| {
                let mut call = Call::new(crs(crs_str), name.to_string());
                if i == 0 {
                    call.booked_departure = Some(departure_time);
                } else if i == stations.len() - 1 {
                    call.booked_arrival =
                        Some(departure_time + chrono::Duration::minutes(30 * i as i64));
                } else {
                    call.booked_arrival =
                        Some(departure_time + chrono::Duration::minutes(15 * i as i64));
                    call.booked_departure =
                        Some(departure_time + chrono::Duration::minutes(15 * i as i64 + 2));
                }
                call
            })
            .collect();

        let first_crs = crs(stations[0].0);
        let service = Service {
            service_ref: ServiceRef::new(id.to_string(), first_crs),
            headcode: Headcode::parse(headcode),
            operator: "Test Operator".to_string(),
            operator_code: AtocCode::parse("TO").ok(),
            calls,
            board_station_idx: CallIndex(0),
        };

        let destination_name = stations
            .last()
            .map(|(_, n)| n.to_string())
            .unwrap_or_default();
        let destination_crs = stations.last().map(|(c, _)| crs(c));

        let candidate = ServiceCandidate {
            service_ref: service.service_ref.clone(),
            headcode: service.headcode,
            scheduled_departure: departure_time,
            expected_departure: None,
            destination: destination_name,
            destination_crs,
            operator: "Test Operator".to_string(),
            operator_code: service.operator_code,
            platform: Some("1".to_string()),
            is_cancelled: false,
        };

        Arc::new(ConvertedService { service, candidate })
    }

    #[test]
    fn no_services_returns_empty() {
        let services: Vec<Arc<ConvertedService>> = vec![];
        let matches = filter_and_rank_matches(&services, None);
        assert!(matches.is_empty());
    }

    #[test]
    fn no_terminus_filter_returns_all() {
        let services = vec![
            mock_service(
                "svc1",
                "1P01",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 0),
            ),
            mock_service(
                "svc2",
                "1P02",
                &[("WDB", "Woodbridge"), ("LST", "London Liverpool Street")],
                time(10, 15),
            ),
        ];

        let matches = filter_and_rank_matches(&services, None);

        assert_eq!(matches.len(), 2);
        assert!(
            matches
                .iter()
                .all(|m| m.confidence == MatchConfidence::NextStationOnly)
        );
    }

    #[test]
    fn terminus_filter_excludes_non_matching() {
        let services = vec![
            mock_service(
                "svc1",
                "1P01",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 0),
            ),
            mock_service(
                "svc2",
                "1P02",
                &[("WDB", "Woodbridge"), ("LST", "London Liverpool Street")],
                time(10, 15),
            ),
            mock_service(
                "svc3",
                "1P03",
                &[
                    ("WDB", "Woodbridge"),
                    ("FLX", "Felixstowe"),
                    ("IPS", "Ipswich"),
                ],
                time(10, 30),
            ),
        ];

        let matches = filter_and_rank_matches(&services, Some(&crs("IPS")));

        assert_eq!(matches.len(), 2);
        assert!(
            matches
                .iter()
                .all(|m| { m.service.service.destination_call().unwrap().1.station == crs("IPS") })
        );
        assert!(
            matches
                .iter()
                .all(|m| m.confidence == MatchConfidence::Exact)
        );
    }

    #[test]
    fn terminus_filter_no_matches_returns_empty() {
        let services = vec![mock_service(
            "svc1",
            "1P01",
            &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
            time(10, 0),
        )];

        let matches = filter_and_rank_matches(&services, Some(&crs("LST")));

        assert!(matches.is_empty());
    }

    #[test]
    fn sorted_by_departure_time() {
        let services = vec![
            mock_service(
                "svc1",
                "1P01",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 30),
            ),
            mock_service(
                "svc2",
                "1P02",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 0),
            ),
            mock_service(
                "svc3",
                "1P03",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 15),
            ),
        ];

        let matches = filter_and_rank_matches(&services, Some(&crs("IPS")));

        assert_eq!(matches.len(), 3);
        // Should be sorted by time: 10:00, 10:15, 10:30
        assert_eq!(matches[0].service.service.service_ref.darwin_id, "svc2");
        assert_eq!(matches[1].service.service.service_ref.darwin_id, "svc3");
        assert_eq!(matches[2].service.service.service_ref.darwin_id, "svc1");
    }

    #[test]
    fn exact_matches_sorted_before_partial() {
        // This tests that if we somehow had mixed confidence levels,
        // exact matches come first. In practice, with terminus filter
        // all matches are exact, and without filter all are partial.
        // But this documents the intended behavior.
        let services = vec![mock_service(
            "svc1",
            "1P01",
            &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
            time(10, 0),
        )];

        // With terminus filter, should be exact
        let exact_matches = filter_and_rank_matches(&services, Some(&crs("IPS")));
        assert_eq!(exact_matches[0].confidence, MatchConfidence::Exact);

        // Without terminus filter, should be partial
        let partial_matches = filter_and_rank_matches(&services, None);
        assert_eq!(
            partial_matches[0].confidence,
            MatchConfidence::NextStationOnly
        );
    }

    #[test]
    fn single_exact_match_scenario() {
        // Realistic scenario: user is on train to Ipswich, next stop is Woodbridge
        // Only one train to Ipswich is departing from Woodbridge soon
        let services = vec![
            mock_service(
                "liverpool_st",
                "1P10",
                &[("WDB", "Woodbridge"), ("LST", "London Liverpool Street")],
                time(10, 0),
            ),
            mock_service(
                "ipswich",
                "2P15",
                &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
                time(10, 5),
            ),
            mock_service(
                "felixstowe",
                "2F20",
                &[("WDB", "Woodbridge"), ("FLX", "Felixstowe")],
                time(10, 10),
            ),
        ];

        let matches = filter_and_rank_matches(&services, Some(&crs("IPS")));

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].service.service.service_ref.darwin_id, "ipswich");
        assert_eq!(matches[0].confidence, MatchConfidence::Exact);
    }

    #[test]
    fn multiple_trains_to_same_terminus() {
        // Scenario: multiple trains to same terminus (common on busy lines)
        let services = vec![
            mock_service(
                "fast",
                "1P01",
                &[("RDG", "Reading"), ("PAD", "London Paddington")],
                time(10, 0),
            ),
            mock_service(
                "slow",
                "2P02",
                &[
                    ("RDG", "Reading"),
                    ("SLO", "Slough"),
                    ("PAD", "London Paddington"),
                ],
                time(10, 5),
            ),
            mock_service(
                "semi_fast",
                "1P03",
                &[("RDG", "Reading"), ("PAD", "London Paddington")],
                time(10, 10),
            ),
        ];

        let matches = filter_and_rank_matches(&services, Some(&crs("PAD")));

        // All three go to Paddington, sorted by departure time
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0].service.service.service_ref.darwin_id, "fast");
        assert_eq!(matches[1].service.service.service_ref.darwin_id, "slow");
        assert_eq!(
            matches[2].service.service.service_ref.darwin_id,
            "semi_fast"
        );
    }

    #[test]
    fn long_distance_train_with_many_stops() {
        // Scenario: user on a long-distance train with many stops
        let services = vec![
            mock_service(
                "ecml_express",
                "1E01",
                &[
                    ("PBO", "Peterborough"),
                    ("GRA", "Grantham"),
                    ("NEW", "Newark North Gate"),
                    ("DON", "Doncaster"),
                    ("YRK", "York"),
                    ("DAR", "Darlington"),
                    ("NCL", "Newcastle"),
                    ("EDI", "Edinburgh"),
                ],
                time(10, 0),
            ),
            mock_service(
                "local",
                "2E05",
                &[("PBO", "Peterborough"), ("GRA", "Grantham")],
                time(10, 15),
            ),
        ];

        // User wants Edinburgh - only the express goes there
        let matches = filter_and_rank_matches(&services, Some(&crs("EDI")));

        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].service.service.service_ref.darwin_id,
            "ecml_express"
        );
    }

    #[test]
    fn preserves_service_details() {
        let services = vec![mock_service(
            "test_svc",
            "1A23",
            &[("WDB", "Woodbridge"), ("IPS", "Ipswich")],
            time(10, 23),
        )];

        let matches = filter_and_rank_matches(&services, Some(&crs("IPS")));

        assert_eq!(matches.len(), 1);
        let matched = &matches[0];

        // Verify service details are preserved
        assert_eq!(matched.service.service.service_ref.darwin_id, "test_svc");
        assert_eq!(
            matched
                .service
                .service
                .headcode
                .as_ref()
                .unwrap()
                .to_string(),
            "1A23"
        );
        assert_eq!(matched.service.service.operator, "Test Operator");
        assert_eq!(matched.service.candidate.destination, "Ipswich");
        assert_eq!(matched.service.candidate.scheduled_departure, time(10, 23));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::domain::{
        Call, CallIndex, Headcode, RailTime, Service, ServiceCandidate, ServiceRef,
    };
    use chrono::{NaiveDate, NaiveTime};
    use proptest::prelude::*;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 3).unwrap()
    }

    /// Generate a valid 3-letter CRS code
    fn arb_crs() -> impl Strategy<Value = Crs> {
        "[A-Z]{3}".prop_map(|s| Crs::parse(&s).unwrap())
    }

    /// Generate a valid time
    fn arb_time() -> impl Strategy<Value = RailTime> {
        (0u32..24, 0u32..60).prop_map(|(h, m)| {
            let t = NaiveTime::from_hms_opt(h, m, 0).unwrap();
            RailTime::new(date(), t)
        })
    }

    /// Generate a mock service with random calling points
    fn arb_service() -> impl Strategy<Value = Arc<ConvertedService>> {
        (
            "[a-z0-9]{8}",        // id
            "[0-9][A-Z][0-9]{2}", // headcode
            arb_crs(),            // origin (board station)
            arb_crs(),            // terminus
            arb_time(),           // departure time
        )
            .prop_map(|(id, headcode, origin, terminus, dep_time)| {
                let calls = vec![
                    {
                        let mut c = Call::new(origin, "Origin".to_string());
                        c.booked_departure = Some(dep_time);
                        c
                    },
                    {
                        let mut c = Call::new(terminus, "Terminus".to_string());
                        c.booked_arrival = Some(dep_time + chrono::Duration::minutes(30));
                        c
                    },
                ];

                let service = Service {
                    service_ref: ServiceRef::new(id.clone(), origin),
                    headcode: Headcode::parse(&headcode),
                    operator: "Test".to_string(),
                    operator_code: None,
                    calls,
                    board_station_idx: CallIndex(0),
                };

                let candidate = ServiceCandidate {
                    service_ref: service.service_ref.clone(),
                    headcode: service.headcode,
                    scheduled_departure: dep_time,
                    expected_departure: None,
                    destination: "Terminus".to_string(),
                    destination_crs: Some(terminus),
                    operator: "Test".to_string(),
                    operator_code: None,
                    platform: None,
                    is_cancelled: false,
                };

                Arc::new(ConvertedService { service, candidate })
            })
    }

    proptest! {
        /// Filtering with no terminus returns all services
        #[test]
        fn no_filter_returns_all(services in prop::collection::vec(arb_service(), 0..10)) {
            let matches = filter_and_rank_matches(&services, None::<&Crs>);
            prop_assert_eq!(matches.len(), services.len());
        }

        /// All matches without terminus filter have NextStationOnly confidence
        #[test]
        fn no_filter_all_partial_confidence(services in prop::collection::vec(arb_service(), 1..10)) {
            let matches = filter_and_rank_matches(&services, None::<&Crs>);
            for m in matches {
                prop_assert_eq!(m.confidence, MatchConfidence::NextStationOnly);
            }
        }

        /// All matches with terminus filter have Exact confidence
        #[test]
        fn with_filter_all_exact_confidence(
            services in prop::collection::vec(arb_service(), 1..10),
            terminus in arb_crs()
        ) {
            let matches = filter_and_rank_matches(&services, Some(&terminus));
            for m in matches {
                prop_assert_eq!(m.confidence, MatchConfidence::Exact);
            }
        }

        /// Filtering only includes services with matching terminus
        #[test]
        fn filter_only_matching_terminus(
            services in prop::collection::vec(arb_service(), 1..20),
            terminus in arb_crs()
        ) {
            let matches = filter_and_rank_matches(&services, Some(&terminus));

            for m in &matches {
                let dest = m.service.service.destination_call()
                    .expect("service should have destination");
                prop_assert_eq!(&dest.1.station, &terminus);
            }
        }

        /// Output is sorted by departure time
        #[test]
        fn output_sorted_by_time(services in prop::collection::vec(arb_service(), 0..10)) {
            let matches = filter_and_rank_matches(&services, None::<&Crs>);

            for window in matches.windows(2) {
                let a_time = window[0].service.candidate.expected_departure
                    .or(Some(window[0].service.candidate.scheduled_departure));
                let b_time = window[1].service.candidate.expected_departure
                    .or(Some(window[1].service.candidate.scheduled_departure));

                prop_assert!(a_time <= b_time, "Matches should be sorted by departure time");
            }
        }

        /// Number of matches <= number of input services
        #[test]
        fn matches_bounded_by_input(
            services in prop::collection::vec(arb_service(), 0..20),
            terminus in prop::option::of(arb_crs())
        ) {
            let matches = filter_and_rank_matches(&services, terminus.as_ref());
            prop_assert!(matches.len() <= services.len());
        }
    }
}
