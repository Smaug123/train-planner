//! Disk-based cache for station data.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use super::client::StationDto;
use super::error::StationError;

/// Default cache TTL: 24 hours.
const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Cached station data with metadata.
#[derive(Debug, Serialize, Deserialize)]
struct CachedStations {
    /// Unix timestamp when the cache was written.
    cached_at_secs: u64,
    /// The cached station data.
    stations: Vec<StationDto>,
}

/// Configuration for the station disk cache.
#[derive(Debug, Clone)]
pub struct StationCacheConfig {
    /// Path to the cache file.
    pub path: PathBuf,
    /// How long the cache remains valid.
    pub ttl: Duration,
}

impl StationCacheConfig {
    /// Create a new cache config with the given path and default TTL (24 hours).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ttl: DEFAULT_TTL,
        }
    }

    /// Set a custom TTL.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }
}

impl Default for StationCacheConfig {
    fn default() -> Self {
        // Default to a cache file in the current directory
        Self::new("stations_cache.json")
    }
}

/// Disk cache for station data.
#[derive(Debug, Clone)]
pub struct StationCache {
    config: StationCacheConfig,
}

impl StationCache {
    /// Create a new station cache with the given config.
    pub fn new(config: StationCacheConfig) -> Self {
        Self { config }
    }

    /// Try to load stations from the cache.
    ///
    /// Returns `None` if the cache doesn't exist, is invalid, or has expired.
    pub fn load(&self) -> Option<Vec<StationDto>> {
        let contents = std::fs::read_to_string(&self.config.path).ok()?;
        let cached: CachedStations = serde_json::from_str(&contents).ok()?;

        // Check if cache has expired
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_secs();

        let age_secs = now.saturating_sub(cached.cached_at_secs);
        if age_secs >= self.config.ttl.as_secs() {
            return None;
        }

        Some(cached.stations)
    }

    /// Save stations to the cache.
    ///
    /// Creates parent directories if they don't exist.
    pub fn save(&self, stations: &[StationDto]) -> Result<(), StationError> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| StationError::Cache {
                message: "system time before unix epoch".to_string(),
            })?
            .as_secs();

        let cached = CachedStations {
            cached_at_secs: now,
            stations: stations.to_vec(),
        };

        // Create parent directories if needed
        if let Some(parent) = self.config.path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| StationError::Cache {
                message: format!("failed to create cache directory: {}", e),
            })?;
        }

        let json = serde_json::to_string_pretty(&cached).map_err(|e| StationError::Cache {
            message: format!("failed to serialize cache: {}", e),
        })?;

        std::fs::write(&self.config.path, json).map_err(|e| StationError::Cache {
            message: format!("failed to write cache file: {}", e),
        })?;

        Ok(())
    }

    /// Get the cache file path.
    pub fn path(&self) -> &Path {
        &self.config.path
    }

    /// Get the cache TTL.
    pub fn ttl(&self) -> Duration {
        self.config.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn save_and_load_cache() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("stations.json");
        let config = StationCacheConfig::new(&cache_path);
        let cache = StationCache::new(config);

        let stations = vec![
            StationDto {
                crs_code: "KGX".to_string(),
                name: "London Kings Cross".to_string(),
            },
            StationDto {
                crs_code: "PAD".to_string(),
                name: "London Paddington".to_string(),
            },
        ];

        cache.save(&stations).unwrap();

        let loaded = cache.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].crs_code, "KGX");
        assert_eq!(loaded[1].crs_code, "PAD");
    }

    #[test]
    fn expired_cache_returns_none() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("stations.json");
        let config = StationCacheConfig::new(&cache_path).with_ttl(Duration::from_secs(0));
        let cache = StationCache::new(config);

        let stations = vec![StationDto {
            crs_code: "KGX".to_string(),
            name: "London Kings Cross".to_string(),
        }];

        cache.save(&stations).unwrap();

        // With 0 TTL, cache should immediately be expired
        assert!(cache.load().is_none());
    }

    #[test]
    fn missing_cache_returns_none() {
        let config = StationCacheConfig::new("/nonexistent/path/stations.json");
        let cache = StationCache::new(config);

        assert!(cache.load().is_none());
    }

    #[test]
    fn creates_parent_directories() {
        let dir = tempdir().unwrap();
        let cache_path = dir.path().join("nested").join("dir").join("stations.json");
        let config = StationCacheConfig::new(&cache_path);
        let cache = StationCache::new(config);

        let stations = vec![StationDto {
            crs_code: "KGX".to_string(),
            name: "London Kings Cross".to_string(),
        }];

        cache.save(&stations).unwrap();
        assert!(cache_path.exists());
    }
}
