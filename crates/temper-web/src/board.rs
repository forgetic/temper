// SPDX-License-Identifier: MPL-2.0

//! Board wire types — the Rust side of the cross-language contract with the
//! TypeScript board client (`crates/temper-web/ui/src/model.ts`).
//!
//! These types serialize to exactly the JSON the TS `apply()` reducer consumes:
//! the `{t:"snapshot",seq,state:{workers,cards,problems}}` cold-start envelope
//! and the per-delta [`BoardEvent`] union (`card.move`, `problem.add`, …), each
//! feed event carrying a monotonic `seq` cursor.
//!
//! The serialized field names mirror `model.ts` verbatim (camelCase where the TS
//! interface uses it, e.g. `enteredAt`); the Rust-side serialization tests and
//! the shared `ui/fixtures/*.json` keep the two sides from drifting.

use serde::{Deserialize, Serialize};

/// A board lane. The set is fixed by the client (`model.ts` `Lane`); the server
/// projects a workflow's exclusive lifecycle state dimension onto these columns
/// (see [`crate::project::lanes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Triage,
    Implement,
    Review,
    Ci,
    Done,
}

impl Lane {
    /// The lane's wire token, matching the TS `Lane` union.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Triage => "triage",
            Lane::Implement => "implement",
            Lane::Review => "review",
            Lane::Ci => "ci",
            Lane::Done => "done",
        }
    }
}

/// A live activity hint shown on an in-flight card (`model.ts` `Activity`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub kind: ActivityKind,
    pub text: String,
}

/// `think` vs `tool` activity (`model.ts` `Activity.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    Think,
    Tool,
}

/// Step progress affordance (`▓▓▓░ 3/4`); `model.ts` `Card.steps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Steps {
    pub done: u32,
    pub total: u32,
}

/// CI status badge on a card (`model.ts` `Card.ci`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiStatus {
    Running,
    Failed,
}

/// One pipeline card — an artifact (issue/PR) flowing across lanes. Mirrors
/// `model.ts` `Card`; `enteredAt` is epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub lane: Lane,
    #[serde(rename = "ref")]
    pub artifact_ref: String,
    pub role: String,
    pub title: String,
    #[serde(rename = "enteredAt")]
    pub entered_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps: Option<Steps>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<Activity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
}

/// Problem severity (`model.ts` `Sev`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sev {
    Warn,
    Bad,
}

/// One problem-ticker row (`model.ts` `Problem`). `card` is the card id it
/// attaches to; `since` is epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub sev: Sev,
    pub msg: String,
    pub card: String,
    pub since: i64,
}

/// Worker health tile (`model.ts` `State.workers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Workers {
    pub healthy: usize,
    pub total: usize,
}

/// The cold-start state payload inside a [`BoardEvent::Snapshot`] — the board's
/// rebuildable projection (`model.ts` snapshot `state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SnapshotState {
    pub workers: Workers,
    pub cards: std::collections::BTreeMap<String, Card>,
    pub problems: std::collections::BTreeMap<String, Problem>,
}

/// The board feed event union — the `data:` JSON payload of an SSE message,
/// mirroring `model.ts` `Event` (the server-emitted variants only; the TS
/// `open`/`close`/`tick` variants are client-side and never sent over the wire).
///
/// Tagged on `t`; every variant except the snapshot carries a monotonic `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum BoardEvent {
    /// Cold-start snapshot: `{t:"snapshot",seq,state:{…}}`.
    #[serde(rename = "snapshot")]
    Snapshot { seq: u64, state: SnapshotState },
    /// A card moved to a new lane; `now` resets the card's age.
    #[serde(rename = "card.move")]
    CardMove {
        seq: u64,
        id: String,
        lane: Lane,
        now: i64,
    },
    /// A card's live activity hint changed.
    #[serde(rename = "card.activity")]
    CardActivity {
        seq: u64,
        id: String,
        activity: Activity,
    },
    /// A card's step progress changed.
    #[serde(rename = "card.step")]
    CardStep { seq: u64, id: String, steps: Steps },
    /// A problem row appeared/updated, keyed by `id`.
    #[serde(rename = "problem.add")]
    ProblemAdd {
        seq: u64,
        id: String,
        problem: Problem,
    },
    /// A problem row cleared, keyed by `id`.
    #[serde(rename = "problem.clear")]
    ProblemClear { seq: u64, id: String },
}

impl BoardEvent {
    /// The event's sequence cursor (every server-emitted variant carries one).
    #[must_use]
    pub fn seq(&self) -> u64 {
        match self {
            BoardEvent::Snapshot { seq, .. }
            | BoardEvent::CardMove { seq, .. }
            | BoardEvent::CardActivity { seq, .. }
            | BoardEvent::CardStep { seq, .. }
            | BoardEvent::ProblemAdd { seq, .. }
            | BoardEvent::ProblemClear { seq, .. } => *seq,
        }
    }
}

#[cfg(test)]
#[path = "board_tests.rs"]
mod board_tests;
