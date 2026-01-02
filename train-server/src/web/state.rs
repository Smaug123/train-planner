//! Application state for the web layer.

use std::sync::Arc;

use crate::cache::CachedDarwinClient;
use crate::planner::SearchConfig;
use crate::walkable::WalkableConnections;

/// Shared application state.
///
/// Contains all the services needed to handle requests.
#[derive(Clone)]
pub struct AppState {
    /// Cached Darwin API client
    pub darwin: Arc<CachedDarwinClient>,

    /// Walkable connections between stations
    pub walkable: Arc<WalkableConnections>,

    /// Journey planner configuration
    pub config: Arc<SearchConfig>,
}

impl AppState {
    /// Create a new app state.
    pub fn new(
        darwin: CachedDarwinClient,
        walkable: WalkableConnections,
        config: SearchConfig,
    ) -> Self {
        Self {
            darwin: Arc::new(darwin),
            walkable: Arc::new(walkable),
            config: Arc::new(config),
        }
    }
}
