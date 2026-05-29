//! Backend-agnostic Forge domain model and interface for Harness.
//!
//! This crate intentionally contains no concrete backend logic. Backends
//! implement [`Forge`] using the portable types in [`model`] and [`ids`].

pub mod forge;
pub mod ids;
pub mod model;

pub use forge::{CiJobQuery, Forge, ForgeError, ForgeResult, IssueQuery, PullRequestQuery};
pub use ids::*;
pub use model::*;
