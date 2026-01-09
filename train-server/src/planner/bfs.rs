//! BFS fallback for 3+ change journeys.
//!
//! This module implements a forward BFS search that is used when the arrivals-first
//! approach needs to find journeys with more than 2 changes. The key optimization
//! is that whenever we reach a feeder station (one with direct service to the
//! destination), we can complete the journey via the ArrivalsIndex without further
//! exploration.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Duration;
use futures::future::join_all;
use tracing::{debug, trace};

use super::arrivals_index::ArrivalsIndex;
use super::config::SearchConfig;
use super::search::ServiceProvider;
use crate::domain::{CallIndex, Crs, Journey, Leg, RailTime, Segment, Service, Walk};
use crate::walkable::WalkableConnections;

/// BFS state: partial journey ending at a station with available time.
#[derive(Clone)]
struct BfsState {
    segments: Vec<Segment>,
    station: Crs,
    available_time: RailTime,
    changes_so_far: usize,
}

/// Result of BFS search: found journeys and API call count.
pub struct BfsResult {
    pub journeys: Vec<Journey>,
    pub api_calls: usize,
}

/// Parameters for BFS search, bundled for cleaner function signature.
pub struct BfsParams<'a> {
    pub current_service: &'a Arc<Service>,
    pub current_position: CallIndex,
    pub destination: Crs,
    pub start_time: RailTime,
}

/// Run BFS fallback search for 3+ change journeys.
///
/// This is called when arrivals-first search hasn't found enough journeys
/// and max_changes > 2. It uses forward BFS but with a key optimization:
/// whenever we reach a feeder station, we can complete the journey via
/// the ArrivalsIndex without further exploration.
pub async fn find_bfs_journeys<P: ServiceProvider>(
    params: &BfsParams<'_>,
    index: &ArrivalsIndex,
    departures_cache: &mut HashMap<Crs, Vec<Arc<Service>>>,
    walkable: &WalkableConnections,
    config: &SearchConfig,
    provider: &P,
) -> BfsResult {
    let mut journeys = Vec::new();
    let mut api_calls = 0;

    let min_connection = config.min_connection();
    let max_journey = config.max_journey();
    let max_walk = config.max_walk();

    // Track visited (station, change_level) to avoid redundant exploration
    let mut visited_states: HashSet<(Crs, usize)> = HashSet::new();

    // Initialize frontier with all stations on current train
    let train = params.current_service;
    let pos = params.current_position.0;

    let mut frontier: Vec<BfsState> = Vec::new();

    for (alight_idx, alight_call) in train.calls.iter().enumerate().skip(pos) {
        if alight_call.is_cancelled {
            continue;
        }
        if alight_call.station == params.destination {
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
            params.current_position,
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
        for (walkable_station, walk_time) in walkable.walkable_from(&alight_call.station) {
            if walk_time > max_walk {
                continue;
            }
            let walk = Walk::new(alight_call.station, walkable_station, walk_time);
            frontier.push(BfsState {
                segments: vec![Segment::Train(leg.clone()), Segment::Walk(walk)],
                station: walkable_station,
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
            if state.changes_so_far >= config.max_changes {
                continue;
            }

            // Skip if total journey time would exceed limit
            let elapsed = state
                .available_time
                .signed_duration_since(params.start_time);
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

                    let total_duration =
                        feeder.dest_arrival.signed_duration_since(params.start_time);
                    if total_duration > max_journey {
                        continue;
                    }

                    let alight_idx = match feeder
                        .service
                        .calls
                        .iter()
                        .position(|c| c.station == params.destination)
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
        let batch_calls = batch_fetch_departures(
            &stations_vec,
            params.start_time,
            departures_cache,
            config,
            provider,
        )
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
                    if alight_call.station == params.destination {
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

                    let total_so_far = arrival_time.signed_duration_since(params.start_time);
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
                    for (walkable_station, walk_time) in
                        walkable.walkable_from(&alight_call.station)
                    {
                        if walk_time > max_walk {
                            continue;
                        }
                        let walk = Walk::new(alight_call.station, walkable_station, walk_time);
                        let mut walk_segments = new_segments.clone();
                        walk_segments.push(Segment::Walk(walk));

                        next_frontier.push(BfsState {
                            segments: walk_segments,
                            station: walkable_station,
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

    BfsResult {
        journeys,
        api_calls,
    }
}

/// Batch fetch departures for multiple stations in parallel.
///
/// Fetches departures for all given stations, respecting `batch_size` for
/// parallelism. Results are inserted into the cache. Returns the number
/// of API calls made.
async fn batch_fetch_departures<P: ServiceProvider>(
    stations: &[Crs],
    after: RailTime,
    cache: &mut HashMap<Crs, Vec<Arc<Service>>>,
    config: &SearchConfig,
    provider: &P,
) -> usize {
    if stations.is_empty() {
        return 0;
    }

    let mut api_calls = 0;

    for batch in stations.chunks(config.batch_size) {
        let futures: Vec<_> = batch
            .iter()
            .map(|station| async move {
                let result = provider.get_departures(station, after).await;
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
