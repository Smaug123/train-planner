//! Darwin LDB HTTP client.
//!
//! Provides async methods for querying the Darwin Live Departure Boards API.
//! Handles authentication, rate limiting, and conversion to domain types.

use std::sync::Arc;

use chrono::NaiveDate;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::Semaphore;

use crate::domain::Crs;

use super::convert::{ConvertedService, convert_station_board};
use super::error::DarwinError;
use super::types::{ServiceDetails, StationBoardWithDetails};

/// Default base URL for Darwin LDB API.
const DEFAULT_BASE_URL: &str =
    "https://api1.raildata.org.uk/1010-live-departure-board-dep-with-details/LDBWS";

/// Default maximum concurrent requests.
const DEFAULT_MAX_CONCURRENT: usize = 5;

/// Configuration for the Darwin client.
#[derive(Debug, Clone)]
pub struct DarwinConfig {
    /// API key for authentication
    pub api_key: String,
    /// Base URL for the API (defaults to production Darwin)
    pub base_url: String,
    /// Maximum concurrent requests
    pub max_concurrent: usize,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl DarwinConfig {
    /// Create a new config with the given API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            timeout_secs: 30,
        }
    }

    /// Set a custom base URL (for testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Set maximum concurrent requests.
    pub fn with_max_concurrent(mut self, n: usize) -> Self {
        self.max_concurrent = n;
        self
    }

    /// Set request timeout.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// Darwin LDB API client.
///
/// Provides methods for querying departure boards and service details.
/// Uses a semaphore to limit concurrent requests and avoid rate limiting.
#[derive(Debug, Clone)]
pub struct DarwinClient {
    http: reqwest::Client,
    base_url: String,
    semaphore: Arc<Semaphore>,
}

impl DarwinClient {
    /// Create a new Darwin client with the given configuration.
    pub fn new(config: DarwinConfig) -> Result<Self, DarwinError> {
        let mut headers = HeaderMap::new();

        // Set the API key header
        // Darwin uses "x-apikey" for authentication
        let api_key =
            HeaderValue::from_str(&config.api_key).map_err(|_| DarwinError::ApiError {
                status: 0,
                message: "Invalid API key format".to_string(),
            })?;
        headers.insert("x-apikey", api_key);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            http,
            base_url: config.base_url,
            semaphore: Arc::new(Semaphore::new(config.max_concurrent)),
        })
    }

    /// Get departure board with details for a station.
    ///
    /// Returns services with their calling points already included.
    /// This is the most efficient way to get service information since
    /// it avoids needing separate GetServiceDetails calls.
    ///
    /// # Arguments
    ///
    /// * `crs` - Station CRS code
    /// * `num_rows` - Number of services to return (max 150)
    /// * `time_offset` - Minutes offset from now (-120 to 120)
    /// * `time_window` - Minutes window for results (0 to 120)
    /// * `board_date` - Date to use for parsing times
    pub async fn get_departures_with_details(
        &self,
        crs: &Crs,
        num_rows: u8,
        time_offset: i16,
        time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DarwinError::ApiError {
                status: 0,
                message: "Semaphore closed".to_string(),
            })?;

        let url = format!(
            "{}/api/20220120/GetDepBoardWithDetails/{}",
            self.base_url,
            crs.as_str()
        );

        let response = self
            .http
            .get(&url)
            .query(&[
                ("numRows", num_rows.to_string()),
                ("timeOffset", time_offset.to_string()),
                ("timeWindow", time_window.to_string()),
            ])
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DarwinError::Unauthorized);
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(DarwinError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DarwinError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await?;

        let board: StationBoardWithDetails =
            serde_json::from_str(&body).map_err(|e| DarwinError::Json {
                message: e.to_string(),
                body: Some(body.chars().take(500).collect()),
            })?;

        convert_station_board(&board, board_date).map_err(|e| DarwinError::Json {
            message: e.to_string(),
            body: None,
        })
    }

    /// Get departure board with details, filtered to services calling at a destination.
    ///
    /// # Arguments
    ///
    /// * `crs` - Origin station CRS code
    /// * `filter_crs` - Destination station CRS code to filter by
    /// * `num_rows` - Number of services to return
    /// * `time_offset` - Minutes offset from now
    /// * `time_window` - Minutes window for results
    /// * `board_date` - Date to use for parsing times
    pub async fn get_departures_to(
        &self,
        crs: &Crs,
        filter_crs: &Crs,
        num_rows: u8,
        time_offset: i16,
        time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DarwinError::ApiError {
                status: 0,
                message: "Semaphore closed".to_string(),
            })?;

        let url = format!(
            "{}/api/20220120/GetDepBoardWithDetails/{}",
            self.base_url,
            crs.as_str()
        );

        let response = self
            .http
            .get(&url)
            .query(&[
                ("numRows", num_rows.to_string()),
                ("timeOffset", time_offset.to_string()),
                ("timeWindow", time_window.to_string()),
                ("filterCrs", filter_crs.as_str().to_string()),
                ("filterType", "to".to_string()),
            ])
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DarwinError::Unauthorized);
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(DarwinError::RateLimited);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DarwinError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await?;

        let board: StationBoardWithDetails =
            serde_json::from_str(&body).map_err(|e| DarwinError::Json {
                message: e.to_string(),
                body: Some(body.chars().take(500).collect()),
            })?;

        convert_station_board(&board, board_date).map_err(|e| DarwinError::Json {
            message: e.to_string(),
            body: None,
        })
    }

    /// Get service details by ID.
    ///
    /// **Important:** Darwin service IDs are ephemeral and only valid while
    /// the service appears on a departure board (~2 minutes after expected
    /// departure). This method may return `ServiceNotFound` if the ID has
    /// expired.
    ///
    /// For most use cases, prefer `get_departures_with_details` which includes
    /// calling points inline, avoiding the need for separate detail requests.
    pub async fn get_service_details(
        &self,
        service_id: &str,
    ) -> Result<ServiceDetails, DarwinError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DarwinError::ApiError {
                status: 0,
                message: "Semaphore closed".to_string(),
            })?;

        let url = format!(
            "{}/api/20220120/GetServiceDetails/{}",
            self.base_url, service_id
        );

        let response = self.http.get(&url).send().await?;

        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(DarwinError::Unauthorized);
        }

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(DarwinError::RateLimited);
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(DarwinError::ServiceNotFound);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DarwinError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await?;

        // Darwin returns null/empty for expired service IDs
        if body.is_empty() || body == "null" {
            return Err(DarwinError::ServiceNotFound);
        }

        serde_json::from_str(&body).map_err(|e| DarwinError::Json {
            message: e.to_string(),
            body: Some(body.chars().take(500).collect()),
        })
    }

    /// Get the raw departure board response (for debugging/testing).
    pub async fn get_departures_raw(
        &self,
        crs: &Crs,
        num_rows: u8,
    ) -> Result<StationBoardWithDetails, DarwinError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DarwinError::ApiError {
                status: 0,
                message: "Semaphore closed".to_string(),
            })?;

        let url = format!(
            "{}/api/20220120/GetDepBoardWithDetails/{}",
            self.base_url,
            crs.as_str()
        );

        let response = self
            .http
            .get(&url)
            .query(&[("numRows", num_rows.to_string())])
            .send()
            .await?;

        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(DarwinError::ApiError {
                status: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await?;

        serde_json::from_str(&body).map_err(|e| DarwinError::Json {
            message: e.to_string(),
            body: Some(body.chars().take(500).collect()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_builder() {
        let config = DarwinConfig::new("test-key")
            .with_base_url("http://localhost:8080")
            .with_max_concurrent(10)
            .with_timeout(60);

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.base_url, "http://localhost:8080");
        assert_eq!(config.max_concurrent, 10);
        assert_eq!(config.timeout_secs, 60);
    }

    #[test]
    fn config_defaults() {
        let config = DarwinConfig::new("test-key");

        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.max_concurrent, DEFAULT_MAX_CONCURRENT);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn client_creation() {
        let config = DarwinConfig::new("test-key");
        let client = DarwinClient::new(config);
        assert!(client.is_ok());
    }

    // Integration tests would go here, but require a real API key
    // and would make actual HTTP requests. They should be marked
    // with #[ignore] and run separately.
}
