//! Domain types for the train journey planner.
//!
//! This module contains the core domain model types that represent
//! validated rail data. All types enforce their invariants at construction
//! time, so code that receives these types can trust their validity.

mod headcode;
mod operator;
mod service_uid;
mod station;
mod time;

pub use headcode::Headcode;
pub use operator::{AtocCode, InvalidAtocCode};
pub use service_uid::{InvalidServiceUid, ServiceUid};
pub use station::{Crs, InvalidCrs};
pub use time::{RailTime, TimeError, parse_time_sequence, parse_time_sequence_reverse};
