//! National Rail Station API client.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;

use super::error::StationError;

/// Default base URL for the Station API (Rail Data Marketplace).
const DEFAULT_BASE_URL: &str = "https://api1.raildata.org.uk/1010-nationalrail-knowledgebase-stations-feed-_json_---production5_0";

/// Wrapper for the stations response.
#[derive(Debug, Deserialize)]
pub struct StationsResponse {
    pub stations: Vec<StationDto>,
}

/// Minimal DTO for station data - we only need CRS and name.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StationDto {
    pub crs_code: String,
    pub name: String,
}

/// Configuration for the Station API client.
#[derive(Debug, Clone)]
pub struct StationClientConfig {
    /// API key for x-apikey header authentication
    pub api_key: String,
    /// Base URL for the API
    pub base_url: String,
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl StationClientConfig {
    /// Create a new config with the given API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout_secs: 30,
        }
    }

    /// Set a custom base URL (for testing).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

/// Client for the National Rail Station API.
#[derive(Debug, Clone)]
pub struct StationClient {
    http: reqwest::Client,
    base_url: String,
}

impl StationClient {
    /// Create a new Station API client.
    pub fn new(config: StationClientConfig) -> Result<Self, StationError> {
        let mut headers = HeaderMap::new();

        // Use x-apikey header for Rail Data Marketplace authentication
        let api_key_header =
            HeaderValue::from_str(&config.api_key).map_err(|_| StationError::Api {
                status: 0,
                message: "Invalid API key format".to_string(),
            })?;
        headers.insert(HeaderName::from_static("x-apikey"), api_key_header);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        Ok(Self {
            http,
            base_url: config.base_url,
        })
    }

    /// Fetch all stations from the API.
    pub async fn fetch_all(&self) -> Result<Vec<StationDto>, StationError> {
        let url = format!("{}/stations", self.base_url);

        let response = self.http.get(&url).send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(StationError::Unauthorized);
        }

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StationError::Api {
                status: status.as_u16(),
                message: body,
            });
        }

        let body = response.text().await?;

        let response: StationsResponse =
            serde_json::from_str(&body).map_err(|e| StationError::Json {
                message: e.to_string(),
            })?;

        Ok(response.stations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = StationClientConfig::new("test-api-key");
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn config_with_base_url() {
        let config =
            StationClientConfig::new("test-api-key").with_base_url("http://localhost:8080");
        assert_eq!(config.base_url, "http://localhost:8080");
    }
}
