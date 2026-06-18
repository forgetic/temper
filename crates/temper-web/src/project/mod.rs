// SPDX-License-Identifier: MPL-2.0

//! Projection from raw backend data into the board's card model.
//!
//! - [`snapshot`] projects the daemon's raw `GET /v1/state` snapshot (the
//!   un-laned dispatch view from PR C) into the board's cold-start
//!   `{t:"snapshot",…}` envelope.
//! - [`lanes`] derives a card's board lane from the workflow's exclusive
//!   lifecycle state dimension (UX Appendix A.4).

pub mod lanes;
pub mod snapshot;

pub use lanes::LaneMap;
pub use snapshot::{RawDaemonSnapshot, project_snapshot};
