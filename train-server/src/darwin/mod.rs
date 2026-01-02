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

mod client;
mod convert;
mod error;
mod types;

pub use client::{DarwinClient, DarwinConfig};
pub use convert::{ConversionError, ConvertedService};
pub use error::DarwinError;
pub use types::{
    ArrayOfCallingPoints, CallingPoint, ServiceDetails, ServiceItemWithCallingPoints,
    ServiceLocation, StationBoardWithDetails,
};
