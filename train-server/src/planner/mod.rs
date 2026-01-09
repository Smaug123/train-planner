//! Journey planner using arrivals-first search.
//!
//! This module implements the core journey planning algorithm that answers:
//! "I'm on this train at this position - how can I reach my destination?"
//!
//! The algorithm uses an arrivals-first approach: instead of forward-searching
//! from the current position (which leads to combinatorial explosion), we start
//! from the destination by fetching its arrivals board. This gives us all trains
//! that could complete the journey, and their previous calling points, in a single
//! API call. Journeys are then found via set intersection.

mod arrivals_index;
mod bfs;
mod config;
mod rank;
mod search;

pub use arrivals_index::{ArrivalsIndex, FeederInfo};
pub use config::SearchConfig;
pub use rank::{deduplicate, rank_journeys, remove_dominated};
pub use search::{Planner, SearchError, SearchRequest, SearchResult, ServiceProvider};
