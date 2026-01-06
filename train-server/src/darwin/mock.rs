//! Mock Darwin client for testing without API access.
//!
//! Loads sample departure boards from JSON files and serves them
//! as if they were live API responses.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use chrono::NaiveDate;
use tokio::sync::RwLock;

use crate::domain::Crs;

use super::convert::{ConvertedService, convert_station_board};
use super::error::DarwinError;
use super::types::StationBoardWithDetails;

/// Mock Darwin client that serves data from JSON files.
///
/// This is useful for development and testing without needing real Darwin API credentials.
#[derive(Clone)]
pub struct MockDarwinClient {
    /// Pre-loaded station boards, keyed by CRS.
    boards: Arc<RwLock<HashMap<Crs, StationBoardWithDetails>>>,
}

impl MockDarwinClient {
    /// Create a new mock client by loading JSON files from a directory.
    ///
    /// Expects files named `{CRS}.json` (e.g., `PAD.json`, `KGX.json`).
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self, DarwinError> {
        let data_dir = data_dir.as_ref();
        let mut boards = HashMap::new();

        // Read all .json files in the directory
        let entries = std::fs::read_dir(data_dir).map_err(|e| DarwinError::ApiError {
            status: 0,
            message: format!("Failed to read mock data directory: {}", e),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| DarwinError::ApiError {
                status: 0,
                message: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            // Extract CRS from filename (e.g., "PAD.json" -> "PAD")
            let crs_str =
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| DarwinError::ApiError {
                        status: 0,
                        message: format!("Invalid filename: {:?}", path),
                    })?;

            let crs = Crs::parse(crs_str).map_err(|_| DarwinError::ApiError {
                status: 0,
                message: format!("Invalid CRS in filename: {}", crs_str),
            })?;

            // Load and parse the JSON file
            let json = std::fs::read_to_string(&path).map_err(|e| DarwinError::ApiError {
                status: 0,
                message: format!("Failed to read {:?}: {}", path, e),
            })?;

            let board: StationBoardWithDetails =
                serde_json::from_str(&json).map_err(|e| DarwinError::ApiError {
                    status: 0,
                    message: format!("Failed to parse {:?}: {}", path, e),
                })?;

            boards.insert(crs, board);
        }

        if boards.is_empty() {
            return Err(DarwinError::ApiError {
                status: 0,
                message: format!("No mock board files found in {:?}", data_dir),
            });
        }

        Ok(Self {
            boards: Arc::new(RwLock::new(boards)),
        })
    }

    /// Get departure board with details for a station.
    ///
    /// Mimics the real `DarwinClient::get_departures_with_details` interface.
    /// Time parameters are ignored - mock data is static.
    pub async fn get_departures_with_details(
        &self,
        crs: &Crs,
        _num_rows: u8,
        _time_offset: i16,
        _time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        let boards = self.boards.read().await;

        let board = boards.get(crs).ok_or_else(|| DarwinError::ApiError {
            status: 404,
            message: format!(
                "No mock data for station {}. Available: {:?}",
                crs.as_str(),
                boards.keys().map(|c| c.as_str()).collect::<Vec<_>>()
            ),
        })?;

        // Convert the station board to domain types
        convert_station_board(board, board_date).map_err(|e| DarwinError::ApiError {
            status: 500,
            message: format!("Failed to convert mock board data: {}", e),
        })
    }

    /// Get arrival board with details for a station.
    ///
    /// Mimics the real `DarwinClient::get_arrivals_with_details` interface.
    /// For mock purposes, returns the same data as departures (JSON structure is identical).
    pub async fn get_arrivals_with_details(
        &self,
        crs: &Crs,
        _num_rows: u8,
        _time_offset: i16,
        _time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        // Arrivals use the same JSON structure as departures, just with sta/eta instead of std/etd.
        // For mock purposes, we reuse the same data.
        let boards = self.boards.read().await;

        let board = boards.get(crs).ok_or_else(|| DarwinError::ApiError {
            status: 404,
            message: format!(
                "No mock data for station {}. Available: {:?}",
                crs.as_str(),
                boards.keys().map(|c| c.as_str()).collect::<Vec<_>>()
            ),
        })?;

        convert_station_board(board, board_date).map_err(|e| DarwinError::ApiError {
            status: 500,
            message: format!("Failed to convert mock board data: {}", e),
        })
    }

    /// List available stations in the mock data.
    pub async fn available_stations(&self) -> Vec<Crs> {
        let boards = self.boards.read().await;
        boards.keys().copied().collect()
    }

    /// Reload mock data from disk (useful for development).
    pub async fn reload(&self, data_dir: impl AsRef<Path>) -> Result<(), DarwinError> {
        let new_client = Self::new(data_dir)?;
        let mut boards = self.boards.write().await;
        let new_boards = new_client.boards.read().await;
        *boards = new_boards.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn load_mock_data() {
        let client = MockDarwinClient::new("data/mock_boards").unwrap();
        let stations = client.available_stations().await;

        // Should have loaded at least PAD, RDG, BRI
        assert!(stations.contains(&Crs::parse("PAD").unwrap()));
    }

    #[tokio::test]
    async fn get_departures() {
        let client = MockDarwinClient::new("data/mock_boards").unwrap();
        let crs = Crs::parse("PAD").unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 3).unwrap();

        let services = client
            .get_departures_with_details(&crs, 10, 0, 120, date)
            .await
            .unwrap();

        assert!(!services.is_empty());
        assert!(services[0].service.calls.len() > 1);
    }

    #[tokio::test]
    async fn unknown_station_returns_error() {
        let client = MockDarwinClient::new("data/mock_boards").unwrap();
        let crs = Crs::parse("XYZ").unwrap();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 3).unwrap();

        let result = client
            .get_departures_with_details(&crs, 10, 0, 120, date)
            .await;

        assert!(result.is_err());
    }
}
