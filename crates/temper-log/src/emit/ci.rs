// SPDX-License-Identifier: MPL-2.0

//! CI completion event inputs.

use crate::WorkItemRef;

/// Inputs for [`super::emit_ci_completed`] (`trigger` / `ci.completed`).
#[derive(Clone, Debug)]
pub struct CiCompleted<'a> {
    /// The pull request whose CI finished.
    pub item: &'a WorkItemRef,
    /// CI conclusion token (e.g. `success`, `failure`).
    pub conclusion: &'a str,
    /// Wall-clock CI duration in milliseconds (numeric field; human-formatted).
    pub duration_ms: u64,
    /// Trigger provenance for coordinated CI wakes.
    pub trigger_source: Option<&'a str>,
    /// Delay between latest-job completion and detection, clamped at zero.
    pub detection_latency_ms: Option<u64>,
    /// Selected workflow queue when a role job was enqueued.
    pub queue: Option<&'a str>,
    /// Selected workflow role when a role job was enqueued.
    pub role: Option<&'a str>,
}

impl<'a> CiCompleted<'a> {
    /// Creates a completion event without coordinated-wake metadata.
    pub const fn new(item: &'a WorkItemRef, conclusion: &'a str, duration_ms: u64) -> Self {
        Self {
            item,
            conclusion,
            duration_ms,
            trigger_source: None,
            detection_latency_ms: None,
            queue: None,
            role: None,
        }
    }
}
