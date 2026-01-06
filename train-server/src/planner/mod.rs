//! Journey planner using BFS search.
//!
//! This module implements the core journey planning algorithm that answers:
//! "I'm on this train at this position - how can I reach my destination?"
//!
//! The algorithm uses breadth-first search to explore possible routes,
//! considering train changes and walking connections between stations.

mod config;
mod rank;
mod search;

pub use config::SearchConfig;
pub use rank::rank_journeys;
pub use search::{Planner, SearchError, SearchRequest, SearchResult, ServiceProvider};
