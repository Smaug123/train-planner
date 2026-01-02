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
