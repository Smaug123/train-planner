//! Journey ranking for search results.
//!
//! Ranks journeys by a combination of factors to present the most useful
//! options first.

use crate::domain::Journey;

/// Rank journeys by preference.
///
/// Journeys are ranked by:
/// 1. Arrival time (earlier is better)
/// 2. Number of changes (fewer is better)
/// 3. Total duration (shorter is better)
///
/// Returns journeys sorted best-first.
pub fn rank_journeys(mut journeys: Vec<Journey>) -> Vec<Journey> {
    journeys.sort_by(|a, b| {
        // Primary: arrival time
        let arr_cmp = a.arrival_time().cmp(&b.arrival_time());
        if arr_cmp != std::cmp::Ordering::Equal {
            return arr_cmp;
        }

        // Secondary: fewer changes
        let changes_cmp = a.change_count().cmp(&b.change_count());
        if changes_cmp != std::cmp::Ordering::Equal {
            return changes_cmp;
        }

        // Tertiary: shorter duration
        a.total_duration().cmp(&b.total_duration())
    });

    journeys
}

/// Remove dominated journeys.
///
/// A journey is dominated if another journey:
/// - Arrives at the same time or earlier
/// - Has the same or fewer changes
/// - Has the same or shorter duration
///
/// This prunes journeys that are strictly worse than others.
pub fn remove_dominated(journeys: Vec<Journey>) -> Vec<Journey> {
    if journeys.len() <= 1 {
        return journeys;
    }

    let mut result = Vec::with_capacity(journeys.len());

    for journey in journeys {
        let dominated = result.iter().any(|existing: &Journey| {
            existing.arrival_time() <= journey.arrival_time()
                && existing.change_count() <= journey.change_count()
                && existing.total_duration() <= journey.total_duration()
                // Must be strictly better in at least one dimension
                && (existing.arrival_time() < journey.arrival_time()
                    || existing.change_count() < journey.change_count()
                    || existing.total_duration() < journey.total_duration())
        });

        if !dominated {
            // Also remove any existing journeys dominated by this one
            result.retain(|existing: &Journey| {
                !(journey.arrival_time() <= existing.arrival_time()
                    && journey.change_count() <= existing.change_count()
                    && journey.total_duration() <= existing.total_duration()
                    && (journey.arrival_time() < existing.arrival_time()
                        || journey.change_count() < existing.change_count()
                        || journey.total_duration() < existing.total_duration()))
            });
            result.push(journey);
        }
    }

    result
}

/// Deduplicate journeys that are effectively identical.
///
/// Two journeys are considered duplicates if they:
/// - Arrive at the same time
/// - Depart at the same time
/// - Have the same number of changes
///
/// When duplicates exist, keeps the one with shortest duration.
pub fn deduplicate(mut journeys: Vec<Journey>) -> Vec<Journey> {
    if journeys.len() <= 1 {
        return journeys;
    }

    // Sort by (arrival, departure, changes, duration) to group duplicates
    journeys.sort_by(|a, b| {
        let arr = a.arrival_time().cmp(&b.arrival_time());
        if arr != std::cmp::Ordering::Equal {
            return arr;
        }
        let dep = a.departure_time().cmp(&b.departure_time());
        if dep != std::cmp::Ordering::Equal {
            return dep;
        }
        let changes = a.change_count().cmp(&b.change_count());
        if changes != std::cmp::Ordering::Equal {
            return changes;
        }
        a.total_duration().cmp(&b.total_duration())
    });

    // Keep first of each (arrival, departure, changes) group
    let mut result = Vec::with_capacity(journeys.len());
    let mut last_key: Option<(_, _, _)> = None;

    for journey in journeys {
        let key = (
            journey.arrival_time(),
            journey.departure_time(),
            journey.change_count(),
        );

        if last_key != Some(key) {
            result.push(journey);
            last_key = Some(key);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Call, CallIndex, Crs, Leg, RailTime, Segment, Service, ServiceRef};
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

    fn make_service(id: &str, calls_data: &[(&str, &str, &str, &str)]) -> Arc<Service> {
        let mut calls: Vec<Call> = calls_data
            .iter()
            .map(|(station, name, arr, dep)| {
                let mut call = Call::new(crs(station), (*name).to_string());
                if !arr.is_empty() {
                    call.booked_arrival = Some(time(arr));
                }
                if !dep.is_empty() {
                    call.booked_departure = Some(time(dep));
                }
                call
            })
            .collect();

        // Ensure first has departure, last has arrival
        if !calls.is_empty() {
            if calls[0].booked_departure.is_none() && calls[0].booked_arrival.is_some() {
                calls[0].booked_departure = calls[0].booked_arrival;
            }
            let last = calls.len() - 1;
            if calls[last].booked_arrival.is_none() && calls[last].booked_departure.is_some() {
                calls[last].booked_arrival = calls[last].booked_departure;
            }
        }

        Arc::new(Service {
            service_ref: ServiceRef::new(id.to_string(), crs("PAD")),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        })
    }

    fn make_journey(legs: Vec<(Arc<Service>, usize, usize)>) -> Journey {
        let legs: Vec<Leg> = legs
            .into_iter()
            .map(|(service, board, alight)| {
                Leg::new(service, CallIndex(board), CallIndex(alight)).unwrap()
            })
            .collect();

        let segments: Vec<Segment> = legs.into_iter().map(Segment::Train).collect();
        Journey::new(segments).unwrap()
    }

    #[test]
    fn rank_by_arrival() {
        // Two direct journeys, different arrival times
        let svc1 = make_service(
            "A",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:30", ""),
            ],
        );
        let svc2 = make_service(
            "B",
            &[
                ("PAD", "Paddington", "", "10:15"),
                ("RDG", "Reading", "10:40", ""),
            ],
        );

        let j1 = make_journey(vec![(svc1, 0, 1)]);
        let j2 = make_journey(vec![(svc2, 0, 1)]);

        let ranked = rank_journeys(vec![j2.clone(), j1.clone()]);

        // Earlier arrival should be first
        assert_eq!(ranked[0].arrival_time(), time("10:30"));
        assert_eq!(ranked[1].arrival_time(), time("10:40"));
    }

    #[test]
    fn rank_by_changes_when_same_arrival() {
        // One direct, one with change, same arrival
        let direct = make_service(
            "D",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("BRI", "Bristol", "11:30", ""),
            ],
        );

        let leg1 = make_service(
            "C1",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:30", ""),
            ],
        );
        let leg2 = make_service(
            "C2",
            &[
                ("RDG", "Reading", "", "10:45"),
                ("BRI", "Bristol", "11:30", ""),
            ],
        );

        let j_direct = make_journey(vec![(direct, 0, 1)]);
        let j_change = make_journey(vec![(leg1, 0, 1), (leg2, 0, 1)]);

        let ranked = rank_journeys(vec![j_change.clone(), j_direct.clone()]);

        // Same arrival, but direct has fewer changes
        assert_eq!(ranked[0].change_count(), 0);
        assert_eq!(ranked[1].change_count(), 1);
    }

    #[test]
    fn remove_dominated_keeps_pareto_optimal() {
        // Journey A: arrives 10:30, 0 changes
        // Journey B: arrives 10:40, 0 changes (dominated by A)
        // Journey C: arrives 10:25, 1 change (not dominated - earlier but more changes)

        let svc_a = make_service(
            "A",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:30", ""),
            ],
        );
        let svc_b = make_service(
            "B",
            &[
                ("PAD", "Paddington", "", "10:10"),
                ("RDG", "Reading", "10:40", ""),
            ],
        );
        let svc_c1 = make_service(
            "C1",
            &[
                ("PAD", "Paddington", "", "09:45"),
                ("SWI", "Swindon", "10:10", ""),
            ],
        );
        let svc_c2 = make_service(
            "C2",
            &[
                ("SWI", "Swindon", "", "10:15"),
                ("RDG", "Reading", "10:25", ""),
            ],
        );

        let j_a = make_journey(vec![(svc_a, 0, 1)]);
        let j_b = make_journey(vec![(svc_b, 0, 1)]);
        let j_c = make_journey(vec![(svc_c1, 0, 1), (svc_c2, 0, 1)]);

        let result = remove_dominated(vec![j_a, j_b, j_c]);

        // B should be removed (dominated by A)
        // A and C should remain (neither dominates the other)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn deduplicate_same_times() {
        // Two journeys with same arrival/departure/changes
        let svc1 = make_service(
            "X",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:30", ""),
            ],
        );
        let svc2 = make_service(
            "Y",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:30", ""),
            ],
        );

        let j1 = make_journey(vec![(svc1, 0, 1)]);
        let j2 = make_journey(vec![(svc2, 0, 1)]);

        let result = deduplicate(vec![j1, j2]);

        // Should keep only one
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn empty_input() {
        assert!(rank_journeys(vec![]).is_empty());
        assert!(remove_dominated(vec![]).is_empty());
        assert!(deduplicate(vec![]).is_empty());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::domain::{Call, CallIndex, Crs, Leg, RailTime, Segment, Service, ServiceRef};
    use chrono::{NaiveDate, NaiveTime};
    use proptest::prelude::*;
    use std::sync::Arc;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_time(hour: u32, min: u32) -> RailTime {
        let time = NaiveTime::from_hms_opt(hour % 24, min % 60, 0).unwrap();
        RailTime::new(date(), time)
    }

    /// Generate a valid service with parameterized times.
    /// dep_mins: departure time in minutes from midnight
    /// duration_mins: journey duration
    fn make_service_with_times(id: u32, dep_mins: u16, duration_mins: u16) -> Arc<Service> {
        let dep_hour = (dep_mins / 60) as u32 % 24;
        let dep_min = (dep_mins % 60) as u32;
        let arr_mins = dep_mins + duration_mins;
        let arr_hour = (arr_mins / 60) as u32 % 24;
        let arr_min = (arr_mins % 60) as u32;

        let dep_time = make_time(dep_hour, dep_min);
        let arr_time = make_time(arr_hour, arr_min);

        let origin_crs = Crs::parse("PAD").unwrap();
        let dest_crs = Crs::parse("RDG").unwrap();

        let mut origin_call = Call::new(origin_crs, "Paddington".to_string());
        origin_call.booked_departure = Some(dep_time);

        let mut dest_call = Call::new(dest_crs, "Reading".to_string());
        dest_call.booked_arrival = Some(arr_time);

        Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{id}"), origin_crs),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls: vec![origin_call, dest_call],
            board_station_idx: CallIndex(0),
        })
    }

    /// Generate a two-leg journey with a change.
    /// Creates PAD -> RDG (change) RDG -> BRI
    fn make_two_leg_journey(
        id: u32,
        dep_mins: u16,
        leg1_duration: u16,
        connection_wait: u16,
        leg2_duration: u16,
    ) -> Journey {
        let dep_hour = (dep_mins / 60) as u32 % 24;
        let dep_min = (dep_mins % 60) as u32;

        let leg1_arr_mins = dep_mins + leg1_duration;
        let leg1_arr_hour = (leg1_arr_mins / 60) as u32 % 24;
        let leg1_arr_min = (leg1_arr_mins % 60) as u32;

        let leg2_dep_mins = leg1_arr_mins + connection_wait;
        let leg2_dep_hour = (leg2_dep_mins / 60) as u32 % 24;
        let leg2_dep_min = (leg2_dep_mins % 60) as u32;

        let leg2_arr_mins = leg2_dep_mins + leg2_duration;
        let leg2_arr_hour = (leg2_arr_mins / 60) as u32 % 24;
        let leg2_arr_min = (leg2_arr_mins % 60) as u32;

        let pad = Crs::parse("PAD").unwrap();
        let rdg = Crs::parse("RDG").unwrap();
        let bri = Crs::parse("BRI").unwrap();

        // First service: PAD -> RDG
        let mut s1_origin = Call::new(pad, "Paddington".to_string());
        s1_origin.booked_departure = Some(make_time(dep_hour, dep_min));

        let mut s1_dest = Call::new(rdg, "Reading".to_string());
        s1_dest.booked_arrival = Some(make_time(leg1_arr_hour, leg1_arr_min));

        let svc1 = Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{id}A"), pad),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls: vec![s1_origin, s1_dest],
            board_station_idx: CallIndex(0),
        });

        // Second service: RDG -> BRI
        let mut s2_origin = Call::new(rdg, "Reading".to_string());
        s2_origin.booked_departure = Some(make_time(leg2_dep_hour, leg2_dep_min));

        let mut s2_dest = Call::new(bri, "Bristol".to_string());
        s2_dest.booked_arrival = Some(make_time(leg2_arr_hour, leg2_arr_min));

        let svc2 = Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{id}B"), rdg),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls: vec![s2_origin, s2_dest],
            board_station_idx: CallIndex(0),
        });

        let leg1 = Leg::new(svc1, CallIndex(0), CallIndex(1)).unwrap();
        let leg2 = Leg::new(svc2, CallIndex(0), CallIndex(1)).unwrap();

        Journey::new(vec![Segment::Train(leg1), Segment::Train(leg2)]).unwrap()
    }

    /// Strategy for generating a single-leg journey
    fn journey_strategy() -> impl Strategy<Value = Journey> {
        (
            0u32..1000, // id
            0u16..1380, // dep_mins (0:00 - 23:00)
            10u16..120, // duration (10 mins - 2 hours)
        )
            .prop_map(|(id, dep_mins, duration)| {
                let svc = make_service_with_times(id, dep_mins, duration);
                let leg = Leg::new(svc, CallIndex(0), CallIndex(1)).unwrap();
                Journey::new(vec![Segment::Train(leg)]).unwrap()
            })
    }

    /// Strategy for generating journeys with varied change counts.
    /// Bias parameter controls probability of multi-leg journey.
    fn journey_with_changes_strategy(change_bias: f64) -> impl Strategy<Value = Journey> {
        prop::bool::weighted(change_bias).prop_flat_map(|has_change| {
            if has_change {
                (
                    0u32..1000,
                    0u16..1200, // dep_mins
                    15u16..60,  // leg1_duration
                    5u16..30,   // connection_wait
                    15u16..60,  // leg2_duration
                )
                    .prop_map(|(id, dep, d1, wait, d2)| make_two_leg_journey(id, dep, d1, wait, d2))
                    .boxed()
            } else {
                journey_strategy().boxed()
            }
        })
    }

    /// Strategy for generating a list of journeys, fuzzing over distribution bias
    fn journeys_strategy() -> impl Strategy<Value = Vec<Journey>> {
        // Fuzz over the change bias itself
        (0.0f64..1.0).prop_flat_map(|change_bias| {
            prop::collection::vec(journey_with_changes_strategy(change_bias), 0..15)
        })
    }

    // ========== rank_journeys properties ==========

    proptest! {
        #[test]
        fn rank_journeys_is_sorted(journeys in journeys_strategy()) {
            let ranked = rank_journeys(journeys);

            // Reference: check sorted by (arrival, changes, duration)
            for window in ranked.windows(2) {
                let a = &window[0];
                let b = &window[1];

                let a_key = (a.arrival_time(), a.change_count(), a.total_duration());
                let b_key = (b.arrival_time(), b.change_count(), b.total_duration());

                prop_assert!(
                    a_key <= b_key,
                    "Not sorted: {:?} should come before {:?}",
                    a_key,
                    b_key
                );
            }
        }

        #[test]
        fn rank_journeys_preserves_elements(journeys in journeys_strategy()) {
            let original_len = journeys.len();
            let ranked = rank_journeys(journeys);

            prop_assert_eq!(ranked.len(), original_len);
        }
    }

    // ========== remove_dominated properties ==========

    /// Check if journey `a` dominates journey `b`
    fn dominates(a: &Journey, b: &Journey) -> bool {
        a.arrival_time() <= b.arrival_time()
            && a.change_count() <= b.change_count()
            && a.total_duration() <= b.total_duration()
            && (a.arrival_time() < b.arrival_time()
                || a.change_count() < b.change_count()
                || a.total_duration() < b.total_duration())
    }

    proptest! {
        #[test]
        fn remove_dominated_no_internal_domination(journeys in journeys_strategy()) {
            let result = remove_dominated(journeys);

            // No journey in result should dominate another
            for (i, a) in result.iter().enumerate() {
                for (j, b) in result.iter().enumerate() {
                    if i != j {
                        prop_assert!(
                            !dominates(a, b),
                            "Journey {} dominates journey {} in result",
                            i,
                            j
                        );
                    }
                }
            }
        }

        #[test]
        fn remove_dominated_subset(journeys in journeys_strategy()) {
            let original_len = journeys.len();
            let result = remove_dominated(journeys);

            prop_assert!(result.len() <= original_len);
        }
    }

    // Test with instrumentation to verify we hit dominated cases
    #[test]
    fn remove_dominated_distribution() {
        use proptest::test_runner::{Config, TestRunner};
        use std::cell::Cell;

        let mut runner = TestRunner::new(Config::with_cases(500));
        let dominated_removed_count = Cell::new(0u32);
        let total_tests = Cell::new(0u32);

        let _ = runner.run(&journeys_strategy(), |journeys| {
            let original_len = journeys.len();
            let result = remove_dominated(journeys);

            if result.len() < original_len {
                dominated_removed_count.set(dominated_removed_count.get() + 1);
            }
            total_tests.set(total_tests.get() + 1);
            Ok(())
        });

        // We should see some dominated journeys removed
        // (not all inputs will have dominated journeys, but some should)
        assert!(
            dominated_removed_count.get() > 0 || total_tests.get() < 10,
            "Never removed dominated journeys in {} tests",
            total_tests.get()
        );
    }

    // ========== deduplicate properties ==========

    proptest! {
        #[test]
        fn deduplicate_no_duplicate_keys(journeys in journeys_strategy()) {
            let result = deduplicate(journeys);

            // No two journeys should have same (arrival, departure, changes)
            for (i, a) in result.iter().enumerate() {
                for (j, b) in result.iter().enumerate() {
                    if i != j {
                        let a_key = (a.arrival_time(), a.departure_time(), a.change_count());
                        let b_key = (b.arrival_time(), b.departure_time(), b.change_count());
                        prop_assert!(
                            a_key != b_key,
                            "Duplicate key at {} and {}: {:?}",
                            i,
                            j,
                            a_key
                        );
                    }
                }
            }
        }

        #[test]
        fn deduplicate_subset(journeys in journeys_strategy()) {
            let original_len = journeys.len();
            let result = deduplicate(journeys);

            prop_assert!(result.len() <= original_len);
        }
    }

    // Test with instrumentation to verify we hit duplicate cases
    #[test]
    fn deduplicate_distribution() {
        use proptest::test_runner::{Config, TestRunner};
        use std::cell::Cell;

        let mut runner = TestRunner::new(Config::with_cases(500));
        let duplicates_removed_count = Cell::new(0u32);
        let total_tests = Cell::new(0u32);

        // Use a strategy that's more likely to generate duplicates
        let dup_strategy = prop::collection::vec(
            (
                0u32..5, // fewer IDs = more likely duplicates
                0u16..4, // dep slot (each * 60 = hour)
                0u16..2, // duration slot (each * 30 = duration)
            ),
            2..10,
        )
        .prop_map(|params| {
            params
                .into_iter()
                .map(|(id, dep_slot, dur_slot)| {
                    let svc = make_service_with_times(id, dep_slot * 60, dur_slot * 30 + 30);
                    let leg = Leg::new(svc, CallIndex(0), CallIndex(1)).unwrap();
                    Journey::new(vec![Segment::Train(leg)]).unwrap()
                })
                .collect::<Vec<_>>()
        });

        let _ = runner.run(&dup_strategy, |journeys| {
            let original_len = journeys.len();
            let result = deduplicate(journeys);

            if result.len() < original_len {
                duplicates_removed_count.set(duplicates_removed_count.get() + 1);
            }
            total_tests.set(total_tests.get() + 1);
            Ok(())
        });

        // We should see some duplicates removed
        assert!(
            duplicates_removed_count.get() > 0,
            "Never removed duplicates in {} tests (strategy may need tuning)",
            total_tests.get()
        );
    }
}
