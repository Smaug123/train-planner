//! BFS journey search algorithm.
//!
//! Finds possible routes from a position on a train to a destination,
//! exploring connections via train changes and walking.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tracing::{debug, info, instrument, trace, warn};

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
pub trait ServiceProvider: Send + Sync {
    /// Get departures from a station after a given time.
    ///
    /// Returns services that depart from `station` after `after` within
    /// the time window.
    fn get_departures(
        &self,
        station: &Crs,
        after: RailTime,
    ) -> impl std::future::Future<Output = Result<Vec<Arc<Service>>, SearchError>> + Send;
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

            // Need arrival time at this stop. Darwin only provides departure times for
            // intermediate calling points (arrival is typically 1-2 mins earlier but
            // not exposed), so fall back to departure time if arrival isn't available.
            let arrival_time = match call
                .expected_arrival()
                .or_else(|| call.expected_departure())
            {
                Some(t) => t,
                None => continue, // Skip stops without any time
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
    #[instrument(skip(self, request), fields(
        destination = %request.destination.as_str(),
        current_position = request.current_position.0,
        service_id = %request.current_service.service_ref.darwin_id
    ))]
    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResult, SearchError> {
        info!(
            terminus = %request.current_service.calls.last().map(|c| c.station.as_str()).unwrap_or("?"),
            "Starting journey search"
        );
        request.validate()?;

        // Check if destination is directly reachable on current train
        let mut journeys = Vec::new();
        let mut routes_explored = 0;

        // Track best arrival time for pruning: skip states that can't beat our best
        let mut best_arrival: Option<RailTime> = None;

        // Check direct service to destination
        if let Some(journey) = self.check_direct(request) {
            debug!("Direct route found on current train");
            best_arrival = Some(journey.arrival_time());
            journeys.push(journey);
        } else {
            debug!("No direct route on current train");
        }

        // Initialize BFS with states from alighting at each subsequent stop
        let initial_states = SearchState::initial_states(request);
        debug!(initial_states = initial_states.len(), "Starting BFS");
        for state in &initial_states {
            trace!(
                station = %state.station.as_str(),
                time = %state.time,
                "Initial alighting point"
            );
        }
        let mut queue: VecDeque<SearchState> = initial_states.into();

        // Track visited (station, time bucket) to avoid redundant exploration
        let mut visited: HashSet<(Crs, i64)> = HashSet::new();

        while let Some(state) = queue.pop_front() {
            routes_explored += 1;

            // Pruning: skip if we can't possibly beat the best arrival time
            // (we're already at or past the best known arrival)
            if best_arrival.is_some_and(|best| state.time >= best) {
                trace!(
                    station = %state.station.as_str(),
                    time = %state.time,
                    "Pruned: can't beat best arrival time"
                );
                continue;
            }

            // Check if at destination
            if state.at_destination(&request.destination) {
                if let Some(journey) = state.to_journey() {
                    let arrival = journey.arrival_time();
                    debug!(
                        changes = journey.change_count(),
                        arrival = %arrival,
                        "Found journey to destination"
                    );
                    // Update best arrival if this is better
                    if best_arrival.is_none_or(|best| arrival < best) {
                        best_arrival = Some(arrival);
                    }
                    journeys.push(journey);
                }
                continue;
            }

            // Pruning: check max changes
            if state.changes >= self.config.max_changes {
                trace!(
                    station = %state.station.as_str(),
                    changes = state.changes,
                    "Pruned: max changes exceeded"
                );
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
                trace!(
                    station = %state.station.as_str(),
                    duration = ?journey_so_far,
                    "Pruned: max journey time exceeded"
                );
                continue;
            }

            // Deduplicate by (station, time bucket)
            let time_bucket = state.time.to_datetime().and_utc().timestamp() / 300; // 5-min buckets
            if !visited.insert((state.station, time_bucket)) {
                trace!(
                    station = %state.station.as_str(),
                    "Pruned: already visited this station/time"
                );
                continue;
            }

            // Limit total exploration
            if routes_explored > 10000 {
                warn!("Search terminated: exceeded 10000 routes explored");
                break;
            }

            // Get departures from this station
            let min_departure = state.time + self.config.min_connection();
            debug!(
                station = %state.station.as_str(),
                time = %state.time,
                min_departure = %min_departure,
                changes = state.changes,
                "Exploring station"
            );

            let departures = self
                .provider
                .get_departures(&state.station, min_departure)
                .await?;

            debug!(
                station = %state.station.as_str(),
                departures = departures.len(),
                "Got departures"
            );

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

                trace!(
                    service_id = %service.service_ref.darwin_id,
                    terminus = %service.calls.last().map(|c| c.station.as_str()).unwrap_or("?"),
                    "Exploring service"
                );

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
        debug!(
            routes_explored,
            journeys_found = journeys.len(),
            "BFS complete"
        );
        let journeys = remove_dominated(journeys);
        let journeys = deduplicate(journeys);
        let mut journeys = rank_journeys(journeys);
        journeys.truncate(self.config.max_results);

        info!(
            routes_explored,
            journeys = journeys.len(),
            "Search complete"
        );

        if journeys.is_empty() {
            warn!("No routes found to destination");
        }

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

            // Fall back to departure time if arrival isn't available (see initial_states)
            let arrival_time = match call
                .expected_arrival()
                .or_else(|| call.expected_departure())
            {
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
        async fn get_departures(
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

    #[tokio::test]
    async fn direct_journey() {
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
        let result = planner.search(&request).await.unwrap();

        assert_eq!(result.journeys.len(), 1);
        assert_eq!(result.journeys[0].change_count(), 0);
        assert_eq!(result.journeys[0].arrival_time(), time("11:30"));
    }

    #[tokio::test]
    async fn direct_journey_from_middle() {
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
        let result = planner.search(&request).await.unwrap();

        assert_eq!(result.journeys.len(), 1);
        assert_eq!(result.journeys[0].departure_time(), time("10:27"));
        assert_eq!(result.journeys[0].arrival_time(), time("11:30"));
    }

    #[tokio::test]
    async fn one_change_journey() {
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
        let result = planner.search(&request).await.unwrap();

        assert!(!result.journeys.is_empty());
        let journey = &result.journeys[0];
        assert_eq!(journey.change_count(), 1);
        assert_eq!(journey.arrival_time(), time("11:00"));
    }

    #[tokio::test]
    async fn journey_with_walk() {
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
        let result = planner.search(&request).await.unwrap();

        assert!(!result.journeys.is_empty());
        let journey = &result.journeys[0];
        // One train + walk + one train
        assert!(journey.segments().len() >= 2);
    }

    #[tokio::test]
    async fn invalid_request_position_out_of_bounds() {
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
        let result = planner.search(&request).await;

        assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    }

    #[tokio::test]
    async fn invalid_request_no_subsequent_stops() {
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
        let result = planner.search(&request).await;

        assert!(matches!(result, Err(SearchError::InvalidRequest(_))));
    }

    #[tokio::test]
    async fn respects_max_changes() {
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
        let config = SearchConfig {
            max_changes: 2, // Only allow 2 changes
            ..Default::default()
        };

        let planner = Planner::new(&provider, &walkable, &config);

        let request = SearchRequest::new(current, CallIndex(0), crs("FFF"));
        let result = planner.search(&request).await.unwrap();

        // Should not find route to FFF (would need 4 changes)
        // But might find routes to intermediate stations
        for journey in &result.journeys {
            assert!(journey.change_count() <= 2);
        }
    }

    #[tokio::test]
    async fn empty_result_when_no_route() {
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
        let result = planner.search(&request).await.unwrap();

        assert!(result.journeys.is_empty());
    }

    #[tokio::test]
    async fn intermediate_stops_with_only_departure_times() {
        // Simulates Elizabeth Line / Darwin behavior where intermediate calling points
        // only have departure times (no arrival times). The search should still generate
        // initial states for these stops by falling back to departure time.
        //
        // Route: ZLW -> LST -> ZFD -> PAD (terminus)
        // Only ZFD has a connection to CTK (destination)
        let mut zlw = Call::new(crs("ZLW"), "Whitechapel".into());
        zlw.booked_departure = Some(time("23:00"));

        let mut lst = Call::new(crs("LST"), "Liverpool Street".into());
        lst.booked_departure = Some(time("23:05")); // No arrival time!

        let mut zfd = Call::new(crs("ZFD"), "Farringdon".into());
        zfd.booked_departure = Some(time("23:08")); // No arrival time!

        let mut pad = Call::new(crs("PAD"), "Paddington".into());
        pad.booked_arrival = Some(time("23:20")); // Terminus has arrival

        let current = Arc::new(Service {
            service_ref: ServiceRef::new("ELIZ1".into(), crs("ZLW")),
            headcode: None,
            operator: "Elizabeth Line".into(),
            operator_code: None,
            calls: vec![zlw, lst, zfd, pad],
            board_station_idx: CallIndex(0),
        });

        // Connection from ZFD to CTK (Thameslink)
        let connection = make_service(
            "TL1",
            "ZFD",
            &[
                ("ZFD", "Farringdon", "", "23:15"),
                ("CTK", "City Thameslink", "23:18", ""),
            ],
        );

        let provider = MockProvider::new(vec![connection]);
        let walkable = WalkableConnections::new();
        let config = SearchConfig::default();

        let planner = Planner::new(&provider, &walkable, &config);

        // User at ZLW (position 0), wants CTK
        let request = SearchRequest::new(current, CallIndex(0), crs("CTK"));
        let result = planner.search(&request).await.unwrap();

        // Should find the journey: ZLW->ZFD (Elizabeth Line) + ZFD->CTK (Thameslink)
        assert!(
            !result.journeys.is_empty(),
            "Should find journey via Farringdon even though intermediate stops lack arrival times"
        );
        let journey = &result.journeys[0];
        assert_eq!(journey.change_count(), 1);
        assert_eq!(journey.arrival_time(), time("23:18"));
    }
}

/// Property-based tests comparing BFS planner to reference implementation.
#[cfg(test)]
mod proptests {
    use super::*;
    use crate::domain::{Call, ServiceRef};
    use crate::planner::rank::remove_dominated;
    use chrono::{NaiveDate, NaiveTime};
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// Mock service provider for testing
    struct MockProvider {
        services: Vec<Arc<Service>>,
    }

    impl ServiceProvider for MockProvider {
        async fn get_departures(
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

    /// Helper to run async search in a blocking context for proptest
    fn block_on_search<P: ServiceProvider>(
        planner: &Planner<'_, P>,
        request: &SearchRequest,
    ) -> Result<SearchResult, SearchError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(planner.search(request))
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_time(hour: u32, min: u32) -> RailTime {
        let time = NaiveTime::from_hms_opt(hour % 24, min % 60, 0).unwrap();
        RailTime::new(date(), time)
    }

    // Station codes for our test network
    const STATIONS: [&str; 6] = ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"];

    fn station_crs(idx: usize) -> Crs {
        Crs::parse(STATIONS[idx % STATIONS.len()]).unwrap()
    }

    /// Generate a service connecting two stations
    fn make_test_service(
        id: u32,
        from_idx: usize,
        to_idx: usize,
        dep_mins: u16,
        duration_mins: u16,
    ) -> Arc<Service> {
        let from = station_crs(from_idx);
        let to = station_crs(to_idx);

        let dep_hour = (dep_mins / 60) as u32 % 24;
        let dep_min = (dep_mins % 60) as u32;
        let arr_mins = dep_mins + duration_mins;
        let arr_hour = (arr_mins / 60) as u32 % 24;
        let arr_min = (arr_mins % 60) as u32;

        let mut origin = Call::new(from, format!("Station {}", from_idx));
        origin.booked_departure = Some(make_time(dep_hour, dep_min));

        let mut dest = Call::new(to, format!("Station {}", to_idx));
        dest.booked_arrival = Some(make_time(arr_hour, arr_min));

        Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{id}"), from),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls: vec![origin, dest],
            board_station_idx: CallIndex(0),
        })
    }

    /// Generate a service network: list of services connecting stations
    fn network_strategy() -> impl Strategy<Value = Vec<Arc<Service>>> {
        // Generate 3-10 services
        prop::collection::vec(
            (
                0u32..100,  // id
                0usize..6,  // from station
                0usize..6,  // to station (different from from)
                0u16..1200, // dep_mins
                15u16..60,  // duration
            ),
            3..10,
        )
        .prop_map(|params| {
            params
                .into_iter()
                .enumerate()
                .filter_map(|(i, (id, from, to, dep, dur))| {
                    // Skip self-loops
                    if from == to {
                        return None;
                    }
                    Some(make_test_service(id + i as u32 * 100, from, to, dep, dur))
                })
                .collect()
        })
    }

    /// Reference implementation: exhaustive DFS search
    /// Returns all valid journeys from current service/position to destination
    fn reference_search(
        current_service: &Arc<Service>,
        current_position: CallIndex,
        destination: &Crs,
        services: &[Arc<Service>],
        walkable: &WalkableConnections,
        config: &SearchConfig,
    ) -> Vec<Journey> {
        let mut results = Vec::new();
        let mut used = HashSet::new();
        used.insert(current_service.service_ref.darwin_id.clone());

        // Try direct route first
        if let Some((dest_idx, _)) = current_service.find_call(destination, current_position.next())
            && let Ok(leg) = Leg::new(current_service.clone(), current_position, dest_idx)
            && let Ok(journey) = Journey::new(vec![Segment::Train(leg)])
        {
            results.push(journey);
        }

        // DFS from each intermediate alighting point
        for alight_idx in (current_position.0 + 1)..current_service.calls.len() {
            let alight_call = &current_service.calls[alight_idx];
            let alight_time = match alight_call.expected_arrival() {
                Some(t) => t,
                None => continue,
            };

            if let Ok(first_leg) = Leg::new(
                current_service.clone(),
                current_position,
                CallIndex(alight_idx),
            ) {
                let first_segment = Segment::Train(first_leg);

                // If this is the destination, we're done
                if &alight_call.station == destination {
                    if let Ok(journey) = Journey::new(vec![first_segment.clone()]) {
                        results.push(journey);
                    }
                    continue;
                }

                // Otherwise, recurse
                reference_dfs(
                    &alight_call.station,
                    alight_time,
                    destination,
                    vec![first_segment],
                    0,
                    &used,
                    services,
                    walkable,
                    config,
                    &mut results,
                );
            }
        }

        results
    }

    /// DFS helper for reference search
    #[allow(clippy::too_many_arguments)]
    fn reference_dfs(
        station: &Crs,
        time: RailTime,
        destination: &Crs,
        segments: Vec<Segment>,
        changes: usize,
        used: &HashSet<String>,
        services: &[Arc<Service>],
        walkable: &WalkableConnections,
        config: &SearchConfig,
        results: &mut Vec<Journey>,
    ) {
        // Pruning
        if changes >= config.max_changes {
            return;
        }

        // Check journey time
        let first_dep = segments.first().map(|s| match s {
            Segment::Train(leg) => leg.departure_time(),
            Segment::Walk(_) => time,
        });
        if let Some(dep) = first_dep
            && time.signed_duration_since(dep) > config.max_journey()
        {
            return;
        }

        // Limit recursion depth to avoid infinite loops
        if segments.len() > 10 {
            return;
        }

        let min_connection = time + config.min_connection();

        // Try each service
        for service in services {
            if used.contains(&service.service_ref.darwin_id) {
                continue;
            }

            // Find if this service stops at our station
            let board_idx = service.calls.iter().position(|c| {
                &c.station == station
                    && c.expected_departure()
                        .is_some_and(|dep| dep >= min_connection)
            });

            let board_idx = match board_idx {
                Some(i) => CallIndex(i),
                None => continue,
            };

            // Try alighting at each subsequent stop
            for alight_idx in (board_idx.0 + 1)..service.calls.len() {
                let alight_call = &service.calls[alight_idx];
                let alight_time = match alight_call.expected_arrival() {
                    Some(t) => t,
                    None => continue,
                };

                if let Ok(leg) = Leg::new(service.clone(), board_idx, CallIndex(alight_idx)) {
                    let mut new_segments = segments.clone();
                    new_segments.push(Segment::Train(leg));

                    // Check if we reached destination
                    if &alight_call.station == destination {
                        if let Ok(journey) = Journey::new(new_segments) {
                            results.push(journey);
                        }
                    } else {
                        // Recurse
                        let mut new_used = used.clone();
                        new_used.insert(service.service_ref.darwin_id.clone());

                        reference_dfs(
                            &alight_call.station,
                            alight_time,
                            destination,
                            new_segments,
                            changes + 1,
                            &new_used,
                            services,
                            walkable,
                            config,
                            results,
                        );
                    }
                }
            }
        }

        // Try walking
        for (walk_to, walk_duration) in walkable.walkable_from(station) {
            if walk_duration > config.max_walk() {
                continue;
            }

            let walk = Walk {
                from: *station,
                to: walk_to,
                duration: walk_duration,
            };
            let walk_arrival = time + walk_duration;

            let mut new_segments = segments.clone();
            new_segments.push(Segment::Walk(walk));

            // Check if walk destination is our destination
            if walk_to == *destination {
                if let Ok(journey) = Journey::new(new_segments) {
                    results.push(journey);
                }
            } else {
                // Recurse (walking doesn't count as a change)
                reference_dfs(
                    &walk_to,
                    walk_arrival,
                    destination,
                    new_segments,
                    changes,
                    used,
                    services,
                    walkable,
                    config,
                    results,
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(50))]

        /// BFS should find all Pareto-optimal journeys that reference finds
        #[test]
        fn bfs_finds_pareto_optimal(network in network_strategy()) {
            // Skip empty networks
            if network.is_empty() {
                return Ok(());
            }

            // Pick first service as current, destination = last station
            let current = network[0].clone();
            let dest = station_crs(5); // FFF

            // Build mock provider
            let provider = MockProvider { services: network.clone() };
            let walkable = WalkableConnections::new();
            let config = SearchConfig {
                max_changes: 2,
                max_results: 100, // Don't limit results for comparison
                ..Default::default()
            };

            // Run BFS
            let planner = Planner::new(&provider, &walkable, &config);
            let request = SearchRequest::new(current.clone(), CallIndex(0), dest);

            // Skip invalid requests
            if request.validate().is_err() {
                return Ok(());
            }

            let bfs_result = block_on_search(&planner, &request)?;

            // Run reference
            let ref_journeys = reference_search(
                &current,
                CallIndex(0),
                &dest,
                &network,
                &walkable,
                &config,
            );

            // Get Pareto-optimal from reference
            let ref_pareto = remove_dominated(ref_journeys);

            // Every Pareto-optimal journey from reference should have a
            // non-dominated equivalent in BFS results
            for ref_j in &ref_pareto {
                let has_equivalent = bfs_result.journeys.iter().any(|bfs_j| {
                    // Either same or BFS dominates it (both acceptable)
                    bfs_j.arrival_time() <= ref_j.arrival_time()
                        && bfs_j.change_count() <= ref_j.change_count()
                        && bfs_j.total_duration() <= ref_j.total_duration()
                });

                prop_assert!(
                    has_equivalent,
                    "BFS missed Pareto-optimal journey: arr={:?}, changes={}, dur={:?}",
                    ref_j.arrival_time(),
                    ref_j.change_count(),
                    ref_j.total_duration()
                );
            }
        }

        /// All BFS results should satisfy constraints
        #[test]
        fn bfs_respects_constraints(network in network_strategy()) {
            if network.is_empty() {
                return Ok(());
            }

            let current = network[0].clone();
            let dest = station_crs(5);

            let provider = MockProvider { services: network };
            let walkable = WalkableConnections::new();
            let config = SearchConfig {
                max_changes: 2,
                ..Default::default()
            };

            let planner = Planner::new(&provider, &walkable, &config);
            let request = SearchRequest::new(current, CallIndex(0), dest);

            if request.validate().is_err() {
                return Ok(());
            }

            let result = block_on_search(&planner, &request)?;

            for journey in &result.journeys {
                prop_assert!(
                    journey.change_count() <= config.max_changes,
                    "Journey exceeds max_changes"
                );
            }
        }
    }

    // Instrumented test to verify we find journeys sometimes
    #[test]
    fn search_finds_some_journeys() {
        use proptest::test_runner::{Config, TestRunner};
        use std::cell::Cell;

        let mut runner = TestRunner::new(Config::with_cases(100));
        let journeys_found = Cell::new(0u32);
        let tests_with_valid_request = Cell::new(0u32);

        let _ = runner.run(&network_strategy(), |network| {
            if network.is_empty() {
                return Ok(());
            }

            let current = network[0].clone();
            let dest = station_crs(5);

            let provider = MockProvider {
                services: network.clone(),
            };
            let walkable = WalkableConnections::new();
            let config = SearchConfig::default();

            let planner = Planner::new(&provider, &walkable, &config);
            let request = SearchRequest::new(current, CallIndex(0), dest);

            if request.validate().is_ok() {
                tests_with_valid_request.set(tests_with_valid_request.get() + 1);

                if let Ok(result) = block_on_search(&planner, &request)
                    && !result.journeys.is_empty()
                {
                    journeys_found.set(journeys_found.get() + 1);
                }
            }

            Ok(())
        });

        // Not all random networks will have routes, but some should
        // This is informational - print stats
        println!(
            "Found journeys in {}/{} valid tests",
            journeys_found.get(),
            tests_with_valid_request.get()
        );
    }
}
