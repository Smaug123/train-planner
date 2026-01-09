//! Unit tests for the arrivals-first search algorithm.

use super::*;
use crate::domain::{Call, ServiceRef};
use std::collections::HashMap;
use std::sync::Mutex;

fn date() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
}

fn time(s: &str) -> RailTime {
    RailTime::parse_hhmm(s, date()).unwrap()
}

fn crs(s: &str) -> Crs {
    Crs::parse(s).unwrap()
}

fn make_service(
    id: &str,
    calls_data: &[(&str, &str, &str, &str)], // (crs, name, arr, dep)
) -> Arc<Service> {
    let calls: Vec<Call> = calls_data
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

    let board_crs = calls
        .first()
        .map(|c| c.station)
        .unwrap_or_else(|| crs("XXX"));

    Arc::new(Service {
        service_ref: ServiceRef::new(id.to_string(), board_crs),
        headcode: None,
        operator: "Test".to_string(),
        operator_code: None,
        calls,
        board_station_idx: CallIndex(0),
    })
}

/// Mock service provider for testing.
struct MockProvider {
    departures: HashMap<Crs, Vec<Arc<Service>>>,
    arrivals: HashMap<Crs, Vec<Arc<Service>>>,
    call_count: Mutex<usize>,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            departures: HashMap::new(),
            arrivals: HashMap::new(),
            call_count: Mutex::new(0),
        }
    }

    fn add_departures(&mut self, station: Crs, services: Vec<Arc<Service>>) {
        self.departures.insert(station, services);
    }

    fn add_arrivals(&mut self, station: Crs, services: Vec<Arc<Service>>) {
        self.arrivals.insert(station, services);
    }

    fn api_call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

impl ServiceProvider for MockProvider {
    async fn get_departures(
        &self,
        station: &Crs,
        _after: RailTime,
    ) -> Result<Vec<Arc<Service>>, SearchError> {
        *self.call_count.lock().unwrap() += 1;
        Ok(self.departures.get(station).cloned().unwrap_or_default())
    }

    async fn get_arrivals(
        &self,
        station: &Crs,
        _after: RailTime,
    ) -> Result<Vec<Arc<Service>>, SearchError> {
        *self.call_count.lock().unwrap() += 1;
        Ok(self.arrivals.get(station).cloned().unwrap_or_default())
    }
}

#[tokio::test]
async fn direct_journey_found() {
    // Current train: PAD -> RDG -> SWI -> BRI
    // User at PAD, destination BRI
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", "10:27"),
            ("SWI", "Swindon", "10:50", "10:52"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let provider = MockProvider::new();
    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    assert_eq!(result.journeys.len(), 1);
    assert!(result.journeys[0].is_direct());
    assert_eq!(result.journeys[0].destination(), &crs("BRI"));
}

#[tokio::test]
async fn direct_journey_needs_zero_api_calls_when_max_changes_zero() {
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let provider = MockProvider::new();
    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 0,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    assert_eq!(result.journeys.len(), 1);
    assert_eq!(result.routes_explored, 0); // No API calls needed
}

#[tokio::test]
async fn one_change_journey_found() {
    // Current train: PAD -> RDG
    // Arriving train at BRI via RDG: RDG -> SWI -> BRI
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    // Service arriving at BRI that calls at RDG
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:35"),
            ("SWI", "Swindon", "10:55", "10:57"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 1-change journey: PAD -> RDG, change, RDG -> BRI
    assert!(!result.journeys.is_empty());
    let journey = &result.journeys[0];
    assert_eq!(journey.change_count(), 1);
    assert_eq!(journey.origin(), &crs("PAD"));
    assert_eq!(journey.destination(), &crs("BRI"));

    // API calls: 1 arrivals + 2 departures (PAD and RDG for 2-change exploration)
    assert_eq!(result.routes_explored, 3);
}

#[tokio::test]
async fn one_change_needs_only_arrivals_when_max_changes_is_one() {
    // Same setup as one_change_journey_found but with max_changes=1
    // to verify that 1-change search needs only the arrivals call
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:35"),
            ("SWI", "Swindon", "10:55", "10:57"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 1, // Only 1-change search, no 2-change
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    assert!(!result.journeys.is_empty());
    // With max_changes=1, we only need the arrivals call (no 2-change departures)
    assert_eq!(result.routes_explored, 1);
}

#[tokio::test]
async fn one_change_with_walk() {
    // Current train: PAD -> KGX
    // Walk KGX -> STP
    // Arriving train: STP -> BRI (destination)
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("KGX", "King's Cross", "10:30", ""),
        ],
    );

    // Service arriving at BRI via STP
    let arriving_service = make_service(
        "AR",
        &[
            ("STP", "St Pancras", "", "10:45"),
            ("BRI", "Bristol", "12:00", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    // KGX -> STP is walkable
    let mut walkable = WalkableConnections::new();
    walkable.add(crs("KGX"), crs("STP"), 5);

    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 1-change journey with walk
    assert!(!result.journeys.is_empty());
    let journey = &result.journeys[0];
    assert_eq!(journey.change_count(), 1);
    assert!(journey.walks().count() > 0);
}

#[tokio::test]
async fn respects_min_connection_time() {
    // Current train: PAD -> RDG arriving 10:25
    // Arriving train: RDG departing 10:27 (only 2 min connection)
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:27"), // Only 2 min after arrival
            ("BRI", "Bristol", "11:00", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        min_connection_mins: 5, // 5 min minimum
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should not find journey due to tight connection
    assert!(result.journeys.is_empty());
}

#[tokio::test]
async fn two_change_journey_found() {
    // Current train: PAD -> OXF (not a feeder station)
    // Bridge service: OXF -> RDG
    // Arriving train: RDG -> BRI
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("OXF", "Oxford", "11:00", ""),
        ],
    );

    // Service arriving at BRI via RDG (makes RDG a feeder)
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "12:00"),
            ("BRI", "Bristol", "12:30", ""),
        ],
    );

    // Bridge service from OXF to RDG
    let bridge_service = make_service(
        "BR",
        &[
            ("OXF", "Oxford", "", "11:10"),
            ("RDG", "Reading", "11:45", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("OXF"), vec![bridge_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 2-change journey
    assert!(!result.journeys.is_empty());
    let journey = &result.journeys[0];
    assert_eq!(journey.change_count(), 2);

    // API calls: 1 arrivals + departures from PAD and OXF (both non-feeders)
    // PAD is position 0 (where user boards), OXF is position 1
    assert_eq!(result.routes_explored, 3);
}

#[tokio::test]
async fn api_calls_bounded() {
    // Train with many stops, none are feeders
    let current_train = make_service(
        "CT",
        &[
            ("AAA", "Station A", "", "10:00"),
            ("BBB", "Station B", "10:10", "10:12"),
            ("CCC", "Station C", "10:20", "10:22"),
            ("DDD", "Station D", "10:30", "10:32"),
            ("EEE", "Station E", "10:40", ""),
        ],
    );

    // Only service arriving at destination, from ZZZ (not on current train)
    let arriving_service = make_service(
        "AR",
        &[
            ("ZZZ", "Station Z", "", "12:00"),
            ("DST", "Destination", "12:30", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("DST"), vec![arriving_service]);
    // No departures set up -> will return empty for each station queried

    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("DST"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // API calls should be bounded: 1 arrivals + at most N departures
    // where N is number of non-feeder stations on current train (5 stops)
    assert!(
        result.routes_explored <= 6,
        "Expected <= 6 API calls, got {}",
        result.routes_explored
    );
}

#[tokio::test]
async fn invalid_position_rejected() {
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    let provider = MockProvider::new();
    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    // Position 5 is out of bounds (train has 2 calls)
    let request = SearchRequest::new(current_train, CallIndex(5), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await;

    assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
}

#[tokio::test]
async fn multiple_arriving_services_all_considered() {
    // Current train: PAD -> RDG
    // Two different arriving services at BRI via RDG
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    let arriving1 = make_service(
        "AR1",
        &[
            ("RDG", "Reading", "", "10:35"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let arriving2 = make_service(
        "AR2",
        &[
            ("RDG", "Reading", "", "10:45"),
            ("BRI", "Bristol", "11:30", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving1, arriving2]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_results: 10,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find both options (before deduplication/domination filtering)
    // At minimum should have the earlier arriving one
    assert!(!result.journeys.is_empty());
    assert_eq!(result.journeys[0].arrival_time(), time("11:20"));
}

#[tokio::test]
async fn feeder_stations_also_explored_for_two_change() {
    // Current train: PAD -> RDG
    // RDG is a feeder station (has service to BRI)
    // We still query departures from RDG for 2-change exploration
    // (because 1-change via RDG might be rejected due to timing)
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("RDG", "Reading", "10:25", ""),
        ],
    );

    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:35"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // API calls: 1 arrivals + 2 departures (PAD and RDG)
    // Feeder stations are now explored for 2-change in case 1-change is rejected
    assert_eq!(result.routes_explored, 3);
    // And should still find the 1-change journey
    assert!(!result.journeys.is_empty());
}

#[tokio::test]
async fn all_stops_explored_for_two_change_even_when_feeders() {
    // Even when all stops on the train are feeders, we still explore them
    // for 2-change journeys (in case 1-change is rejected due to timing)
    let current_train = make_service(
        "CT",
        &[
            ("RDG", "Reading", "", "10:00"),
            ("SWI", "Swindon", "10:30", ""),
        ],
    );

    // Service arriving at BRI via RDG and SWI (both become feeders)
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:15"),
            ("SWI", "Swindon", "10:35", "10:37"),
            ("BRI", "Bristol", "11:00", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig::default();

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // API calls: 1 arrivals + 2 departures (RDG and SWI)
    // Both are feeders but we still explore them for 2-change
    assert_eq!(result.routes_explored, 3);
    // Should find 1-change journeys (RDG->BRI or SWI->BRI connections)
    assert!(!result.journeys.is_empty());
}

#[tokio::test]
async fn three_change_journey_via_bfs_fallback() {
    // Current train: PAD -> AAA (not a feeder)
    // First bridge: AAA -> BBB (not a feeder)
    // Second bridge: BBB -> RDG (RDG is a feeder)
    // Arriving train: RDG -> BRI
    // This requires 3 changes: PAD, AAA, BBB, RDG
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("AAA", "Station A", "10:30", ""),
        ],
    );

    // Service arriving at BRI via RDG (makes RDG a feeder)
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "12:30"),
            ("BRI", "Bristol", "13:00", ""),
        ],
    );

    // First bridge: AAA -> BBB
    let bridge1 = make_service(
        "BR1",
        &[
            ("AAA", "Station A", "", "10:40"),
            ("BBB", "Station B", "11:10", ""),
        ],
    );

    // Second bridge: BBB -> RDG
    let bridge2 = make_service(
        "BR2",
        &[
            ("BBB", "Station B", "", "11:20"),
            ("RDG", "Reading", "12:00", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("PAD"), vec![]); // No useful services from PAD
    provider.add_departures(crs("AAA"), vec![bridge1]);
    provider.add_departures(crs("BBB"), vec![bridge2]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 3, // Allow 3 changes
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 3-change journey via BFS fallback
    assert!(!result.journeys.is_empty(), "Should find 3-change journey");
    let journey = &result.journeys[0];
    assert_eq!(journey.change_count(), 3, "Journey should have 3 changes");
    assert_eq!(journey.origin(), &crs("PAD"));
    assert_eq!(journey.destination(), &crs("BRI"));
}

#[tokio::test]
async fn bfs_fallback_uses_arrivals_index_shortcut() {
    // Verify that BFS terminates at feeder stations using ArrivalsIndex
    // Without the shortcut, BFS would continue exploring from RDG
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("AAA", "Station A", "10:30", ""),
        ],
    );

    // RDG is a feeder via this arriving service
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "12:30"),
            ("BRI", "Bristol", "13:00", ""),
        ],
    );

    // Bridge from AAA reaches RDG (a feeder)
    let bridge = make_service(
        "BR",
        &[
            ("AAA", "Station A", "", "10:40"),
            ("RDG", "Reading", "11:30", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("PAD"), vec![]);
    provider.add_departures(crs("AAA"), vec![bridge]);
    // NOT adding departures from RDG - if BFS doesn't use the shortcut,
    // it would try to fetch them

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 3,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 2-change journey (PAD->AAA, AAA->RDG, RDG->BRI)
    // The BFS should use ArrivalsIndex shortcut at RDG
    assert!(!result.journeys.is_empty());

    // API calls: 1 arrivals + 2 departures (PAD, AAA)
    // NOT 3 (would be 3 if BFS tried to fetch from RDG)
    assert_eq!(
        result.routes_explored, 3,
        "BFS should not fetch departures from feeder station RDG"
    );
}

#[tokio::test]
async fn bfs_fallback_reuses_departures_cache() {
    // Verify that departures fetched in 2-change phase are reused by BFS
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("AAA", "Station A", "10:30", ""),
        ],
    );

    // No feeder stations reachable in 2 changes
    let arriving_service = make_service(
        "AR",
        &[
            ("ZZZ", "Station Z", "", "12:30"),
            ("BRI", "Bristol", "13:00", ""),
        ],
    );

    // Bridge from AAA to BBB (BBB not a feeder)
    let bridge = make_service(
        "BR",
        &[
            ("AAA", "Station A", "", "10:40"),
            ("BBB", "Station B", "11:10", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("PAD"), vec![]);
    provider.add_departures(crs("AAA"), vec![bridge.clone()]);
    provider.add_departures(crs("BBB"), vec![]); // No onward connections

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 3,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let _result = planner.search(&request).await.unwrap();

    // 2-change phase queries: PAD, AAA (2 calls)
    // BFS fallback should reuse PAD and AAA from cache
    // BFS only needs to fetch BBB (1 call)
    // Total: 1 arrivals + 2 departures (PAD, AAA) + 1 departures (BBB) = 4
    // But PAD and AAA are cached, so BFS doesn't re-fetch them
    // The actual count depends on which stations BFS explores
    assert!(
        provider.api_call_count() <= 4,
        "Expected <= 4 API calls due to cache reuse, got {}",
        provider.api_call_count()
    );
}

#[tokio::test]
async fn bfs_finds_direct_destination_not_via_feeder() {
    // BFS can find journeys that go directly to destination
    // without going through a feeder station
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("AAA", "Station A", "10:30", ""),
        ],
    );

    // Arriving service via feeder RDG
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "12:30"),
            ("BRI", "Bristol", "13:00", ""),
        ],
    );

    // Alternative: bridge from AAA goes directly to BRI
    let direct_bridge = make_service(
        "DB",
        &[
            ("AAA", "Station A", "", "10:40"),
            ("BRI", "Bristol", "11:30", ""), // Faster than via RDG
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("PAD"), vec![]);
    provider.add_departures(crs("AAA"), vec![direct_bridge]);

    let walkable = WalkableConnections::new();
    let config = SearchConfig {
        max_changes: 3,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find the direct route (1-change via AAA->BRI)
    assert!(!result.journeys.is_empty());
    // The fastest should be the direct one arriving at 11:30
    assert_eq!(result.journeys[0].arrival_time(), time("11:30"));
}

#[tokio::test]
async fn bfs_respects_max_changes_limit() {
    // BFS should not exceed max_changes
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "10:00"),
            ("AAA", "Station A", "10:30", ""),
        ],
    );

    // Feeder at CCC (requires 3 changes to reach)
    let arriving_service = make_service(
        "AR",
        &[
            ("CCC", "Station C", "", "12:30"),
            ("BRI", "Bristol", "13:00", ""),
        ],
    );

    // AAA -> BBB
    let bridge1 = make_service(
        "BR1",
        &[
            ("AAA", "Station A", "", "10:40"),
            ("BBB", "Station B", "11:00", ""),
        ],
    );

    // BBB -> CCC
    let bridge2 = make_service(
        "BR2",
        &[
            ("BBB", "Station B", "", "11:10"),
            ("CCC", "Station C", "11:30", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("PAD"), vec![]);
    provider.add_departures(crs("AAA"), vec![bridge1]);
    provider.add_departures(crs("BBB"), vec![bridge2]);

    let walkable = WalkableConnections::new();

    // With max_changes=2, should NOT find the 3-change journey
    let config = SearchConfig {
        max_changes: 2,
        ..SearchConfig::default()
    };

    let request = SearchRequest::new(current_train.clone(), CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    assert!(
        result.journeys.is_empty(),
        "Should not find journey with max_changes=2"
    );

    // With max_changes=3, SHOULD find it
    let config = SearchConfig {
        max_changes: 3,
        ..SearchConfig::default()
    };

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    assert!(
        !result.journeys.is_empty(),
        "Should find journey with max_changes=3"
    );
    assert_eq!(result.journeys[0].change_count(), 3);
}

/// Regression test: stations_to_query dedup should keep the entry with
/// earliest arrival at the query station, not the earliest call index.
///
/// Scenario: A later stop with a much shorter walk can arrive earlier
/// at the query station and catch a bridge service that would be missed
/// if we only tried the earlier stop.
#[tokio::test]
async fn two_change_dedup_prefers_earliest_arrival_at_query_station() {
    // Current train: PAD -> STA (10:00) -> STB (10:10)
    // STA has 14-min walk to QRY, STB has 1-min walk to QRY
    //
    // Path via STA: 10:00 + 14min walk = arrive QRY 10:14
    //               available 10:19 (with 5min min_connection) -> MISSES bridge at 10:17
    // Path via STB: 10:10 + 1min walk = arrive QRY 10:11
    //               available 10:16 -> CATCHES bridge at 10:17
    let current_train = make_service(
        "CT",
        &[
            ("PAD", "Paddington", "", "09:30"),
            ("STA", "Station A", "10:00", "10:02"),
            ("STB", "Station B", "10:10", ""),
        ],
    );

    // Bridge service from QRY to RDG (feeder station)
    let bridge_service = make_service(
        "BR",
        &[
            ("QRY", "Query Station", "", "10:17"),
            ("RDG", "Reading", "10:40", ""),
        ],
    );

    // Arriving service from RDG to destination BRI
    let arriving_service = make_service(
        "AR",
        &[
            ("RDG", "Reading", "", "10:50"),
            ("BRI", "Bristol", "11:20", ""),
        ],
    );

    let mut provider = MockProvider::new();
    provider.add_arrivals(crs("BRI"), vec![arriving_service]);
    provider.add_departures(crs("QRY"), vec![bridge_service]);

    // Set up walkable connections: both STA and STB can walk to QRY
    // but with very different walk times
    let mut walkable = WalkableConnections::new();
    walkable.add(crs("STA"), crs("QRY"), 14); // 14 min walk
    walkable.add(crs("STB"), crs("QRY"), 1); // 1 min walk

    let config = SearchConfig::default(); // 5 min min_connection

    let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

    let planner = Planner::new(&provider, &walkable, &config);
    let result = planner.search(&request).await.unwrap();

    // Should find 2-change journey: PAD -> STB, walk to QRY, QRY -> RDG, RDG -> BRI
    // If the bug exists (dedup by call index), it would try path via STA,
    // miss the bridge, and find no journey.
    assert!(
        !result.journeys.is_empty(),
        "Should find journey via STB (shorter walk, earlier arrival at QRY)"
    );

    // Verify it's a 2-change journey through QRY
    let journey = &result.journeys[0];
    assert_eq!(
        journey.change_count(),
        2,
        "Expected 2-change journey through QRY"
    );

    // Verify the walk is from STB, not STA
    let walk = journey.walks().next().expect("Should have a walk segment");
    assert_eq!(
        walk.from,
        crs("STB"),
        "Walk should be from STB (shorter walk time)"
    );
    assert_eq!(walk.to, crs("QRY"));
}
