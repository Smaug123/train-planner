//! Web layer for the train journey planner.
//!
//! Provides HTTP endpoints for searching services and planning journeys.

mod dto;
mod routes;
mod state;

pub use dto::*;
pub use routes::create_router;
pub use state::AppState;
