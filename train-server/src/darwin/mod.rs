//! Darwin LDB (Live Departure Boards) client.
//!
//! This module provides an HTTP client for the National Rail Darwin API,
//! which provides real-time train departure information.
//!
//! Key characteristics of Darwin:
//! - Service IDs are **ephemeral** - only valid while the service appears
//!   on a departure board (~2 minutes after expected departure)
//! - Times are in "HH:MM" format (UK local time)
//! - `GetDepBoardWithDetails` returns calling points inline, avoiding
//!   the need for separate service detail requests

use chrono::NaiveDate;

use crate::domain::Crs;

mod client;
mod convert;
mod error;
mod mock;
mod types;

pub use client::{DarwinClient, DarwinConfig};
pub use convert::{ConversionError, ConvertedService};
pub use error::DarwinError;
pub use mock::MockDarwinClient;
pub use types::{
    ArrayOfCallingPoints, CallingPoint, ServiceDetails, ServiceItemWithCallingPoints,
    ServiceLocation, StationBoardWithDetails,
};

/// Unified client that can be either real or mock.
///
/// This allows the app to switch between real API and mock data
/// via environment configuration.
#[derive(Clone)]
pub enum DarwinClientImpl {
    Real(DarwinClient),
    Mock(MockDarwinClient),
}

impl DarwinClientImpl {
    /// Get departure board with details for a station.
    pub async fn get_departures_with_details(
        &self,
        crs: &Crs,
        num_rows: u8,
        time_offset: i16,
        time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        match self {
            Self::Real(client) => {
                client
                    .get_departures_with_details(
                        crs,
                        num_rows,
                        time_offset,
                        time_window,
                        board_date,
                    )
                    .await
            }
            Self::Mock(client) => {
                client
                    .get_departures_with_details(
                        crs,
                        num_rows,
                        time_offset,
                        time_window,
                        board_date,
                    )
                    .await
            }
        }
    }

    /// Get arrival board with details for a station.
    pub async fn get_arrivals_with_details(
        &self,
        crs: &Crs,
        num_rows: u8,
        time_offset: i16,
        time_window: u16,
        board_date: NaiveDate,
    ) -> Result<Vec<ConvertedService>, DarwinError> {
        match self {
            Self::Real(client) => {
                client
                    .get_arrivals_with_details(crs, num_rows, time_offset, time_window, board_date)
                    .await
            }
            Self::Mock(client) => {
                client
                    .get_arrivals_with_details(crs, num_rows, time_offset, time_window, board_date)
                    .await
            }
        }
    }
}
