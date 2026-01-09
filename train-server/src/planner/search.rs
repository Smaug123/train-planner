//! Arrivals-first journey search algorithm.
//!
//! Instead of forward-searching from the current position (BFS), this algorithm:
//! 1. Fetches the destination's arrivals board (1 API call)
//! 2. Builds an index of "feeder" trains and their calling points
//! 3. Finds direct journeys by checking if current train reaches destination
//! 4. Finds 1-change journeys via set intersection (0 API calls)
//! 5. Finds 2-change journeys by querying departures from non-feeder stations
//!
//! This reduces API calls from ~2000 to ~1-10 for typical journeys.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Duration;
use futures::future::join_all;
use tracing::{debug, info, instrument, trace};

use super::arrivals_index::ArrivalsIndex;
use super::config::SearchConfig;
use super::rank::{deduplicate, rank_journeys, remove_dominated};
use crate::domain::{CallIndex, Crs, Journey, Leg, RailTime, Segment, Service, Walk};
use crate::walkable::WalkableConnections;

/// Provider of train service information.
///
/// Abstracts the data source (real API vs mock) for testing.
pub trait ServiceProvider: Send + Sync {
    /// Get departures from a station after a given time.
    fn get_departures(
        &self,
        station: &Crs,
        after: RailTime,
    ) -> impl std::future::Future<Output = Result<Vec<Arc<Service>>, SearchError>> + Send;

    /// Get arrivals at a station (for destination-first search).
    fn get_arrivals(
        &self,
        station: &Crs,
        after: RailTime,
    ) -> impl std::future::Future<Output = Result<Vec<Arc<Service>>, SearchError>> + Send;
}

/// Error type for search operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SearchError {
    /// Invalid search request.
    #[error("invalid search request: {0}")]
    InvalidRequest(String),

    /// Failed to fetch service data.
    #[error("failed to fetch services at {station}: {message}")]
    FetchError { station: Crs, message: String },

    /// Search timed out.
    #[error("search timed out")]
    Timeout,
}

/// A request to search for journeys.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// The train the user is currently on.
    pub current_service: Arc<Service>,

    /// The user's current position (call index) on the train.
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
            return Err(SearchError::InvalidRequest(format!(
                "Position {} is out of bounds for train with {} calls",
                self.current_position.0,
                self.current_service.calls.len()
            )));
        }

        Ok(())
    }

    /// Get the current station.
    pub fn current_station(&self) -> &Crs {
        &self.current_service.calls[self.current_position.0].station
    }

    /// Get the current time (expected departure from current position).
    pub fn current_time(&self) -> Option<RailTime> {
        let call = &self.current_service.calls[self.current_position.0];
        call.expected_departure().or(call.expected_arrival())
    }
}

/// Result of a journey search.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Found journeys, ranked by preference.
    pub journeys: Vec<Journey>,

    /// Number of API calls made during search.
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

/// Journey planner using arrivals-first search.
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
            "Starting arrivals-first journey search"
        );
        request.validate()?;

        let mut journeys = Vec::new();
        let mut api_calls = 0;
        let mut departures_cache: HashMap<Crs, Vec<Arc<Service>>> = HashMap::new();

        // Phase 1: Check direct journey (current train goes to destination)
        if let Some(j) = self.find_direct(request) {
            debug!("Direct route found on current train");
            journeys.push(j);
        }

        // Early exit: if direct journey exists and no changes allowed, we're done
        if !journeys.is_empty() && self.config.max_changes == 0 {
            return Ok(SearchResult {
                journeys,
                routes_explored: api_calls,
            });
        }

        // Phase 2: Fetch arrivals at destination and build index (1 API call)
        let current_time = request.current_time().ok_or_else(|| {
            SearchError::InvalidRequest("Cannot determine current time".to_string())
        })?;

        let arrivals = self
            .provider
            .get_arrivals(&request.destination, current_time)
            .await?;
        api_calls += 1;

        debug!(
            arrivals = arrivals.len(),
            "Built arrivals index for destination"
        );

        let index = ArrivalsIndex::from_arrivals(request.destination, arrivals);
        debug!(
            feeder_stations = index.feeder_station_count(),
            total_feeders = index.total_feeder_count(),
            "Arrivals index built"
        );

        // Phase 3: Find 1-change journeys (0 API calls)
        if self.config.max_changes >= 1 {
            let one_change = self.find_one_change(request, &index);
            debug!(found = one_change.len(), "Found 1-change journeys");
            journeys.extend(one_change);
        }

        // Phase 4: Find 2-change journeys (limited API calls)
        if self.config.max_changes >= 2 {
            let (two_change, calls) = self
                .find_two_change(request, &index, &mut departures_cache)
                .await?;
            debug!(
                found = two_change.len(),
                api_calls = calls,
                "Found 2-change journeys"
            );
            journeys.extend(two_change);
            api_calls += calls;
        }

        // Phase 5: BFS fallback for 3+ change journeys
        if self.config.max_changes > 2 {
            let (bfs_journeys, bfs_calls) = self
                .find_bfs_fallback(request, &index, &mut departures_cache)
                .await?;
            debug!(
                found = bfs_journeys.len(),
                api_calls = bfs_calls,
                "Found BFS fallback journeys"
            );
            journeys.extend(bfs_journeys);
            api_calls += bfs_calls;
        }

        // Phase 6: Rank, deduplicate, and limit results
        let journeys = remove_dominated(journeys);
        let journeys = deduplicate(journeys);
        let journeys = rank_journeys(journeys);
        let journeys: Vec<Journey> = journeys.into_iter().take(self.config.max_results).collect();

        info!(
            api_calls,
            journeys = journeys.len(),
            "Arrivals-first search complete"
        );

        Ok(SearchResult {
            journeys,
            routes_explored: api_calls,
        })
    }

    /// Find a direct journey (staying on current train to destination).
    fn find_direct(&self, request: &SearchRequest) -> Option<Journey> {
        let train = &request.current_service;
        let pos = request.current_position.0;

        // Check if any call after current position is the destination
        // Note: skip(pos + 1) to avoid trying to create a leg from pos to pos
        for (idx, call) in train.calls.iter().enumerate().skip(pos + 1) {
            if call.station == request.destination && !call.is_cancelled {
                // Found direct journey
                let leg = match Leg::new(train.clone(), request.current_position, CallIndex(idx)) {
                    Ok(l) => l,
                    Err(_) => continue,
                };
                return Journey::new(vec![Segment::Train(leg)]).ok();
            }
        }

        // Also check walkable destinations from any stop
        for (idx, call) in train.calls.iter().enumerate().skip(pos) {
            if call.is_cancelled {
                continue;
            }

            // Check if we can walk from this stop to destination
            if self
                .walkable
                .is_walkable(&call.station, &request.destination)
            {
                let walk_duration = self.walkable.get(&call.station, &request.destination)?;

                // Only if walk is within limits
                if walk_duration <= self.config.max_walk() {
                    let leg =
                        Leg::new(train.clone(), request.current_position, CallIndex(idx)).ok()?;
                    let walk = Walk::new(call.station, request.destination, walk_duration);
                    return Journey::new(vec![Segment::Train(leg), Segment::Walk(walk)]).ok();
                }
            }
        }

        None
    }

    /// Find 1-change journeys using the arrivals index.
    ///
    /// For each station on the current train after our position, check if it's
    /// a feeder station (has services going to destination). If so, check timing
    /// constraints for valid connections.
    fn find_one_change(&self, request: &SearchRequest, index: &ArrivalsIndex) -> Vec<Journey> {
        let mut journeys = Vec::new();
        let train = &request.current_service;
        let pos = request.current_position.0;
        let min_connection = self.config.min_connection();
        let max_journey = self.config.max_journey();
        let max_walk = self.config.max_walk();
        let start_time = match request.current_time() {
            Some(t) => t,
            None => return journeys,
        };

        // For each station on current train after our position
        for (alight_idx, alight_call) in train.calls.iter().enumerate().skip(pos) {
            if alight_call.is_cancelled {
                continue;
            }

            // Skip destination itself (handled by direct)
            if alight_call.station == request.destination {
                continue;
            }

            let arrival_at_alight = match alight_call
                .expected_arrival()
                .or_else(|| alight_call.expected_departure())
            {
                Some(t) => t,
                None => continue,
            };

            // Check both the station itself and walkable neighbours
            let stations_to_check: Vec<(Crs, Duration)> =
                std::iter::once((alight_call.station, Duration::zero()))
                    .chain(
                        self.walkable
                            .walkable_from(&alight_call.station)
                            .into_iter()
                            .filter(|(_, d)| *d <= max_walk),
                    )
                    .collect();

            for (feeder_station, walk_time) in stations_to_check {
                // Get services at this feeder station going to destination
                for feeder in index.feeders_at(&feeder_station) {
                    // Calculate connection time (including walk if needed)
                    let available_time = arrival_at_alight + walk_time;
                    let connection_time = feeder.board_time.signed_duration_since(available_time);

                    // Check timing constraints
                    if connection_time < min_connection {
                        trace!(
                            station = %feeder_station.as_str(),
                            connection_mins = connection_time.num_minutes(),
                            "Skipping: connection too tight"
                        );
                        continue; // Not enough time to make connection
                    }

                    let total_duration = feeder.dest_arrival.signed_duration_since(start_time);
                    if total_duration > max_journey {
                        trace!(
                            station = %feeder_station.as_str(),
                            duration_mins = total_duration.num_minutes(),
                            "Skipping: journey too long"
                        );
                        continue; // Journey too long
                    }

                    // Build the journey
                    if let Some(journey) = self.build_one_change_journey(
                        train,
                        request.current_position,
                        CallIndex(alight_idx),
                        &feeder.service,
                        feeder.board_index,
                        &alight_call.station,
                        &feeder_station,
                        walk_time,
                        &request.destination,
                    ) {
                        journeys.push(journey);
                    }
                }
            }
        }

        journeys
    }

    /// Build a 1-change journey from the given components.
    #[allow(clippy::too_many_arguments)]
    fn build_one_change_journey(
        &self,
        first_train: &Arc<Service>,
        board_first: CallIndex,
        alight_first: CallIndex,
        second_train: &Arc<Service>,
        board_second: CallIndex,
        alight_station: &Crs,
        board_station: &Crs,
        walk_time: Duration,
        destination: &Crs,
    ) -> Option<Journey> {
        let leg1 = Leg::new(first_train.clone(), board_first, alight_first).ok()?;

        // Find where second train arrives at destination
        // Note: service may continue past destination, so find actual destination call
        let alight_second_idx = second_train
            .calls
            .iter()
            .position(|c| c.station == *destination)?;
        let leg2 = Leg::new(
            second_train.clone(),
            board_second,
            CallIndex(alight_second_idx),
        )
        .ok()?;

        let mut segments = vec![Segment::Train(leg1)];

        // Add walk if changing between different stations
        if alight_station != board_station {
            segments.push(Segment::Walk(Walk::new(
                *alight_station,
                *board_station,
                walk_time,
            )));
        }

        segments.push(Segment::Train(leg2));

        Journey::new(segments).ok()
    }

    /// Find 2-change journeys.
    ///
    /// For each station on the current train that is NOT a feeder station,
    /// fetch departures and check if any of those services call at a feeder station.
    async fn find_two_change(
        &self,
        request: &SearchRequest,
        index: &ArrivalsIndex,
        departures_cache: &mut HashMap<Crs, Vec<Arc<Service>>>,
    ) -> Result<(Vec<Journey>, usize), SearchError> {
        let mut journeys = Vec::new();

        let train = &request.current_service;
        let pos = request.current_position.0;
        let min_connection = self.config.min_connection();
        let max_journey = self.config.max_journey();
        let max_walk = self.config.max_walk();
        let start_time = match request.current_time() {
            Some(t) => t,
            None => return Ok((journeys, 0)),
        };

        // Collect stations to query (all stops on current train, including feeders)
        // Also include walkable stations from each stop
        let mut stations_to_query: Vec<(usize, Crs, Duration)> = Vec::new();

        for (alight_idx, alight_call) in train.calls.iter().enumerate().skip(pos) {
            if alight_call.is_cancelled {
                continue;
            }

            // Skip destination
            if alight_call.station == request.destination {
                continue;
            }

            // Include ALL stations (including feeders) for 2-change exploration.
            // Even if a station is a feeder, we need to explore 2-change paths through it
            // because the 1-change via that feeder might be rejected (too long, bad timing).
            stations_to_query.push((alight_idx, alight_call.station, Duration::zero()));

            // Also check walkable neighbours
            for (walkable_station, walk_time) in self.walkable.walkable_from(&alight_call.station) {
                if walk_time <= max_walk {
                    stations_to_query.push((alight_idx, walkable_station, walk_time));
                }
            }
        }

        // Deduplicate by station (keep the one with earliest arrival at query station)
        // Sort by station (as string), then by arrival time at query station
        stations_to_query.sort_by(|(idx_a, s_a, w_a), (idx_b, s_b, w_b)| {
            let arrival_at_query = |idx: usize, walk: &Duration| {
                train.calls[idx]
                    .expected_arrival()
                    .or_else(|| train.calls[idx].expected_departure())
                    .map(|t| t + *walk)
            };

            s_a.as_str()
                .cmp(s_b.as_str())
                .then(arrival_at_query(*idx_a, w_a).cmp(&arrival_at_query(*idx_b, w_b)))
        });
        stations_to_query.dedup_by(|a, b| a.1 == b.1);

        // Collect unique stations that need fetching (not in cache)
        let uncached_stations: Vec<Crs> = stations_to_query
            .iter()
            .map(|(_, station, _)| *station)
            .filter(|s| !departures_cache.contains_key(s))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        debug!(
            total_stations = stations_to_query.len(),
            uncached = uncached_stations.len(),
            "Fetching departures for 2-change search"
        );

        // Batch fetch departures in parallel.
        // We use start_time (current position) for all stations rather than per-station
        // arrival times. This is correct because Darwin's time window has a fixed end point
        // (now + 120 min max); using an earlier start fetches a superset of departures.
        // The filtering at line ~569 discards departures we can't actually catch.
        let api_calls = self
            .batch_fetch_departures(&uncached_stations, start_time, departures_cache)
            .await;

        // Now process synchronously using the cache
        for (alight_idx, query_station, walk_to_query) in stations_to_query {
            let alight_call = &train.calls[alight_idx];

            let arrival_at_alight = match alight_call
                .expected_arrival()
                .or_else(|| alight_call.expected_departure())
            {
                Some(t) => t,
                None => continue,
            };

            // Time when we're available to board at the query station
            let available_at_query = arrival_at_alight + walk_to_query + min_connection;

            // Get departures from cache
            let departures = departures_cache
                .get(&query_station)
                .cloned()
                .unwrap_or_default();

            trace!(
                station = %query_station.as_str(),
                departures = departures.len(),
                "Processing departures for 2-change search"
            );

            // Check each departing service for connections to feeder stations
            for bridge_service in &departures {
                // Find where we board this service
                let bridge_board_idx = match bridge_service
                    .calls
                    .iter()
                    .position(|c| c.station == query_station)
                {
                    Some(idx) => idx,
                    None => continue,
                };

                // Check if service departs after we're available
                let bridge_board_call = &bridge_service.calls[bridge_board_idx];
                let bridge_depart = match bridge_board_call.expected_departure() {
                    Some(t) => t,
                    None => continue,
                };
                if bridge_depart < available_at_query {
                    continue;
                }

                // For each call on the bridge service AFTER where we board
                for (bridge_alight_idx, bridge_call) in bridge_service
                    .calls
                    .iter()
                    .enumerate()
                    .skip(bridge_board_idx + 1)
                {
                    if bridge_call.is_cancelled {
                        continue;
                    }

                    let bridge_arrival = match bridge_call
                        .expected_arrival()
                        .or_else(|| bridge_call.expected_departure())
                    {
                        Some(t) => t,
                        None => continue,
                    };

                    // Check if this call's station (or walkable neighbour) is a feeder
                    let feeder_candidates: Vec<(Crs, Duration)> =
                        std::iter::once((bridge_call.station, Duration::zero()))
                            .chain(
                                self.walkable
                                    .walkable_from(&bridge_call.station)
                                    .into_iter()
                                    .filter(|(_, d)| *d <= max_walk),
                            )
                            .collect();

                    for (feeder_station, walk_to_feeder) in feeder_candidates {
                        for feeder in index.feeders_at(&feeder_station) {
                            // Check timing: can we make the connection?
                            let available_at_feeder = bridge_arrival + walk_to_feeder;
                            let connection_time =
                                feeder.board_time.signed_duration_since(available_at_feeder);

                            if connection_time < min_connection {
                                continue;
                            }

                            let total_duration =
                                feeder.dest_arrival.signed_duration_since(start_time);
                            if total_duration > max_journey {
                                continue;
                            }

                            // Build the 2-change journey
                            if let Some(journey) = self.build_two_change_journey(
                                train,
                                request.current_position,
                                CallIndex(alight_idx),
                                &alight_call.station,
                                &query_station,
                                walk_to_query,
                                bridge_service,
                                CallIndex(bridge_board_idx),
                                CallIndex(bridge_alight_idx),
                                &bridge_call.station,
                                &feeder_station,
                                walk_to_feeder,
                                &feeder.service,
                                feeder.board_index,
                                &request.destination,
                            ) {
                                journeys.push(journey);
                            }
                        }
                    }
                }
            }
        }

        Ok((journeys, api_calls))
    }

    /// Batch fetch departures for multiple stations in parallel.
    ///
    /// Fetches departures for all given stations, respecting `batch_size` for
    /// parallelism. Results are inserted into the cache. Returns the number
    /// of API calls made.
    async fn batch_fetch_departures(
        &self,
        stations: &[Crs],
        after: RailTime,
        cache: &mut HashMap<Crs, Vec<Arc<Service>>>,
    ) -> usize {
        if stations.is_empty() {
            return 0;
        }

        let mut api_calls = 0;

        for batch in stations.chunks(self.config.batch_size) {
            let futures: Vec<_> = batch
                .iter()
                .map(|station| async move {
                    let result = self.provider.get_departures(station, after).await;
                    (*station, result)
                })
                .collect();

            let results = join_all(futures).await;

            for (station, result) in results {
                api_calls += 1;
                match result {
                    Ok(deps) => {
                        cache.insert(station, deps);
                    }
                    Err(e) => {
                        debug!(
                            station = %station.as_str(),
                            error = %e,
                            "Failed to fetch departures, using empty"
                        );
                        // Insert empty vec so we don't retry
                        cache.insert(station, vec![]);
                    }
                }
            }
        }

        api_calls
    }

    /// Build a 2-change journey from components.
    #[allow(clippy::too_many_arguments)]
    fn build_two_change_journey(
        &self,
        first_train: &Arc<Service>,
        board_first: CallIndex,
        alight_first: CallIndex,
        alight_first_station: &Crs,
        board_second_station: &Crs,
        walk_to_second: Duration,
        second_train: &Arc<Service>,
        board_second: CallIndex,
        alight_second: CallIndex,
        alight_second_station: &Crs,
        board_third_station: &Crs,
        walk_to_third: Duration,
        third_train: &Arc<Service>,
        board_third: CallIndex,
        destination: &Crs,
    ) -> Option<Journey> {
        let leg1 = Leg::new(first_train.clone(), board_first, alight_first).ok()?;
        let leg2 = Leg::new(second_train.clone(), board_second, alight_second).ok()?;

        // Third train goes to destination
        // Note: service may continue past destination, so find actual destination call
        let alight_third_idx = third_train
            .calls
            .iter()
            .position(|c| c.station == *destination)?;
        let leg3 = Leg::new(
            third_train.clone(),
            board_third,
            CallIndex(alight_third_idx),
        )
        .ok()?;

        let mut segments = vec![Segment::Train(leg1)];

        // Walk between first and second train if needed
        if alight_first_station != board_second_station {
            segments.push(Segment::Walk(Walk::new(
                *alight_first_station,
                *board_second_station,
                walk_to_second,
            )));
        }

        segments.push(Segment::Train(leg2));

        // Walk between second and third train if needed
        if alight_second_station != board_third_station {
            segments.push(Segment::Walk(Walk::new(
                *alight_second_station,
                *board_third_station,
                walk_to_third,
            )));
        }

        segments.push(Segment::Train(leg3));

        Journey::new(segments).ok()
    }

    /// BFS fallback for 3+ change journeys.
    ///
    /// This is called when arrivals-first search hasn't found enough journeys
    /// and max_changes > 2. It uses forward BFS but with a key optimization:
    /// whenever we reach a feeder station, we can complete the journey via
    /// the ArrivalsIndex without further exploration.
    async fn find_bfs_fallback(
        &self,
        request: &SearchRequest,
        index: &ArrivalsIndex,
        departures_cache: &mut HashMap<Crs, Vec<Arc<Service>>>,
    ) -> Result<(Vec<Journey>, usize), SearchError> {
        let mut journeys = Vec::new();
        let mut api_calls = 0;

        let min_connection = self.config.min_connection();
        let max_journey = self.config.max_journey();
        let max_walk = self.config.max_walk();
        let start_time = match request.current_time() {
            Some(t) => t,
            None => return Ok((journeys, api_calls)),
        };

        // BFS state: partial journey ending at a station with available time
        #[derive(Clone)]
        struct BfsState {
            segments: Vec<Segment>,
            station: Crs,
            available_time: RailTime,
            changes_so_far: usize,
        }

        // Track visited (station, change_level) to avoid redundant exploration
        let mut visited_states: HashSet<(Crs, usize)> = HashSet::new();

        // Initialize frontier with all stations on current train
        let train = &request.current_service;
        let pos = request.current_position.0;

        let mut frontier: Vec<BfsState> = Vec::new();

        for (alight_idx, alight_call) in train.calls.iter().enumerate().skip(pos) {
            if alight_call.is_cancelled {
                continue;
            }
            if alight_call.station == request.destination {
                continue; // Direct handled elsewhere
            }

            let arrival_time = match alight_call
                .expected_arrival()
                .or_else(|| alight_call.expected_departure())
            {
                Some(t) => t,
                None => continue,
            };

            // Build first leg
            let leg = match Leg::new(
                train.clone(),
                request.current_position,
                CallIndex(alight_idx),
            ) {
                Ok(l) => l,
                Err(_) => continue,
            };

            // Add state at this station
            frontier.push(BfsState {
                segments: vec![Segment::Train(leg.clone())],
                station: alight_call.station,
                available_time: arrival_time + min_connection,
                changes_so_far: 0, // We're still on the first train
            });

            // Also consider walkable neighbors
            for (walkable, walk_time) in self.walkable.walkable_from(&alight_call.station) {
                if walk_time > max_walk {
                    continue;
                }
                let walk = Walk::new(alight_call.station, walkable, walk_time);
                frontier.push(BfsState {
                    segments: vec![Segment::Train(leg.clone()), Segment::Walk(walk)],
                    station: walkable,
                    available_time: arrival_time + walk_time + min_connection,
                    changes_so_far: 0, // Walks don't count as changes, only train legs do
                });
            }
        }

        // BFS: explore level by level (each level = one more change)
        while !frontier.is_empty() {
            // First pass: filter frontier and collect stations needing departure fetches
            let mut valid_states: Vec<BfsState> = Vec::new();
            let mut stations_to_fetch: HashSet<Crs> = HashSet::new();

            for state in frontier {
                // Check if we've exceeded max changes
                if state.changes_so_far >= self.config.max_changes {
                    continue;
                }

                // Skip if total journey time would exceed limit
                let elapsed = state.available_time.signed_duration_since(start_time);
                if elapsed > max_journey {
                    continue;
                }

                // Skip if we've visited this state at this change level
                let state_key = (state.station, state.changes_so_far);
                if visited_states.contains(&state_key) {
                    continue;
                }
                visited_states.insert(state_key);

                // If this station is a feeder, complete journey via ArrivalsIndex
                if index.is_feeder(&state.station) {
                    for feeder in index.feeders_at(&state.station) {
                        let time_until_feeder = feeder
                            .board_time
                            .signed_duration_since(state.available_time);

                        if time_until_feeder < Duration::zero() {
                            continue;
                        }

                        let total_duration = feeder.dest_arrival.signed_duration_since(start_time);
                        if total_duration > max_journey {
                            continue;
                        }

                        let alight_idx = match feeder
                            .service
                            .calls
                            .iter()
                            .position(|c| c.station == request.destination)
                        {
                            Some(idx) => idx,
                            None => continue,
                        };
                        let final_leg = match Leg::new(
                            feeder.service.clone(),
                            feeder.board_index,
                            CallIndex(alight_idx),
                        ) {
                            Ok(l) => l,
                            Err(_) => continue,
                        };

                        let mut segments = state.segments.clone();
                        segments.push(Segment::Train(final_leg));

                        if let Ok(journey) = Journey::new(segments) {
                            journeys.push(journey);
                        }
                    }
                    // Don't explore further from feeders
                    continue;
                }

                // Need to fetch departures for this station (if not cached)
                if !departures_cache.contains_key(&state.station) {
                    stations_to_fetch.insert(state.station);
                }
                valid_states.push(state);
            }

            // Batch fetch departures for all non-cached stations in parallel.
            // Uses start_time for all stations; see comment in find_two_change for rationale.
            let stations_vec: Vec<Crs> = stations_to_fetch.into_iter().collect();
            let batch_calls = self
                .batch_fetch_departures(&stations_vec, start_time, departures_cache)
                .await;
            api_calls += batch_calls;

            // Now process valid states using cached departures
            let mut next_frontier: Vec<BfsState> = Vec::new();

            for state in valid_states {
                let departures = departures_cache
                    .get(&state.station)
                    .cloned()
                    .unwrap_or_default();

                trace!(
                    station = %state.station.as_str(),
                    departures = departures.len(),
                    changes = state.changes_so_far,
                    "BFS exploring station"
                );

                // Explore each departing service
                for service in &departures {
                    let board_idx = match service
                        .calls
                        .iter()
                        .position(|c| c.station == state.station)
                    {
                        Some(idx) => idx,
                        None => continue,
                    };

                    let board_call = &service.calls[board_idx];
                    let board_time = match board_call.expected_departure() {
                        Some(t) => t,
                        None => continue,
                    };

                    if board_time < state.available_time {
                        continue;
                    }

                    for (alight_idx, alight_call) in
                        service.calls.iter().enumerate().skip(board_idx + 1)
                    {
                        if alight_call.is_cancelled {
                            continue;
                        }

                        // If we reach destination directly, that's a valid journey
                        if alight_call.station == request.destination {
                            let leg = match Leg::new(
                                service.clone(),
                                CallIndex(board_idx),
                                CallIndex(alight_idx),
                            ) {
                                Ok(l) => l,
                                Err(_) => continue,
                            };

                            let mut segments = state.segments.clone();
                            segments.push(Segment::Train(leg));

                            if let Ok(journey) = Journey::new(segments) {
                                journeys.push(journey);
                            }
                            continue;
                        }

                        let arrival_time = match alight_call
                            .expected_arrival()
                            .or_else(|| alight_call.expected_departure())
                        {
                            Some(t) => t,
                            None => continue,
                        };

                        let total_so_far = arrival_time.signed_duration_since(start_time);
                        if total_so_far > max_journey {
                            continue;
                        }

                        let leg = match Leg::new(
                            service.clone(),
                            CallIndex(board_idx),
                            CallIndex(alight_idx),
                        ) {
                            Ok(l) => l,
                            Err(_) => continue,
                        };

                        let mut new_segments = state.segments.clone();
                        new_segments.push(Segment::Train(leg.clone()));

                        next_frontier.push(BfsState {
                            segments: new_segments.clone(),
                            station: alight_call.station,
                            available_time: arrival_time + min_connection,
                            changes_so_far: state.changes_so_far + 1,
                        });

                        // Also add walkable neighbors
                        for (walkable, walk_time) in
                            self.walkable.walkable_from(&alight_call.station)
                        {
                            if walk_time > max_walk {
                                continue;
                            }
                            let walk = Walk::new(alight_call.station, walkable, walk_time);
                            let mut walk_segments = new_segments.clone();
                            walk_segments.push(Segment::Walk(walk));

                            next_frontier.push(BfsState {
                                segments: walk_segments,
                                station: walkable,
                                available_time: arrival_time + walk_time + min_connection,
                                changes_so_far: state.changes_so_far + 1,
                            });
                        }
                    }
                }
            }

            frontier = next_frontier;
        }

        debug!(
            journeys = journeys.len(),
            api_calls, "BFS fallback complete"
        );

        Ok((journeys, api_calls))
    }
}

#[cfg(test)]
mod tests {
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
            walk.from, crs("STB"),
            "Walk should be from STB (shorter walk time)"
        );
        assert_eq!(walk.to, crs("QRY"));
    }
}

/// Property-based tests comparing arrivals-first search against naive BFS.
#[cfg(test)]
mod proptests {
    use super::*;
    use crate::domain::{Call, ServiceRef};
    use chrono::{NaiveDate, NaiveTime};
    use proptest::prelude::*;
    use std::collections::HashMap;

    // ========== Test infrastructure ==========

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2024, 3, 15).unwrap()
    }

    fn make_time(mins_from_midnight: u16) -> RailTime {
        let hour = (mins_from_midnight / 60) as u32 % 24;
        let min = (mins_from_midnight % 60) as u32;
        let time = NaiveTime::from_hms_opt(hour, min, 0).unwrap();
        RailTime::new(date(), time)
    }

    fn crs(s: &str) -> Crs {
        Crs::parse(s).unwrap()
    }

    /// A small fixed set of station codes for testing.
    const STATIONS: [&str; 8] = ["PAD", "RDG", "SWI", "BRI", "OXF", "DID", "KGX", "STP"];

    fn station_crs(idx: usize) -> Crs {
        crs(STATIONS[idx % STATIONS.len()])
    }

    /// Create a service with the given calls.
    fn make_service(
        id: usize,
        calls_data: Vec<(usize, u16, u16)>, // (station_idx, arr_mins, dep_mins)
    ) -> Arc<Service> {
        let calls: Vec<Call> = calls_data
            .iter()
            .map(|(station_idx, arr_mins, dep_mins)| {
                let station = station_crs(*station_idx);
                let mut call = Call::new(station, format!("Station {}", station_idx));
                if *arr_mins > 0 {
                    call.booked_arrival = Some(make_time(*arr_mins));
                }
                if *dep_mins > 0 {
                    call.booked_departure = Some(make_time(*dep_mins));
                }
                call
            })
            .collect();

        let board_crs = calls.first().map(|c| c.station).unwrap_or(crs("PAD"));

        Arc::new(Service {
            service_ref: ServiceRef::new(format!("SVC{id}"), board_crs),
            headcode: None,
            operator: "Test".to_string(),
            operator_code: None,
            calls,
            board_station_idx: CallIndex(0),
        })
    }

    /// Mock provider that serves from pre-configured data.
    struct TestProvider {
        departures: HashMap<Crs, Vec<Arc<Service>>>,
        arrivals: HashMap<Crs, Vec<Arc<Service>>>,
    }

    impl TestProvider {
        fn new(services: &[Arc<Service>]) -> Self {
            let mut departures: HashMap<Crs, Vec<Arc<Service>>> = HashMap::new();
            let mut arrivals: HashMap<Crs, Vec<Arc<Service>>> = HashMap::new();

            for service in services {
                // Add to departures for each station (except last - can't depart from terminus)
                for call in service
                    .calls
                    .iter()
                    .take(service.calls.len().saturating_sub(1))
                {
                    departures
                        .entry(call.station)
                        .or_default()
                        .push(service.clone());
                }
                // Add to arrivals for each station (except first - that's origin/departure only)
                // This matches Darwin API behavior: arrivals at station X includes all services
                // that call at X, not just those terminating there
                for call in service.calls.iter().skip(1) {
                    arrivals
                        .entry(call.station)
                        .or_default()
                        .push(service.clone());
                }
            }

            Self {
                departures,
                arrivals,
            }
        }
    }

    impl ServiceProvider for TestProvider {
        async fn get_departures(
            &self,
            station: &Crs,
            _after: RailTime,
        ) -> Result<Vec<Arc<Service>>, SearchError> {
            Ok(self.departures.get(station).cloned().unwrap_or_default())
        }

        async fn get_arrivals(
            &self,
            station: &Crs,
            _after: RailTime,
        ) -> Result<Vec<Arc<Service>>, SearchError> {
            Ok(self.arrivals.get(station).cloned().unwrap_or_default())
        }
    }

    // ========== Naive BFS reference implementation ==========

    /// Naive BFS search - simple, obviously correct, but inefficient.
    /// This is the reference implementation we compare against.
    async fn naive_bfs_search<P: ServiceProvider>(
        provider: &P,
        walkable: &WalkableConnections,
        config: &SearchConfig,
        request: &SearchRequest,
    ) -> Result<Vec<Journey>, SearchError> {
        let mut journeys = Vec::new();
        let min_connection = config.min_connection();
        let max_journey = config.max_journey();
        let max_walk = config.max_walk();

        let start_time = match request.current_time() {
            Some(t) => t,
            None => return Ok(journeys),
        };

        // BFS state
        #[derive(Clone)]
        struct State {
            segments: Vec<Segment>,
            station: Crs,
            available_time: RailTime,
            changes: usize,
        }

        // Check direct journey first
        let train = &request.current_service;
        let pos = request.current_position.0;

        for (idx, call) in train.calls.iter().enumerate().skip(pos) {
            if call.station == request.destination && !call.is_cancelled {
                let leg = Leg::new(train.clone(), request.current_position, CallIndex(idx)).ok();
                if let Some(leg) = leg
                    && let Ok(j) = Journey::new(vec![Segment::Train(leg)]) {
                        journeys.push(j);
                    }
            }
        }

        // Initialize frontier
        let mut frontier: Vec<State> = Vec::new();

        for (alight_idx, alight_call) in train.calls.iter().enumerate().skip(pos) {
            if alight_call.is_cancelled || alight_call.station == request.destination {
                continue;
            }

            let arrival_time = match alight_call
                .expected_arrival()
                .or_else(|| alight_call.expected_departure())
            {
                Some(t) => t,
                None => continue,
            };

            let leg = match Leg::new(
                train.clone(),
                request.current_position,
                CallIndex(alight_idx),
            ) {
                Ok(l) => l,
                Err(_) => continue,
            };

            frontier.push(State {
                segments: vec![Segment::Train(leg.clone())],
                station: alight_call.station,
                available_time: arrival_time + min_connection,
                changes: 0,
            });

            // Walkable neighbors
            for (walkable_station, walk_time) in walkable.walkable_from(&alight_call.station) {
                if walk_time > max_walk {
                    continue;
                }
                let walk = Walk::new(alight_call.station, walkable_station, walk_time);
                frontier.push(State {
                    segments: vec![Segment::Train(leg.clone()), Segment::Walk(walk)],
                    station: walkable_station,
                    available_time: arrival_time + walk_time + min_connection,
                    changes: 1,
                });
            }
        }

        // BFS exploration
        while !frontier.is_empty() {
            let mut next_frontier: Vec<State> = Vec::new();

            for state in frontier {
                if state.changes >= config.max_changes {
                    continue;
                }

                let elapsed = state.available_time.signed_duration_since(start_time);
                if elapsed > max_journey {
                    continue;
                }

                // Get departures
                let departures = provider
                    .get_departures(&state.station, state.available_time)
                    .await?;

                for service in &departures {
                    let board_idx = match service
                        .calls
                        .iter()
                        .position(|c| c.station == state.station)
                    {
                        Some(idx) => idx,
                        None => continue,
                    };

                    let board_call = &service.calls[board_idx];
                    let board_time = match board_call.expected_departure() {
                        Some(t) => t,
                        None => continue,
                    };

                    if board_time < state.available_time {
                        continue;
                    }

                    for (alight_idx, alight_call) in
                        service.calls.iter().enumerate().skip(board_idx + 1)
                    {
                        if alight_call.is_cancelled {
                            continue;
                        }

                        let arrival_time = match alight_call
                            .expected_arrival()
                            .or_else(|| alight_call.expected_departure())
                        {
                            Some(t) => t,
                            None => continue,
                        };

                        let total_so_far = arrival_time.signed_duration_since(start_time);
                        if total_so_far > max_journey {
                            continue;
                        }

                        let leg = match Leg::new(
                            service.clone(),
                            CallIndex(board_idx),
                            CallIndex(alight_idx),
                        ) {
                            Ok(l) => l,
                            Err(_) => continue,
                        };

                        let mut new_segments = state.segments.clone();
                        new_segments.push(Segment::Train(leg));

                        // Check if reached destination
                        if alight_call.station == request.destination {
                            if let Ok(j) = Journey::new(new_segments.clone()) {
                                journeys.push(j);
                            }
                            continue;
                        }

                        // Add to next frontier
                        next_frontier.push(State {
                            segments: new_segments.clone(),
                            station: alight_call.station,
                            available_time: arrival_time + min_connection,
                            changes: state.changes + 1,
                        });

                        // Walkable neighbors
                        for (walkable_station, walk_time) in
                            walkable.walkable_from(&alight_call.station)
                        {
                            if walk_time > max_walk {
                                continue;
                            }

                            // Check if walk reaches destination
                            if walkable_station == request.destination {
                                let walk =
                                    Walk::new(alight_call.station, walkable_station, walk_time);
                                let mut walk_segments = new_segments.clone();
                                walk_segments.push(Segment::Walk(walk));
                                if let Ok(j) = Journey::new(walk_segments) {
                                    journeys.push(j);
                                }
                                continue;
                            }

                            let walk = Walk::new(alight_call.station, walkable_station, walk_time);
                            let mut walk_segments = new_segments.clone();
                            walk_segments.push(Segment::Walk(walk));

                            next_frontier.push(State {
                                segments: walk_segments,
                                station: walkable_station,
                                available_time: arrival_time + walk_time + min_connection,
                                changes: state.changes + 1,
                            });
                        }
                    }
                }
            }

            frontier = next_frontier;
        }

        Ok(journeys)
    }

    // ========== Proptest strategies ==========

    /// Generate a valid service (sequence of calls with increasing times).
    /// Ensures no station is visited twice (no loops).
    fn service_strategy(id: usize) -> impl Strategy<Value = Arc<Service>> {
        // Generate 2-5 UNIQUE station indices
        (
            // Use prop_shuffle to get unique stations
            Just(Vec::from_iter(0..STATIONS.len()))
                .prop_shuffle()
                .prop_map(|v| v.into_iter().take(5).collect::<Vec<_>>()),
            // Number of calls (2-5, but at most the number of unique stations)
            2usize..=5,
            // Start time in minutes from midnight (6am - 10pm)
            360u16..1320,
        )
            .prop_flat_map(move |(shuffled_stations, n_calls, start_time)| {
                let n_calls = n_calls.min(shuffled_stations.len());
                let station_indices: Vec<usize> =
                    shuffled_stations.into_iter().take(n_calls).collect();

                // Generate time gaps between stations (10-60 mins each)
                let n_gaps = station_indices.len().saturating_sub(1);
                prop::collection::vec(10u16..60, n_gaps).prop_map(move |gaps| {
                    let mut calls_data = Vec::new();
                    let mut current_time = start_time;

                    for (i, &station_idx) in station_indices.iter().enumerate() {
                        let arr_mins = if i == 0 { 0 } else { current_time };
                        let dep_mins = if i == station_indices.len() - 1 {
                            0
                        } else {
                            current_time + 2 // 2 min dwell time
                        };
                        calls_data.push((station_idx, arr_mins, dep_mins));

                        if i < gaps.len() {
                            current_time += gaps[i];
                        }
                    }

                    make_service(id, calls_data)
                })
            })
    }

    /// Generate a network of services.
    fn network_strategy() -> impl Strategy<Value = Vec<Arc<Service>>> {
        // Generate 3-8 services
        (3usize..=8).prop_flat_map(|n_services| {
            let strategies: Vec<_> = (0..n_services).map(service_strategy).collect();
            strategies
                .into_iter()
                .collect::<Vec<_>>()
                .prop_map(|services| services)
        })
    }

    /// Generate a search request for a given network.
    fn search_request_strategy(
        services: Vec<Arc<Service>>,
    ) -> impl Strategy<Value = (Vec<Arc<Service>>, SearchRequest, Crs)> {
        // Pick a random service as current train
        let n_services = services.len();
        (0..n_services, 0usize..STATIONS.len()).prop_map(move |(svc_idx, dest_idx)| {
            let current_service = services[svc_idx % services.len()].clone();
            let pos = 0; // Start at first stop
            let destination = station_crs(dest_idx);
            let request = SearchRequest::new(current_service, CallIndex(pos), destination);
            (services.clone(), request, destination)
        })
    }

    /// Combined strategy: generate network + search request.
    fn scenario_strategy() -> impl Strategy<Value = (Vec<Arc<Service>>, SearchRequest, Crs)> {
        network_strategy().prop_flat_map(search_request_strategy)
    }

    // ========== Property tests ==========

    /// For every arrival time found by naive BFS, arrivals-first should
    /// find a journey arriving at the same time or earlier.
    ///
    /// Note: this is weaker than "finds all journeys"—a single early
    /// journey can satisfy multiple naive arrival times.
    fn arrivals_first_dominates_naive_arrival_times(
        services: Vec<Arc<Service>>,
        request: SearchRequest,
    ) -> Result<(), TestCaseError> {
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let provider = TestProvider::new(&services);
            let walkable = WalkableConnections::new();
            let config = SearchConfig {
                max_changes: 2,
                max_results: 100,
                ..SearchConfig::default()
            };

            // Run naive BFS
            let naive_journeys = naive_bfs_search(&provider, &walkable, &config, &request).await?;

            // Run arrivals-first search
            let planner = Planner::new(&provider, &walkable, &config);
            let arrivals_first_result = planner.search(&request).await?;

            // For each journey found by naive BFS, check that arrivals-first
            // found a journey that arrives at the same time or earlier
            let arrivals_first_times: Vec<_> = arrivals_first_result
                .journeys
                .iter()
                .map(|j| j.arrival_time())
                .collect();

            for naive_journey in &naive_journeys {
                let naive_arrival = naive_journey.arrival_time();

                // Check if arrivals-first found any journey arriving <= naive_arrival
                let found_equivalent_or_better =
                    arrivals_first_times.iter().any(|&t| t <= naive_arrival);

                // Debug: show journey details
                let naive_route: Vec<_> = naive_journey
                    .segments()
                    .iter()
                    .map(|s| match s {
                        Segment::Train(leg) => format!(
                            "{}({})@{}->{}@{}",
                            leg.service().service_ref.darwin_id,
                            leg.service().calls.len(),
                            leg.board_station().as_str(),
                            leg.alight_station().as_str(),
                            leg.alight_idx().0
                        ),
                        Segment::Walk(w) => format!("walk:{}->{}", w.from.as_str(), w.to.as_str()),
                    })
                    .collect();

                let current_train_route: Vec<_> = request
                    .current_service
                    .calls
                    .iter()
                    .map(|c| c.station.as_str())
                    .collect();

                prop_assert!(
                    found_equivalent_or_better,
                    "Naive BFS found journey arriving at {:?}, but arrivals-first \
                     didn't find any journey arriving at or before that time.\n\
                     Current train: {:?}\n\
                     Naive journey route: {:?}\n\
                     Naive journeys: {}\n\
                     Arrivals-first journeys: {}",
                    naive_arrival,
                    current_train_route,
                    naive_route,
                    naive_journeys.len(),
                    arrivals_first_result.journeys.len()
                );
            }

            Ok(())
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn arrivals_first_complete((services, request, _dest) in scenario_strategy()) {
            arrivals_first_dominates_naive_arrival_times(services, request)?;
        }
    }

    // ========== Focused tests for edge cases ==========

    /// Test with a scenario requiring exactly 3 changes.
    #[tokio::test]
    async fn reference_three_change_journey() {
        // PAD -> AAA -> BBB -> RDG -> BRI (destination)
        let current_train = make_service(
            0,
            vec![
                (0, 0, 600), // PAD depart 10:00
                (4, 630, 0), // OXF arrive 10:30
            ],
        );

        // OXF -> DID
        let bridge1 = make_service(
            1,
            vec![
                (4, 0, 640), // OXF depart 10:40
                (5, 700, 0), // DID arrive 11:40
            ],
        );

        // DID -> RDG
        let bridge2 = make_service(
            2,
            vec![
                (5, 0, 710), // DID depart 11:50
                (1, 750, 0), // RDG arrive 12:30
            ],
        );

        // RDG -> BRI (arriving service)
        let final_service = make_service(
            3,
            vec![
                (1, 0, 800), // RDG depart 13:20
                (3, 850, 0), // BRI arrive 14:10
            ],
        );

        let services = vec![current_train.clone(), bridge1, bridge2, final_service];

        let provider = TestProvider::new(&services);
        let walkable = WalkableConnections::new();
        let config = SearchConfig {
            max_changes: 3,
            ..SearchConfig::default()
        };

        let request = SearchRequest::new(current_train, CallIndex(0), crs("BRI"));

        // Run both algorithms
        let naive_journeys = naive_bfs_search(&provider, &walkable, &config, &request)
            .await
            .unwrap();

        let planner = Planner::new(&provider, &walkable, &config);
        let arrivals_first = planner.search(&request).await.unwrap();

        // Both should find at least one journey
        assert!(
            !naive_journeys.is_empty(),
            "Naive BFS should find at least one journey"
        );
        assert!(
            !arrivals_first.journeys.is_empty(),
            "Arrivals-first should find at least one journey"
        );

        // Arrivals-first should find journey with same or better arrival time
        let naive_best = naive_journeys
            .iter()
            .map(|j| j.arrival_time())
            .min()
            .unwrap();
        let af_best = arrivals_first
            .journeys
            .iter()
            .map(|j| j.arrival_time())
            .min()
            .unwrap();

        assert!(
            af_best <= naive_best,
            "Arrivals-first best ({:?}) should be <= naive best ({:?})",
            af_best,
            naive_best
        );
    }
}
