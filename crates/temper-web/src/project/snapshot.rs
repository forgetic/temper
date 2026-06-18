// SPDX-License-Identifier: MPL-2.0

//! Project the daemon's raw `GET /v1/state` snapshot into the board's cold-start
//! envelope.
//!
//! The daemon snapshot (PR C, `DaemonStateSnapshot`) is the lowest-level honest
//! view of dispatch state: a worker tile, a queued list, an in-flight list, and
//! per-role saturation. It carries no lanes, titles, or `enteredAt` — those are
//! this projection's job (UX Appendix A.1):
//!
//! - each queued / in-flight job → one board card, keyed by `artifact.ref`;
//! - `lane` from the workflow lifecycle dimension when available, else the
//!   queued/in-flight split ([`LaneMap::default_lane`]);
//! - `title` defaults to the `artifact.ref` until a richer forge-backed title
//!   feed lands (out of scope here);
//! - `role_saturation` waitlists become attention problems (overlay, not lane).
//!
//! We deserialize the daemon shape into our OWN [`RawDaemonSnapshot`] structs so
//! temper-web stays a depgraph leaf (no dependency on `temper-engine`); the
//! shapes are pinned to the daemon DTOs by a serialization contract test.

use serde::Deserialize;

use crate::board::{Card, Lane, Problem, Sev, SnapshotState, Workers};
use crate::project::lanes::LaneMap;

/// The raw daemon `GET /v1/state` body, mirroring `temper-engine`'s
/// `DaemonStateSnapshot` wire shape (PR C). Owned here so temper-web does not
/// depend on the engine crate; kept honest by a contract test.
#[derive(Debug, Clone, Deserialize)]
pub struct RawDaemonSnapshot {
    #[serde(default)]
    pub workers: RawWorkers,
    #[serde(default)]
    pub queued: Vec<RawJob>,
    #[serde(default)]
    pub in_flight: Vec<RawJob>,
    #[serde(default)]
    pub role_saturation: Vec<RawRoleSaturation>,
}

/// Worker tile of the raw snapshot.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawWorkers {
    pub healthy: usize,
    pub total: usize,
}

/// One queued or in-flight dispatch job in the raw snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct RawJob {
    pub job_id: String,
    pub role: String,
    #[serde(default)]
    pub repo: String,
    /// Canonical `artifact.ref` join key, or `null` for a non-numeric artifact.
    #[serde(rename = "ref")]
    pub artifact_ref: Option<String>,
}

/// A saturated role's waitlist in the raw snapshot.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawRoleSaturation {
    pub role: String,
    #[serde(default)]
    pub concurrency: usize,
    #[serde(default)]
    pub waiting: Vec<String>,
}

/// Project the raw daemon snapshot into the board's cold-start [`SnapshotState`].
///
/// `now` seeds each card's `enteredAt` (the daemon has no per-card timestamp;
/// using the snapshot time is the honest default and matches the board's
/// age-in-stage start). `lanes` derives each card's lane.
#[must_use]
pub fn project_snapshot(raw: &RawDaemonSnapshot, lanes: &LaneMap, now: i64) -> SnapshotState {
    let mut cards = std::collections::BTreeMap::new();

    for (job, in_flight) in raw
        .queued
        .iter()
        .map(|job| (job, false))
        .chain(raw.in_flight.iter().map(|job| (job, true)))
    {
        let Some(card) = project_card(job, lanes, in_flight, now) else {
            continue;
        };
        cards.insert(card.id.clone(), card);
    }

    let mut problems = std::collections::BTreeMap::new();
    for saturation in &raw.role_saturation {
        for waiting_ref in &saturation.waiting {
            let id = format!("sat:{}:{}", saturation.role, waiting_ref);
            problems.insert(
                id,
                Problem {
                    sev: Sev::Warn,
                    msg: format!(
                        "{} saturated ({} in flight) — {} waiting",
                        saturation.role, saturation.concurrency, waiting_ref
                    ),
                    card: card_id_for_ref(waiting_ref),
                    since: now,
                },
            );
        }
    }

    SnapshotState {
        workers: Workers {
            healthy: raw.workers.healthy,
            total: raw.workers.total,
        },
        cards,
        problems,
    }
}

/// Project one raw job into a board card, or `None` when it lacks the
/// `artifact.ref` join key (non-numeric artifact) and so cannot be addressed by
/// the log-tail deltas the board joins on.
fn project_card(job: &RawJob, lanes: &LaneMap, in_flight: bool, now: i64) -> Option<Card> {
    let artifact_ref = job.artifact_ref.clone()?;
    let lane = if lanes.has_lifecycle() {
        // No lifecycle state is known at cold start (the daemon snapshot carries
        // no labels); seed the queued/in-flight split and let log deltas refine.
        default_split_lane(in_flight)
    } else {
        lanes.default_lane(in_flight)
    };
    Some(Card {
        id: card_id_for_ref(&artifact_ref),
        lane,
        artifact_ref: artifact_ref.clone(),
        role: job.role.clone(),
        title: artifact_ref,
        entered_at: now,
        steps: None,
        activity: None,
        ci: None,
        merged: None,
    })
}

fn default_split_lane(in_flight: bool) -> Lane {
    if in_flight {
        Lane::Implement
    } else {
        Lane::Triage
    }
}

/// Derive a stable, dom-safe card id from an `artifact.ref` (`owner/repo#42` /
/// `owner/repo PR#8`). The board keys cards by this id and the log-tail adapter
/// joins deltas on the same derivation, so the two agree.
#[must_use]
pub fn card_id_for_ref(artifact_ref: &str) -> String {
    let mut id = String::with_capacity(artifact_ref.len());
    for ch in artifact_ref.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' => id.push(ch),
            '#' => id.push('-'),
            _ => id.push('_'),
        }
    }
    id
}

#[cfg(test)]
#[path = "snapshot_tests.rs"]
mod snapshot_tests;
