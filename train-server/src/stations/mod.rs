//! National Rail Station API client and name lookup.
//!
//! Provides CRS code → station name mapping, fetched from the
//! National Rail Station API at startup and refreshed daily.

mod client;
mod error;
mod names;

pub use client::{StationClient, StationClientConfig};
pub use error::StationError;
pub use names::StationNames;
