//! Station name lookup.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::domain::Crs;

use super::client::{StationClient, StationDto};
use super::error::StationError;

/// Thread-safe station name lookup.
///
/// Provides CRS → station name mapping with support for background refresh.
#[derive(Clone)]
pub struct StationNames {
    inner: Arc<RwLock<HashMap<Crs, String>>>,
    client: StationClient,
}

impl StationNames {
    /// Create a new StationNames by fetching from the API.
    ///
    /// This will fail if the API is unreachable.
    pub async fn fetch(client: StationClient) -> Result<Self, StationError> {
        let stations = client.fetch_all().await?;
        let map = build_map(stations);

        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
            client,
        })
    }

    /// Create an empty StationNames (for mock/test mode).
    ///
    /// This is useful when station name lookup is not needed.
    pub fn empty(client: StationClient) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            client,
        }
    }

    /// Look up a station name by CRS code.
    pub async fn get(&self, crs: &Crs) -> Option<String> {
        let guard = self.inner.read().await;
        guard.get(crs).cloned()
    }

    /// Get the number of stations in the lookup.
    pub async fn len(&self) -> usize {
        let guard = self.inner.read().await;
        guard.len()
    }

    /// Check if the lookup is empty.
    pub async fn is_empty(&self) -> bool {
        let guard = self.inner.read().await;
        guard.is_empty()
    }

    /// Refresh the station data from the API.
    ///
    /// On success, replaces the current mapping. On failure, the existing
    /// mapping is preserved and the error is returned.
    pub async fn refresh(&self) -> Result<usize, StationError> {
        let stations = self.client.fetch_all().await?;
        let map = build_map(stations);
        let count = map.len();

        let mut guard = self.inner.write().await;
        *guard = map;

        Ok(count)
    }
}

/// Build the CRS → name map from station DTOs.
fn build_map(stations: Vec<StationDto>) -> HashMap<Crs, String> {
    stations
        .into_iter()
        .filter_map(|s| {
            // The API returns lowercase CRS codes; convert to uppercase
            let crs_upper = s.crs_code.to_uppercase();
            Crs::parse(&crs_upper).ok().map(|crs| (crs, s.name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_map_filters_invalid_crs() {
        let stations = vec![
            StationDto {
                crs_code: "KGX".to_string(),
                name: "London Kings Cross".to_string(),
            },
            StationDto {
                crs_code: "invalid".to_string(),
                name: "Bad Station".to_string(),
            },
            StationDto {
                crs_code: "PAD".to_string(),
                name: "London Paddington".to_string(),
            },
        ];

        let map = build_map(stations);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&Crs::parse("KGX").unwrap()),
            Some(&"London Kings Cross".to_string())
        );
        assert_eq!(
            map.get(&Crs::parse("PAD").unwrap()),
            Some(&"London Paddington".to_string())
        );
    }

    #[test]
    fn build_map_handles_lowercase_crs() {
        let stations = vec![StationDto {
            crs_code: "kgx".to_string(),
            name: "London Kings Cross".to_string(),
        }];

        let map = build_map(stations);
        assert_eq!(map.len(), 1);
        assert!(map.contains_key(&Crs::parse("KGX").unwrap()));
    }
}
