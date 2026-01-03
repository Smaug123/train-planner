//! Web layer for the train journey planner.
//!
//! Provides HTTP endpoints for searching services and planning journeys.

mod dto;
mod routes;
mod rtt;
mod state;
pub mod templates;

pub use dto::*;
pub use routes::create_router;
pub use state::AppState;
pub use templates::*;
