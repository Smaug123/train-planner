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

use crate::darwin::{ConvertedService, DarwinClientImpl, DarwinError, ServiceDetails};
use crate::domain::Crs;

/// Board type: departures or arrivals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BoardType {
    Departures,
    Arrivals,
}

/// Cache key for station boards: (station CRS, date, time bucket, time window, board type).
/// Time bucket is minutes from midnight divided by bucket_mins.
/// Time window is included because the API returns different data for different windows.
/// Board type distinguishes arrivals from departures.
type BoardKey = (Crs, NaiveDate, u16, u16, BoardType);

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
            bucket_mins: 10,
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
    async fn get_board(&self, key: &BoardKey) -> Option<BoardEntry> {
        self.boards.get(key).await
    }

    /// Insert a board entry into the cache.
    async fn insert_board(&self, key: BoardKey, entry: BoardEntry) {
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
/// Wraps a `DarwinClientImpl` (real or mock) and caches departure board responses.
pub struct CachedDarwinClient {
    client: DarwinClientImpl,
    cache: DarwinCache,
}

impl CachedDarwinClient {
    /// Create a new cached client.
    pub fn new(client: DarwinClientImpl, cache_config: &CacheConfig) -> Self {
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
        let key = (*crs, date, bucket, time_window, BoardType::Departures);

        // Try cache first
        if let Some(cached) = self.cache.get_board(&key).await {
            return Ok(cached);
        }

        // Fetch from API
        let services = self
            .client
            .get_departures_with_details(crs, 150, time_offset, time_window, date)
            .await?;

        // Wrap in Arc for sharing
        let services: Vec<Arc<ConvertedService>> = services.into_iter().map(Arc::new).collect();
        let entry = Arc::new(services);

        // Cache and return
        self.cache.insert_board(key, entry.clone()).await;

        Ok(entry)
    }

    /// Get arrivals with details, using cache if available.
    ///
    /// Use this when the train is arriving at its terminus station.
    pub async fn get_arrivals_with_details(
        &self,
        crs: &Crs,
        date: NaiveDate,
        current_mins: u16,
        time_offset: i16,
        time_window: u16,
    ) -> Result<Arc<Vec<Arc<ConvertedService>>>, DarwinError> {
        let bucket = self.cache.time_bucket(time_offset, current_mins);
        let key = (*crs, date, bucket, time_window, BoardType::Arrivals);

        // Try cache first
        if let Some(cached) = self.cache.get_board(&key).await {
            return Ok(cached);
        }

        // Fetch from API
        let services = self
            .client
            .get_arrivals_with_details(crs, 150, time_offset, time_window, date)
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
    pub fn client(&self) -> &DarwinClientImpl {
        &self.client
    }

    /// Get full service details by service ID.
    ///
    /// This is not cached because it's a per-service lookup that's only needed
    /// for arrivals-only services (set-down-only trains).
    pub async fn get_service_details(
        &self,
        service_id: &str,
    ) -> Result<ServiceDetails, DarwinError> {
        self.client.get_service_details(service_id).await
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

        // 10:00 = 600 mins, bucket size 10 → bucket 60
        assert_eq!(cache.time_bucket(0, 600), 60);

        // 10:04 = 604 mins → bucket 60
        assert_eq!(cache.time_bucket(0, 604), 60);

        // 10:05 = 605 mins → bucket 60 (same bucket with 10-min buckets)
        assert_eq!(cache.time_bucket(0, 605), 60);

        // With offset: current 10:00, offset -30 → 9:30 = 570 mins → bucket 57
        assert_eq!(cache.time_bucket(-30, 600), 57);

        // Wrap around midnight: current 0:10 = 10 mins, offset -20 → 23:50 = 1430 mins
        // 1430 / 10 = 143
        assert_eq!(cache.time_bucket(-20, 10), 143);
    }

    #[test]
    fn default_config() {
        let config = CacheConfig::default();
        assert_eq!(config.ttl, Duration::from_secs(60));
        assert_eq!(config.max_capacity, 1000);
        assert_eq!(config.bucket_mins, 10);
    }

    #[test]
    fn cache_creation() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);
        assert_eq!(cache.entry_count(), 0);
    }
}

/// Tests for fixed cache behavior.
#[cfg(test)]
mod fixed_behavior_tests {
    use super::*;

    /// FIXED: Cache key now includes time_window parameter.
    ///
    /// Two requests with the same (station, date, time_bucket) but different
    /// time_window values now use different cache entries.
    #[test]
    fn cache_key_includes_time_window() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);

        // Two different time windows should produce different cache keys
        let crs = Crs::parse("PAD").unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2024, 3, 15).unwrap();
        let current_mins: u16 = 600; // 10:00

        let bucket = cache.time_bucket(0, current_mins);

        // Keys now include time_window as fourth element and board type as fifth
        let key_30: BoardKey = (crs, date, bucket, 30, BoardType::Departures);
        let key_120: BoardKey = (crs, date, bucket, 120, BoardType::Departures);

        // Keys are now different because time_window differs
        assert_ne!(
            key_30, key_120,
            "Cache keys should differ based on time_window"
        );
    }

    /// FIXED: With 10-minute buckets, nearby times share cache.
    ///
    /// Requests at 10:04 and 10:05 now fall in the same bucket, allowing
    /// effective cache sharing for overlapping time windows.
    #[test]
    fn nearby_times_share_bucket() {
        let config = CacheConfig::default();
        let cache = DarwinCache::new(&config);

        // At 10:04, bucket = 604 / 10 = 60
        let bucket_10_04 = cache.time_bucket(0, 604);

        // At 10:05, bucket = 605 / 10 = 60
        let bucket_10_05 = cache.time_bucket(0, 605);

        // With 10-minute buckets, both fall in the same bucket
        assert_eq!(
            bucket_10_04, bucket_10_05,
            "Nearby times should share cache bucket with 10-minute buckets"
        );
    }
}
