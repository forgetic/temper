//! Portable Forge domain model types.
//!
//! Types are grouped by domain responsibility:
//!
//! - [`access`]: shared provisioning vocabulary (access scopes, repo
//!   permissions, webhook event selection).
//! - [`common`]: shared entities and the optimistic-concurrency [`Version`]
//!   token (users, repositories, labels, comments, branch refs).
//! - [`issue`]: issue artifacts and their create/update inputs.
//! - [`pull_request`]: pull-request artifacts, reviews, merges, and inputs.
//! - [`ci`]: CI job artifacts and status enums.
//! - [`ci_retry`]: exact-attempt CI retry fencing and outcomes.
//!
//! All public types are re-exported here so the module flattens to a single
//! `temper_forge_model::model` namespace.

pub mod access;
pub mod ci;
pub mod ci_retry;
pub mod common;
pub mod issue;
pub mod pull_request;

pub use access::*;
pub use ci::*;
pub use ci_retry::*;
pub use common::*;
pub use issue::*;
pub use pull_request::*;
