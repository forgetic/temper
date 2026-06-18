// SPDX-License-Identifier: MPL-2.0

//! The in-memory board read-model: cards, workers, and problems, projected from
//! a fresh daemon snapshot plus a tail of log-derived deltas.
//!
//! It is **derived state** — no durable store. It can be killed and rebuilt from
//! a fresh snapshot plus the event tail (UX §6.2). Each applied delta bumps a
//! monotonic `seq` cursor so the SSE stream resumes from the snapshot's cursor
//! with no gap and no dup (UX §6.3): the snapshot carries `seq`, and the client
//! re-snaps on reconnect then resumes from there.
//!
//! The model mirrors the TS reducer (`model.ts` `apply`): an event targeting an
//! unknown card id is a no-op that still advances the cursor, so the two sides
//! agree on the sequence space.

use crate::board::{
    Activity, BoardEvent, Card, CiStatus, Lane, Problem, SnapshotState, Steps, Workers,
};

/// A board delta to apply to the read-model. These are the projection's output
/// (from snapshot projection or the log-tail adapter); the model turns each into
/// a sequenced [`BoardEvent`] for the SSE fan-out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    /// A new card was discovered (e.g. a freshly-queued job). Replaces any
    /// existing card with the same id.
    UpsertCard(Card),
    /// Move a card to a new lane at `now` (resets its age).
    MoveCard { id: String, lane: Lane, now: i64 },
    /// Update a card's live activity hint.
    SetActivity { id: String, activity: Activity },
    /// Update a card's step progress.
    SetSteps { id: String, steps: Steps },
    /// Set (or clear) a card's CI badge.
    SetCi { id: String, ci: Option<CiStatus> },
    /// Mark a card merged (terminal).
    SetMerged { id: String, merged: bool },
    /// Add or replace a problem row, keyed by `id`.
    AddProblem { id: String, problem: Problem },
    /// Clear a problem row.
    ClearProblem { id: String },
    /// Replace the worker health tile.
    SetWorkers(Workers),
}

/// The in-memory board projection.
#[derive(Debug, Clone, Default)]
pub struct ReadModel {
    state: SnapshotState,
    seq: u64,
}

impl ReadModel {
    /// An empty read-model at seq 0 — the standalone/no-daemon cold start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the whole projection from a fresh snapshot, adopting its cards,
    /// problems, and worker tile. The cursor advances by one so a subsequent
    /// snapshot is distinguishable; callers seed from [`Self::snapshot_event`].
    pub fn load_snapshot(&mut self, state: SnapshotState) {
        self.state = state;
        self.seq += 1;
    }

    /// The current sequence cursor (the seq of the most recently applied event).
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The current worker tile.
    #[must_use]
    pub fn workers(&self) -> Workers {
        self.state.workers
    }

    /// A read-only view of the current cards (keyed by card id).
    #[must_use]
    pub fn cards(&self) -> &std::collections::BTreeMap<String, Card> {
        &self.state.cards
    }

    /// A read-only view of the current problems (keyed by problem id).
    #[must_use]
    pub fn problems(&self) -> &std::collections::BTreeMap<String, Problem> {
        &self.state.problems
    }

    /// The cold-start [`BoardEvent::Snapshot`] at the current cursor.
    #[must_use]
    pub fn snapshot_event(&self) -> BoardEvent {
        BoardEvent::Snapshot {
            seq: self.seq,
            state: self.state.clone(),
        }
    }

    /// Apply a delta, mutate the projection, bump the cursor, and return the
    /// sequenced [`BoardEvent`] to fan out to subscribers — or `None` when the
    /// delta is structurally a no-op that need not be broadcast (an upsert that
    /// changes nothing, or a clear of a missing problem). Unknown-card deltas
    /// still emit (advancing the shared cursor), matching the TS reducer.
    pub fn apply(&mut self, delta: Delta) -> Option<BoardEvent> {
        match delta {
            Delta::UpsertCard(card) => self.upsert_card(card),
            Delta::MoveCard { id, lane, now } => self.emit_card_move(&id, lane, now),
            Delta::SetActivity { id, activity } => self.emit_set_activity(&id, activity),
            Delta::SetSteps { id, steps } => self.emit_set_steps(&id, steps),
            Delta::SetCi { id, ci } => self.emit_set_ci(&id, ci),
            Delta::SetMerged { id, merged } => self.emit_set_merged(&id, merged),
            Delta::AddProblem { id, problem } => self.emit_add_problem(id, problem),
            Delta::ClearProblem { id } => self.emit_clear_problem(&id),
            Delta::SetWorkers(workers) => self.emit_set_workers(workers),
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn upsert_card(&mut self, card: Card) -> Option<BoardEvent> {
        // A newly-discovered card has no dedicated client event in `model.ts`;
        // surface it via a fresh snapshot so the client learns its full shape.
        // (Upserting an identical card is a no-op — no snapshot churn.)
        if self.state.cards.get(&card.id) == Some(&card) {
            return None;
        }
        self.state.cards.insert(card.id.clone(), card);
        let seq = self.next_seq();
        Some(BoardEvent::Snapshot {
            seq,
            state: self.state.clone(),
        })
    }

    fn emit_card_move(&mut self, id: &str, lane: Lane, now: i64) -> Option<BoardEvent> {
        if let Some(card) = self.state.cards.get_mut(id) {
            card.lane = lane;
            card.entered_at = now;
        }
        let seq = self.next_seq();
        Some(BoardEvent::CardMove {
            seq,
            id: id.to_string(),
            lane,
            now,
        })
    }

    fn emit_set_activity(&mut self, id: &str, activity: Activity) -> Option<BoardEvent> {
        if let Some(card) = self.state.cards.get_mut(id) {
            card.activity = Some(activity.clone());
        }
        let seq = self.next_seq();
        Some(BoardEvent::CardActivity {
            seq,
            id: id.to_string(),
            activity,
        })
    }

    fn emit_set_steps(&mut self, id: &str, steps: Steps) -> Option<BoardEvent> {
        if let Some(card) = self.state.cards.get_mut(id) {
            card.steps = Some(steps);
        }
        let seq = self.next_seq();
        Some(BoardEvent::CardStep {
            seq,
            id: id.to_string(),
            steps,
        })
    }

    fn emit_set_ci(&mut self, id: &str, ci: Option<CiStatus>) -> Option<BoardEvent> {
        // `ci` is not its own client event; reflect it through a snapshot so the
        // card's badge updates. No-op when nothing changed.
        match self.state.cards.get_mut(id) {
            Some(card) if card.ci != ci => card.ci = ci,
            _ => return None,
        }
        let seq = self.next_seq();
        Some(BoardEvent::Snapshot {
            seq,
            state: self.state.clone(),
        })
    }

    fn emit_set_merged(&mut self, id: &str, merged: bool) -> Option<BoardEvent> {
        match self.state.cards.get_mut(id) {
            Some(card) if card.merged != Some(merged) => {
                card.merged = Some(merged);
                card.ci = None;
            }
            _ => return None,
        }
        let seq = self.next_seq();
        Some(BoardEvent::Snapshot {
            seq,
            state: self.state.clone(),
        })
    }

    fn emit_add_problem(&mut self, id: String, problem: Problem) -> Option<BoardEvent> {
        if self.state.problems.get(&id) == Some(&problem) {
            return None;
        }
        self.state.problems.insert(id.clone(), problem.clone());
        let seq = self.next_seq();
        Some(BoardEvent::ProblemAdd { seq, id, problem })
    }

    fn emit_clear_problem(&mut self, id: &str) -> Option<BoardEvent> {
        if self.state.problems.remove(id).is_none() {
            return None;
        }
        let seq = self.next_seq();
        Some(BoardEvent::ProblemClear {
            seq,
            id: id.to_string(),
        })
    }

    fn emit_set_workers(&mut self, workers: Workers) -> Option<BoardEvent> {
        if self.state.workers == workers {
            return None;
        }
        self.state.workers = workers;
        let seq = self.next_seq();
        Some(BoardEvent::Snapshot {
            seq,
            state: self.state.clone(),
        })
    }
}

#[cfg(test)]
#[path = "readmodel_tests.rs"]
mod readmodel_tests;
