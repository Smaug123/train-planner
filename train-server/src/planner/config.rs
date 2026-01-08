//! Search configuration for the journey planner.

use chrono::Duration;

/// Configuration parameters for journey search.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    /// Maximum number of train changes allowed.
    pub max_changes: usize,

    /// Maximum number of journeys to return.
    pub max_results: usize,

    /// How far ahead to search for connections (minutes).
    pub time_window_mins: i64,

    /// Minimum time required for a connection (minutes).
    /// Connections tighter than this are rejected.
    pub min_connection_mins: i64,

    /// Maximum walking time to consider (minutes).
    /// Walks longer than this are not suggested.
    pub max_walk_mins: i64,

    /// Maximum total journey time (minutes).
    /// Journeys longer than this are pruned during search.
    pub max_journey_mins: i64,

    /// Maximum number of states to batch for parallel departure fetching.
    /// Higher values increase parallelism but may do redundant work.
    pub batch_size: usize,
}

impl SearchConfig {
    /// Create a new configuration with the given parameters.
    pub fn new(
        max_changes: usize,
        max_results: usize,
        time_window_mins: i64,
        min_connection_mins: i64,
        max_walk_mins: i64,
        max_journey_mins: i64,
        batch_size: usize,
    ) -> Self {
        Self {
            max_changes,
            max_results,
            time_window_mins,
            min_connection_mins,
            max_walk_mins,
            max_journey_mins,
            batch_size,
        }
    }

    /// Returns the time window as a Duration.
    pub fn time_window(&self) -> Duration {
        Duration::minutes(self.time_window_mins)
    }

    /// Returns the minimum connection time as a Duration.
    pub fn min_connection(&self) -> Duration {
        Duration::minutes(self.min_connection_mins)
    }

    /// Returns the maximum walk time as a Duration.
    pub fn max_walk(&self) -> Duration {
        Duration::minutes(self.max_walk_mins)
    }

    /// Returns the maximum journey time as a Duration.
    pub fn max_journey(&self) -> Duration {
        Duration::minutes(self.max_journey_mins)
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_changes: 3,
            max_results: 10,
            time_window_mins: 120, // 2 hours
            min_connection_mins: 5,
            max_walk_mins: 15,
            max_journey_mins: 360, // 6 hours
            batch_size: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = SearchConfig::default();

        assert_eq!(config.max_changes, 3);
        assert_eq!(config.max_results, 10);
        assert_eq!(config.time_window_mins, 120);
        assert_eq!(config.min_connection_mins, 5);
        assert_eq!(config.max_walk_mins, 15);
        assert_eq!(config.max_journey_mins, 360);
        assert_eq!(config.batch_size, 8);
    }

    #[test]
    fn duration_methods() {
        let config = SearchConfig::default();

        assert_eq!(config.time_window(), Duration::minutes(120));
        assert_eq!(config.min_connection(), Duration::minutes(5));
        assert_eq!(config.max_walk(), Duration::minutes(15));
        assert_eq!(config.max_journey(), Duration::minutes(360));
    }

    #[test]
    fn custom_config() {
        let config = SearchConfig::new(2, 5, 60, 3, 10, 180, 16);

        assert_eq!(config.max_changes, 2);
        assert_eq!(config.max_results, 5);
        assert_eq!(config.time_window_mins, 60);
        assert_eq!(config.min_connection_mins, 3);
        assert_eq!(config.max_walk_mins, 10);
        assert_eq!(config.max_journey_mins, 180);
        assert_eq!(config.batch_size, 16);
    }
}
