// SPDX-License-Identifier: MPL-2.0

//! Lane derivation (UX Appendix A.4).
//!
//! Board columns are projected from a workflow's **exclusive lifecycle**
//! `StateDimension` — its ordered `states` — **not** from queues. Queues
//! (especially `pr_changes_requested` / `pr_ci_failed` / `needs_owner`) are
//! attention OVERLAYS handled as problem badges elsewhere, never lanes.
//!
//! The board's lane vocabulary is fixed by the client (`model.ts` `Lane`:
//! `triage`, `implement`, `review`, `ci`, `done`). A user-defined workflow's
//! lifecycle states are arbitrary, so we map each ordered lifecycle state onto
//! one of those five board lanes by:
//!
//! 1. a **name heuristic** — the state id or its bound forge label is matched
//!    against keyword sets per lane (e.g. `triage`/`intake`/`untriaged` →
//!    `triage`, `code`/`progress`/`impl` → `implement`, …);
//! 2. a **positional fallback** when the name is unrecognised — the state's index
//!    within the ordered dimension is spread across the five lanes, so an
//!    unknown-but-ordered lifecycle still flows left→right.
//!
//! If a workflow exposes **no** exclusive lifecycle dimension we log it (rather
//! than guess silently, per A.4) and fall back to mapping the daemon's
//! queued/in-flight split onto `triage`/`implement`.

use temper_workflow::{StateId, ValidatedStateDimension, ValidatedWorkflow};

use crate::board::Lane;

/// A resolver from a workflow's exclusive lifecycle state to a board lane.
///
/// Built once per workflow (cheap; just clones the ordered state list). The
/// read-model then resolves a card's lane from the lifecycle `StateId` it learns
/// via `transition.applied` / `labels.delta` deltas, joined by `artifact.ref`.
#[derive(Debug, Clone)]
pub struct LaneMap {
    /// Ordered (state-id, optional-forge-label) pairs of the chosen dimension.
    states: Vec<(String, Option<String>)>,
    /// Whether a usable exclusive lifecycle dimension was found.
    has_lifecycle: bool,
}

impl LaneMap {
    /// Choose the lifecycle dimension from a workflow and build the resolver.
    ///
    /// The heuristic (A.4): among the **exclusive** dimensions, pick the one
    /// whose ordered states cover the most artifact kinds; ties break on the
    /// most states, then declaration order. Logs and degrades when none exists.
    #[must_use]
    pub fn from_workflow(workflow: &ValidatedWorkflow) -> Self {
        match choose_lifecycle_dimension(workflow.state_dimensions()) {
            Some(dimension) => Self::from_dimension(dimension),
            None => {
                tracing::warn!(
                    target: "temper_web",
                    workflow = workflow.name(),
                    "no exclusive lifecycle state dimension; falling back to queued/in-flight lane mapping"
                );
                Self {
                    states: Vec::new(),
                    has_lifecycle: false,
                }
            }
        }
    }

    /// Build a resolver directly from a chosen exclusive lifecycle dimension.
    #[must_use]
    pub fn from_dimension(dimension: &ValidatedStateDimension) -> Self {
        let states = dimension
            .states
            .iter()
            .map(|state| {
                (
                    state.id.as_str().to_string(),
                    state.label.as_ref().map(|label| label.as_str().to_string()),
                )
            })
            .collect();
        Self {
            states,
            has_lifecycle: true,
        }
    }

    /// A resolver with no lifecycle dimension — every lifecycle lookup falls back
    /// to the queued/in-flight default. Used standalone (no workflow loaded).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            states: Vec::new(),
            has_lifecycle: false,
        }
    }

    /// Whether a usable exclusive lifecycle dimension backs this map.
    #[must_use]
    pub fn has_lifecycle(&self) -> bool {
        self.has_lifecycle
    }

    /// Resolve a board lane for a card currently in the given lifecycle state id.
    ///
    /// Matches the state id (and its bound forge label) against the lane keyword
    /// sets; falls back to the state's position in the ordered dimension when the
    /// name is unrecognised. Returns [`Lane::Triage`] when the state is unknown
    /// to this workflow (a newly-seen artifact not yet classified).
    #[must_use]
    pub fn lane_for_state(&self, state: &StateId) -> Lane {
        self.lane_for_token(state.as_str())
    }

    /// Resolve a lane from a raw state/label token (used by the log-tail adapter,
    /// which sees forge label strings in `labels.delta`).
    #[must_use]
    pub fn lane_for_token(&self, token: &str) -> Lane {
        if let Some(index) = self.position_of(token) {
            // Prefer a name match on the canonical state id or its forge label;
            // otherwise spread the state's position across the five lanes.
            let (state_id, label) = &self.states[index];
            return lane_by_name(state_id)
                .or_else(|| label.as_deref().and_then(lane_by_name))
                .unwrap_or_else(|| lane_by_position(index, self.states.len()));
        }
        // Token not part of the chosen dimension: try a direct name match (a
        // forge label that is not a lifecycle state), else default to triage.
        lane_by_name(token).unwrap_or(Lane::Triage)
    }

    /// The default lane for a card with no lifecycle signal yet — used by the
    /// snapshot projection for the queued/in-flight split when no lifecycle
    /// dimension is available. `in_flight` cards seed `implement`, queued seed
    /// `triage`.
    #[must_use]
    pub fn default_lane(&self, in_flight: bool) -> Lane {
        if in_flight {
            Lane::Implement
        } else {
            Lane::Triage
        }
    }

    fn position_of(&self, token: &str) -> Option<usize> {
        self.states
            .iter()
            .position(|(state_id, label)| state_id == token || label.as_deref() == Some(token))
    }
}

/// Pick the best exclusive lifecycle dimension: most artifact-kind coverage,
/// then most states, then first declared. Non-exclusive dimensions are ignored
/// (an artifact may hold several of their states at once — not a lane axis).
fn choose_lifecycle_dimension(
    dimensions: &[ValidatedStateDimension],
) -> Option<&ValidatedStateDimension> {
    dimensions
        .iter()
        .filter(|dimension| dimension.exclusive && !dimension.states.is_empty())
        .max_by_key(|dimension| (artifact_kind_coverage(dimension), dimension.states.len()))
}

/// Number of distinct artifact kinds the dimension's states reference (an empty
/// `artifacts` list means "all kinds", which we treat as broad coverage).
fn artifact_kind_coverage(dimension: &ValidatedStateDimension) -> usize {
    if dimension
        .states
        .iter()
        .any(|state| state.artifacts.is_empty())
    {
        return usize::MAX;
    }
    let mut kinds: Vec<&str> = dimension
        .states
        .iter()
        .flat_map(|state| {
            state
                .artifacts
                .iter()
                .map(temper_workflow::ArtifactKindId::as_str)
        })
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds.len()
}

/// Keyword heuristic from a state id / forge label to a board lane. Returns
/// `None` when no keyword matches (caller falls back to position).
fn lane_by_name(token: &str) -> Option<Lane> {
    let token = token.to_ascii_lowercase();
    // Order matters: `done`/`merged` before `review`, since a "review_done"
    // state is terminal-leaning; CI before review so `ci`/`gate` win.
    const DONE: &[&str] = &["done", "merged", "landed", "resolved", "closed", "complete"];
    const CI: &[&str] = &["ci", "gate", "verify", "check", "build", "test_gate"];
    const REVIEW: &[&str] = &["review", "approval", "pr_open", "pr_ready", "reviewing"];
    const IMPLEMENT: &[&str] = &[
        "code",
        "impl",
        "progress",
        "working",
        "in_progress",
        "build_",
    ];
    const TRIAGE: &[&str] = &[
        "triage",
        "intake",
        "untriaged",
        "raw",
        "new",
        "ready",
        "backlog",
    ];

    if DONE.iter().any(|kw| token.contains(kw)) {
        Some(Lane::Done)
    } else if CI.iter().any(|kw| token.contains(kw)) {
        Some(Lane::Ci)
    } else if REVIEW.iter().any(|kw| token.contains(kw)) {
        Some(Lane::Review)
    } else if IMPLEMENT.iter().any(|kw| token.contains(kw)) {
        Some(Lane::Implement)
    } else if TRIAGE.iter().any(|kw| token.contains(kw)) {
        Some(Lane::Triage)
    } else {
        None
    }
}

/// Spread an ordered state index across the five board lanes so an unrecognised
/// but ordered lifecycle still flows left→right. The last state always maps to
/// `Done`; the first to `Triage`.
fn lane_by_position(index: usize, total: usize) -> Lane {
    const LANES: [Lane; 5] = [
        Lane::Triage,
        Lane::Implement,
        Lane::Review,
        Lane::Ci,
        Lane::Done,
    ];
    if total <= 1 {
        return Lane::Triage;
    }
    if index + 1 == total {
        return Lane::Done;
    }
    // Map [0, total-1) proportionally onto [0, 5).
    let bucket = (index * LANES.len()) / total;
    LANES[bucket.min(LANES.len() - 1)]
}

#[cfg(test)]
#[path = "lanes_tests.rs"]
mod lanes_tests;
