//! Caching layer for Darwin API responses.
//!
//! Darwin service IDs are ephemeral (only valid while the service appears on
//! a departure board). We cache the departure board response which includes
//! calling points, avoiding separate service detail fetches.
//!
//! Time bucketing (5-minute buckets) bounds cache cardinality while ensuring
//! reasonable freshness.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use moka::future::Cache as MokaCache;

use crate::darwin::{ConvertedService, DarwinClient, DarwinError};
use crate::domain::Crs;

/// Cache key for departure boards: (station CRS, date, time bucket).
/// Time bucket is minutes from midnight divided by 5.
type BoardKey = (Crs, NaiveDate, u16);

/// Cached departure board entry.
type BoardEntry = Arc<Vec<Arc<ConvertedService>>>;

/// Configuration for the cache.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// TTL for cached entries.
    pub ttl: Duration,

    /// Maximum number of cached entries.
    pub max_capacity: u64,

    /// Time bucket size in minutes.
    pub bucket_mins: u16,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(60),
            max_capacity: 1000,
            bucket_mins: 5,
        }
    }
}

/// Cache for Darwin API responses.
pub struct DarwinCache {
    /// Departure boards with details, keyed by (station, date, time_bucket).
    boards: MokaCache<BoardKey, BoardEntry>,

    /// Time bucket size in minutes.
    bucket_mins: u16,
}

impl DarwinCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: &CacheConfig) -> Self {
        let boards = MokaCache::builder()
            .time_to_live(config.ttl)
            .max_capacity(config.max_capacity)
            .build();

        Self {
            boards,
            bucket_mins: config.bucket_mins,
        }
    }

    /// Compute the time bucket for a given time offset.
    /// Returns minutes from midnight divided by bucket size.
    fn time_bucket(&self, time_offset_mins: i16, current_mins: u16) -> u16 {
        let mins = (current_mins as i16 + time_offset_mins).rem_euclid(1440) as u16;
        mins / self.bucket_mins
    }

    /// Get a cached board entry.
    pub async fn get_board(&self, key: &BoardKey) -> Option<BoardEntry> {
        self.boards.get(key).await
    }

    /// Insert a board entry into the cache.
    pub async fn insert_board(&self, key: BoardKey, entry: BoardEntry) {
        self.boards.insert(key, entry).await;
    }

    /// Get cache statistics (for monitoring).
    pub fn entry_count(&self) -> u64 {
        self.boards.entry_count()
    }

    /// Invalidate all cached entries.
    pub fn invalidate_all(&self) {
        self.boards.invalidate_all();
    }
}

/// Darwin client with caching.
///
/// Wraps a `DarwinClient` and caches departure board responses.
pub struct CachedDarwinClient {
    client: DarwinClient,
    cache: DarwinCache,
}

impl CachedDarwinClient {
    /// Create a new cached client.
    pub fn new(client: DarwinClient, cache_config: &CacheConfig) -> Self {
        Self {
            client,
            cache: DarwinCache::new(cache_config),
        }
    }

    /// Get departures with details, using cache if available.
    ///
    /// # Arguments
    /// * `crs` - Station CRS code
    /// * `date` - The date for the query
    /// * `current_mins` - Current time in minutes from midnight
    /// * `time_offset` - Offset from current time in minutes (-120 to 120)
    /// * `time_window` - Time window in minutes (0 to 120)
    pub async fn get_departures_with_details(
        &self,
        crs: &Crs,
        date: NaiveDate,
        current_mins: u16,
        time_offset: i16,
        time_window: u16,
    ) -> Result<Arc<Vec<Arc<ConvertedService>>>, DarwinError> {
        let bucket = self.cache.time_bucket(time_offset, current_mins);
        let key = (*crs, date, bucket);

        // Try cache first
        if let Some(cached) = self.cache.get_board(&key).await {
            return Ok(cached);
        }

        // Fetch from API
        let services = self
            .client
            .get_departures_with_details(crs, 15, time_offset, time_window, date)
            .await?;

        // Wrap in Arc for sharing
        let services: Vec<Arc<ConvertedService>> = services.into_iter().map(Arc::new).collect();
        let entry = Arc::new(services);

        // Cache and return
        self.cache.insert_board(key, entry.clone()).await;

        Ok(entry)
    }

    /// Get departures filtered to a specific destination.
    pub async fn get_departures_to(
        &self,
        crs: &Crs,
        date: NaiveDate,
        current_mins: u16,
        time_offset: i16,
        time_window: u16,
        filter_crs: &Crs,
    ) -> Result<Vec<Arc<ConvertedService>>, DarwinError> {
        // Get all departures (cached)
        let all = self
            .get_departures_with_details(crs, date, current_mins, time_offset, time_window)
            .await?;

        // Filter to those calling at destination
        let filtered: Vec<Arc<ConvertedService>> = all
            .iter()
            .filter(|s| s.service.calls.iter().any(|c| &c.station == filter_crs))
            .cloned()
            .collect();

        Ok(filtered)
    }

    /// Access the underlying client for operations that bypass cache.
    pub fn client(&self) -> &DarwinClient {
        &self.client
    }

    /// Get cache statistics.
    pub fn cache_entry_count(&self) -> u64 {
        self.cache.entry_count()
    }

    /// Invalidate all cached entries.
    pub fn invalidate_cache(&self) {
        self.cache.invalidate_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_bucket_calculation() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);

        // 10:00 = 600 mins, bucket size 5 → bucket 120
        assert_eq!(cache.time_bucket(0, 600), 120);

        // 10:04 = 604 mins → bucket 120
        assert_eq!(cache.time_bucket(0, 604), 120);

        // 10:05 = 605 mins → bucket 121
        assert_eq!(cache.time_bucket(0, 605), 121);

        // With offset: current 10:00, offset -30 → 9:30 = 570 mins → bucket 114
        assert_eq!(cache.time_bucket(-30, 600), 114);

        // Wrap around midnight: current 0:10 = 10 mins, offset -20 → 23:50 = 1430 mins
        // 1430 / 5 = 286
        assert_eq!(cache.time_bucket(-20, 10), 286);
    }

    #[test]
    fn default_config() {
        let config = CacheConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(60));
        assert_eq!(config.max_capacity, 1000);
        assert_eq!(config.bucket_mins, 5);
    }

    #[test]
    fn cache_creation() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);
        assert_eq!(cache.entry_count(), 0);
    }
}

/// Tests that demonstrate bugs in the current implementation.
/// These tests are expected to FAIL until the bugs are fixed.
#[cfg(test)]
mod bug_tests {
    use super::*;

    /// BUG: Cache key doesn't include time_window parameter.
    ///
    /// Two requests with the same (station, date, time_bucket) but different
    /// time_window values will share the same cache entry. This means:
    /// - Request with time_window=30 might get cached
    /// - Request with time_window=120 will return the cached 30-min window data
    ///
    /// The cache key should include time_window to prevent this.
    #[test]
    fn bug_cache_key_ignores_time_window() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);

        // Two different time windows should produce different cache keys
        // Currently they don't - both calls use the same key
        let crs = Crs::parse("PAD").unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        let current_mins: u16 = 600; // 10:00

        // Simulate what CachedDarwinClient does
        let bucket_30 = cache.time_bucket(0, current_mins);
        let bucket_120 = cache.time_bucket(0, current_mins);

        // These should be different because time_window differs, but they're the same
        let key_30 = (crs, date, bucket_30);
        let key_120 = (crs, date, bucket_120);

        // This assertion documents the bug: keys are the same when they shouldn't be
        assert_ne!(
            key_30, key_120,
            "Cache keys should differ based on time_window, but they're identical"
        );
    }

    /// BUG: time_bucket calculation doesn't account for the full query range.
    ///
    /// If current_mins=600 (10:00) and time_window=120, the query covers 10:00-12:00.
    /// But the bucket is only calculated from current_mins, not the window.
    /// This means the same cached data could be returned for different effective ranges.
    #[test]
    fn bug_time_bucket_ignores_window_span() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);

        // At 10:04, bucket = 600 / 5 = 120
        let bucket_10_04 = cache.time_bucket(0, 604);

        // At 10:05, bucket = 605 / 5 = 121
        let bucket_10_05 = cache.time_bucket(0, 605);

        // These are different buckets, which is correct for the start time.
        // But if both requests have time_window=120, they overlap significantly.
        // The 10:04 request covers 10:04-12:04
        // The 10:05 request covers 10:05-12:05
        // They share 119 minutes of overlap but get different cache entries!

        assert_eq!(bucket_10_04, bucket_10_05,
            "Requests with overlapping time windows should share cache, but bucket differs by 1 minute");
    }
}
