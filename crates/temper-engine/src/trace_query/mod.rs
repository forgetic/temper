// SPDX-License-Identifier: MPL-2.0

//! Authorized, finite query projection over the engine-owned trace journal.
//!
//! URI parsing and filesystem access deliberately live outside the daemon's
//! pure state machine. Every successful read revalidates the canonical JSONL
//! stream and any referenced blobs through the journal read API.

mod auth;
mod cursor;
mod model;
mod projection;
mod request;
mod service;

pub use model::{TraceEventPage, TraceRunCounts, TraceRunIdentity, TraceRunPage, TraceRunSummary};
pub(crate) use request::is_trace_uri;
pub(crate) use service::{TraceQueryService, disabled_trace_response};
pub use temper_protocol_activity::TraceExportRecordV1;

pub const AGENT_RUNS_PATH: &str = "/v1/agent-runs";
pub const DEFAULT_RUN_PAGE_LIMIT: usize = 50;
pub const MAX_RUN_PAGE_LIMIT: usize = 200;
pub const DEFAULT_EVENT_PAGE_LIMIT: usize = 500;
pub const MAX_EVENT_PAGE_LIMIT: usize = 1_000;
pub(super) const MAX_QUERY_VALUE_BYTES: usize = 1_024;

#[derive(Clone, Copy)]
pub(super) enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    Forbidden,
    NotFound,
    Unavailable,
}
