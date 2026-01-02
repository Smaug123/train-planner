//! BFS journey search algorithm.
//!
//! Finds possible routes from a position on a train to a destination,
//! exploring connections via train changes and walking.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use crate::domain::{CallIndex, Crs, Journey, Leg, RailTime, Segment, Service, Walk};
use crate::walkable::WalkableConnections;

use super::config::SearchConfig;
use super::rank::{deduplicate, rank_journeys, remove_dominated};

/// Error from journey search.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SearchError {
    /// Failed to fetch departures
    #[error("failed to fetch departures from {station}: {message}")]
    FetchError { station: Crs, message: String },

    /// Invalid search request
    #[error("invalid search request: {0}")]
    InvalidRequest(String),

    /// Search timed out
    #[error("search timed out")]
    Timeout,
}

/// Request for journey search.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// The train service the user is currently on.
    pub current_service: Arc<Service>,

    /// The user's current position on the train (call index).
    pub current_position: CallIndex,

    /// The destination station.
    pub destination: Crs,
}

impl SearchRequest {
    /// Create a new search request.
    pub fn new(
        current_service: Arc<Service>,
        current_position: CallIndex,
        destination: Crs,
    ) -> Self {
        Self {
            current_service,
            current_position,
            destination,
        }
    }

    /// Validate the search request.
    pub fn validate(&self) -> Result<(), SearchError> {
        // Check position is valid
        if self.current_position.0 >= self.current_service.calls.len() {
            return Err(SearchError::InvalidRequest(
                "current position is out of bounds".to_string(),
            ));
        }

        // Check there are subsequent stops
        if self.current_position.0 >= self.current_service.calls.len() - 1 {
            return Err(SearchError::InvalidRequest(
                "no subsequent stops on current train".to_string(),
            ));
        }

        Ok(())
    }
}

/// Result of journey search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Found journeys, ranked best-first.
    pub journeys: Vec<Journey>,

    /// Number of routes explored during search.
    pub routes_explored: usize,
}

impl SearchResult {
    /// Create an empty result.
    pub fn empty() -> Self {
        Self {
            journeys: Vec::new(),
            routes_explored: 0,
        }
    }
}

/// Trait for providing departure services.
///
/// This abstraction allows the planner to be tested with mock data.
pub trait ServiceProvider {
    /// Get departures from a station after a given time.
    ///
    /// Returns services that depart from `station` after `after` within
    /// the time window.
    fn get_departures(
        &self,
        station: &Crs,
        after: RailTime,
    ) -> Result<Vec<Arc<Service>>, SearchError>;
}

/// BFS state during search.
#[derive(Debug, Clone)]
struct SearchState {
    /// Current station.
    station: Crs,

    /// Current time (arrival time at this station).
    time: RailTime,

    /// Journey segments built so far.
    segments: Vec<Segment>,

    /// Number of train changes made.
    changes: usize,

    /// Services already used (to avoid cycles).
    used_services: HashSet<String>,
}

impl SearchState {
    /// Create initial states from alighting at subsequent stops on current train.
    fn initial_states(request: &SearchRequest) -> Vec<Self> {
        let service = &request.current_service;
        let mut states = Vec::new();

        // Create a state for alighting at each subsequent stop
        for idx in (request.current_position.0 + 1)..service.calls.len() {
            let call = &service.calls[idx];

            // Need arrival time at this stop
            let arrival_time = match call.expected_arrival() {
                Some(t) => t,
                None => continue, // Skip stops without arrival times
            };

            // Create the leg from current position to this stop
            let leg = match Leg::new(service.clone(), request.current_position, CallIndex(idx)) {
                Ok(leg) => leg,
                Err(_) => continue, // Skip if leg construction fails
            };

            let mut used = HashSet::new();
            used.insert(service.service_ref.darwin_id.clone());

            states.push(SearchState {
                station: call.station,
                time: arrival_time,
                segments: vec![Segment::Train(leg)],
                changes: 0,
                used_services: used,
            });
        }

        states
    }

    /// Check if we've reached the destination.
    fn at_destination(&self, destination: &Crs) -> bool {
        &self.station == destination
    }

    /// Build a journey from the current state.
    fn to_journey(&self) -> Option<Journey> {
        Journey::new(self.segments.clone()).ok()
    }
}

/// Journey planner using BFS.
pub struct Planner<'a, P: ServiceProvider> {
    provider: &'a P,
    walkable: &'a WalkableConnections,
    config: &'a SearchConfig,
}

impl<'a, P: ServiceProvider> Planner<'a, P> {
    /// Create a new planner.
    pub fn new(
        provider: &'a P,
        walkable: &'a WalkableConnections,
        config: &'a SearchConfig,
    ) -> Self {
        Self {
            provider,
            walkable,
            config,
        }
    }

    /// Search for journeys from current position to destination.
    pub fn search(&self, request: &SearchRequest) -> Result<SearchResult, SearchError> {
        request.validate()?;

        // Check if destination is directly reachable on current train
        let mut journeys = Vec::new();
        let mut routes_explored = 0;

        // Check direct service to destination
        if let Some(journey) = self.check_direct(request) {
            journeys.push(journey);
        }

        // Initialize BFS with states from alighting at each subsequent stop
        let initial_states = SearchState::initial_states(request);
        let mut queue: VecDeque<SearchState> = initial_states.into();

        // Track visited (station, time bucket) to avoid redundant exploration
        let mut visited: HashSet<(Crs, i64)> = HashSet::new();

        while let Some(state) = queue.pop_front() {
            routes_explored += 1;

            // Check if at destination
            if state.at_destination(&request.destination) {
                if let Some(journey) = state.to_journey() {
                    journeys.push(journey);
                }
                continue;
            }

            // Pruning: check max changes
            if state.changes >= self.config.max_changes {
                continue;
            }

            // Pruning: check journey time
            let journey_so_far = state.time.signed_duration_since(
                state
                    .segments
                    .first()
                    .map(|s| match s {
                        Segment::Train(leg) => leg.departure_time(),
                        Segment::Walk(_) => state.time, // First segment is always Train
                    })
                    .unwrap_or(state.time),
            );
            if journey_so_far > self.config.max_journey() {
                continue;
            }

            // Deduplicate by (station, time bucket)
            let time_bucket = state.time.to_datetime().and_utc().timestamp() / 300; // 5-min buckets
            if !visited.insert((state.station, time_bucket)) {
                continue;
            }

            // Limit total exploration
            if routes_explored > 10000 {
                break;
            }

            // Get departures from this station
            let min_departure = state.time + self.config.min_connection();
            let departures = self
                .provider
                .get_departures(&state.station, min_departure)?;

            // Explore each departure
            for service in departures {
                // Skip if we've already used this service
                if state.used_services.contains(&service.service_ref.darwin_id) {
                    continue;
                }

                // Find where we can board this service
                let board_idx = match service.find_call(&state.station, CallIndex(0)) {
                    Some((idx, call)) => {
                        // Check departure time is after our arrival + min connection
                        if let Some(dep) = call.expected_departure() {
                            if dep >= min_departure {
                                idx
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }
                    None => continue,
                };

                // Explore alighting at each subsequent stop
                self.explore_service(
                    &state,
                    &service,
                    board_idx,
                    &request.destination,
                    &mut queue,
                );
            }

            // Explore walking connections
            self.explore_walks(&state, &request.destination, &mut queue);
        }

        // Rank and filter results
        let journeys = remove_dominated(journeys);
        let journeys = deduplicate(journeys);
        let mut journeys = rank_journeys(journeys);
        journeys.truncate(self.config.max_results);

        Ok(SearchResult {
            journeys,
            routes_explored,
        })
    }

    /// Check if destination is directly reachable on current train.
    fn check_direct(&self, request: &SearchRequest) -> Option<Journey> {
        let service = &request.current_service;

        // Look for destination in subsequent calls
        let dest_idx = service.find_call(&request.destination, request.current_position.next())?;

        // Create the leg
        let leg = Leg::new(service.clone(), request.current_position, dest_idx.0).ok()?;

        Journey::new(vec![Segment::Train(leg)]).ok()
    }

    /// Explore alighting options on a service.
    fn explore_service(
        &self,
        state: &SearchState,
        service: &Arc<Service>,
        board_idx: CallIndex,
        destination: &Crs,
        queue: &mut VecDeque<SearchState>,
    ) {
        // Check if this service goes to the destination
        if let Some((dest_idx, _)) = service.find_call(destination, board_idx.next()) {
            // Direct to destination - add final journey
            if let Ok(leg) = Leg::new(service.clone(), board_idx, dest_idx) {
                let mut segments = state.segments.clone();
                segments.push(Segment::Train(leg.clone()));

                let mut used = state.used_services.clone();
                used.insert(service.service_ref.darwin_id.clone());

                queue.push_back(SearchState {
                    station: *destination,
                    time: leg.arrival_time(),
                    segments,
                    changes: state.changes + 1,
                    used_services: used,
                });
            }
        }

        // Also explore intermediate stops (for further connections)
        for idx in (board_idx.0 + 1)..service.calls.len() {
            let call = &service.calls[idx];

            // Skip destination (already handled above)
            if &call.station == destination {
                continue;
            }

            let arrival_time = match call.expected_arrival() {
                Some(t) => t,
                None => continue,
            };

            // Check journey time limit
            let first_dep = state
                .segments
                .first()
                .map(|s| match s {
                    Segment::Train(leg) => leg.departure_time(),
                    Segment::Walk(_) => state.time,
                })
                .unwrap_or(state.time);

            if arrival_time.signed_duration_since(first_dep) > self.config.max_journey() {
                continue;
            }

            if let Ok(leg) = Leg::new(service.clone(), board_idx, CallIndex(idx)) {
                let mut segments = state.segments.clone();
                segments.push(Segment::Train(leg));

                let mut used = state.used_services.clone();
                used.insert(service.service_ref.darwin_id.clone());

                queue.push_back(SearchState {
                    station: call.station,
                    time: arrival_time,
                    segments,
                    changes: state.changes + 1,
                    used_services: used,
                });
            }
        }
    }

    /// Explore walking to nearby stations.
    fn explore_walks(
        &self,
        state: &SearchState,
        destination: &Crs,
        queue: &mut VecDeque<SearchState>,
    ) {
        for (walk_to, walk_duration) in self.walkable.walkable_from(&state.station) {
            // Check walk time limit
            if walk_duration > self.config.max_walk() {
                continue;
            }

            let arrival_time = state.time + walk_duration;

            // Create walk segment
            let walk = Walk {
                from: state.station,
                to: walk_to,
                duration: walk_duration,
            };

            let mut segments = state.segments.clone();
            segments.push(Segment::Walk(walk));

            // If walk destination is our destination, we're done
            if walk_to == *destination {
                queue.push_back(SearchState {
                    station: walk_to,
                    time: arrival_time,
                    segments,
                    changes: state.changes, // Walking doesn't count as a change
                    used_services: state.used_services.clone(),
                });
            } else {
                // Otherwise, explore connections from the walked-to station
                queue.push_back(SearchState {
                    station: walk_to,
                    time: arrival_time,
                    segments,
                    changes: state.changes,
                    used_services: state.used_services.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Call, ServiceRef};

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
        board_crs: &str,
        calls_data: &[(&str, &str, &str, &str)],
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

        Arc::new(Service {
            service_ref: ServiceRef::new(id.to_string(), crs(board_crs)),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        })
    }

    /// Mock service provider for testing.
    struct MockProvider {
        services: Vec<Arc<Service>>,
    }

    impl MockProvider {
        fn new(services: Vec<Arc<Service>>) -> Self {
            Self { services }
        }
    }

    impl ServiceProvider for MockProvider {
        fn get_departures(
            &self,
            station: &Crs,
            after: RailTime,
        ) -> Result<Vec<Arc<Service>>, SearchError> {
            Ok(self
                .services
                .iter()
                .filter(|s| {
                    s.calls.iter().any(|c| {
                        &c.station == station && c.expected_departure().is_some_and(|t| t >= after)
                    })
                })
                .cloned()
                .collect())
        }
    }

    #[test]
    fn direct_journey() {
        // User is on PAD->RDG->SWI->BRI train, wants to go to BRI
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", "10:27"),
                ("SWI", "Swindon", "10:52", "10:54"),
                ("BRI", "Bristol", "11:30", ""),
            ],
        );

        let provider = MockProvider::new(vec![]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(0), crs("BRI"));
        let result = planner.search(&request).unwrap();

        assert_eq!(result.journeys.len(), 1);
        assert_eq!(result.journeys[0].change_count(), 0);
        assert_eq!(result.journeys[0].arrival_time(), time("11:30"));
    }

    #[test]
    fn direct_journey_from_middle() {
        // User is at RDG (position 1), wants to go to BRI
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", "10:27"),
                ("SWI", "Swindon", "10:52", "10:54"),
                ("BRI", "Bristol", "11:30", ""),
            ],
        );

        let provider = MockProvider::new(vec![]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(1), crs("BRI"));
        let result = planner.search(&request).unwrap();

        assert_eq!(result.journeys.len(), 1);
        assert_eq!(result.journeys[0].departure_time(), time("10:27"));
        assert_eq!(result.journeys[0].arrival_time(), time("11:30"));
    }

    #[test]
    fn one_change_journey() {
        // User is on PAD->RDG train, wants to go to OXF
        // Connection at RDG to RDG->OXF train
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", ""),
            ],
        );

        let connection = make_service(
            "S2",
            "RDG",
            &[
                ("RDG", "Reading", "", "10:35"),
                ("OXF", "Oxford", "11:00", ""),
            ],
        );

        let provider = MockProvider::new(vec![connection]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(0), crs("OXF"));
        let result = planner.search(&request).unwrap();

        assert!(!result.journeys.is_empty());
        let journey = &result.journeys[0];
        assert_eq!(journey.change_count(), 1);
        assert_eq!(journey.arrival_time(), time("11:00"));
    }

    #[test]
    fn journey_with_walk() {
        // User is on train to EUS, wants to go to station served from KGX
        // Walk EUS -> KGX, then train KGX -> destination
        let current = make_service(
            "S1",
            "XXX",
            &[
                ("XXX", "Somewhere", "", "10:00"),
                ("EUS", "Euston", "10:30", ""),
            ],
        );

        let from_kgx = make_service(
            "S2",
            "KGX",
            &[
                ("KGX", "Kings Cross", "", "10:45"),
                ("YRK", "York", "12:30", ""),
            ],
        );

        let provider = MockProvider::new(vec![from_kgx]);

        let mut walkable = WalkableConnections::new();
        walkable.add(crs("EUS"), crs("KGX"), 5);

        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(0), crs("YRK"));
        let result = planner.search(&request).unwrap();

        assert!(!result.journeys.is_empty());
        let journey = &result.journeys[0];
        // One train + walk + one train
        assert!(journey.segments().len() >= 2);
    }

    #[test]
    fn invalid_request_position_out_of_bounds() {
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", ""),
            ],
        );

        let provider = MockProvider::new(vec![]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(10), crs("BRI"));
        let result = planner.search(&request);

        assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    }

    #[test]
    fn invalid_request_no_subsequent_stops() {
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", ""),
            ],
        );

        let provider = MockProvider::new(vec![]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        // Position at last stop - no subsequent stops
        let request = SearchRequest::new(current, CallIndex(1), crs("BRI"));
        let result = planner.search(&request);

        assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    }

    #[test]
    fn respects_max_changes() {
        // Setup where destination requires 4 changes but max is 3
        let current = make_service(
            "S1",
            "AAA",
            &[
                ("AAA", "Station A", "", "10:00"),
                ("BBB", "Station B", "10:15", ""),
            ],
        );

        // Chain of connections
        let s2 = make_service(
            "S2",
            "BBB",
            &[
                ("BBB", "Station B", "", "10:25"),
                ("CCC", "Station C", "10:40", ""),
            ],
        );
        let s3 = make_service(
            "S3",
            "CCC",
            &[
                ("CCC", "Station C", "", "10:50"),
                ("DDD", "Station D", "11:05", ""),
            ],
        );
        let s4 = make_service(
            "S4",
            "DDD",
            &[
                ("DDD", "Station D", "", "11:15"),
                ("EEE", "Station E", "11:30", ""),
            ],
        );
        let s5 = make_service(
            "S5",
            "EEE",
            &[
                ("EEE", "Station E", "", "11:40"),
                ("FFF", "Station F", "11:55", ""),
            ],
        );

        let provider = MockProvider::new(vec![s2, s3, s4, s5]);
        let walkable = WalkableConnections::new();
        let mut config = SearchConfig::default();
        config.max_changes = 2; // Only allow 2 changes

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(0), crs("FFF"));
        let result = planner.search(&request).unwrap();

        // Should not find route to FFF (would need 4 changes)
        // But might find routes to intermediate stations
        for journey in &result.journeys {
            assert!(journey.change_count() <= 2);
        }
    }

    #[test]
    fn empty_result_when_no_route() {
        let current = make_service(
            "S1",
            "PAD",
            &[
                ("PAD", "Paddington", "", "10:00"),
                ("RDG", "Reading", "10:25", ""),
            ],
        );

        let provider = MockProvider::new(vec![]); // No connections
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        // Destination not on current train and no connections
        let request = SearchRequest::new(current, CallIndex(0), crs("XXX"));
        let result = planner.search(&request).unwrap();

        assert!(result.journeys.is_empty());
    }
}
