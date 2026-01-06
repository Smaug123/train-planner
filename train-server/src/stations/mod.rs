//! National Rail Station API client and name lookup.
//!
//! Provides CRS code → station name mapping, fetched from the
//! National Rail Station API at startup and refreshed daily.
//!
//! Supports disk-based caching to avoid hitting the expensive
//! stations API on every server restart.

mod cache;
mod client;
mod error;
mod names;

pub use cache::{StationCache, StationCacheConfig};
pub use client::{StationClient, StationClientConfig};
pub use error::StationError;
pub use names::{StationMatch, StationNames};
