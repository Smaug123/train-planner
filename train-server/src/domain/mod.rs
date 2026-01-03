//! Domain types for the train journey planner.
//!
//! This module contains the core domain model types that represent
//! validated rail data. All types enforce their invariants at construction
//! time, so code that receives these types can trust their validity.

mod call;
mod error;
mod headcode;
mod identify;
mod journey;
mod leg;
mod operator;
mod service;
mod service_uid;
mod station;
mod time;

pub use call::{Call, CallIndex};
pub use error::DomainError;
pub use headcode::Headcode;
pub use identify::{IdentifyTrainRequest, MatchConfidence};
pub use journey::{Journey, Segment, Walk};
pub use leg::Leg;
pub use operator::{AtocCode, InvalidAtocCode};
pub use service::{Service, ServiceCandidate, ServiceRef};
pub use service_uid::{InvalidServiceUid, ServiceUid};
pub use station::{Crs, InvalidCrs};
pub use time::{RailTime, TimeError, parse_time_sequence, parse_time_sequence_reverse};
