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
    Activity, BoardEvent, Card, CiStatus, Lane, Problem, SnapshotState, Steps, StreamEvent, Workers,
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
    /// Append a low-rate event to the card's client-side bounded ring. The Rust
    /// read model deliberately does not persist this ephemeral stream.
    PushStream { id: String, event: StreamEvent },
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
    /// Per-card "last touched" epoch ms — bumped on every card-targeting delta in
    /// [`Self::apply`] and on every [`Self::reconcile_snapshot`] that re-lists the
    /// card. The snapshot re-poll uses it as a grace window: a card the live log
    /// just moved (e.g. to `done`/merged) and that the daemon has already dropped
    /// from its `in_flight` list must not be auto-dropped during that window.
    touched: std::collections::BTreeMap<String, i64>,
}

impl ReadModel {
    /// Grace window (ms) for [`Self::reconcile_snapshot`]: a card absent from a
    /// fresh snapshot is kept rather than dropped if a log delta touched it within
    /// this window. Covers the race where a `pr.merged` delta has just moved a
    /// card to `done` and the next daemon snapshot no longer lists it — the board
    /// should keep showing it briefly rather than flicker it away. A few seconds
    /// is comfortably longer than a typical poll interval (2–5s).
    pub const RECONCILE_GRACE_MS: i64 = 5_000;

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

    /// Merge a freshly-projected snapshot into the live model and return the
    /// [`BoardEvent`]s to broadcast — the periodic re-poll's reconcile step.
    ///
    /// Unlike [`Self::load_snapshot`] (cold-start wholesale replace), this
    /// **merges**, respecting the division of authority between the two feeds:
    ///
    /// - **Existence is the snapshot's** — a card present in `fresh` but missing
    ///   from the model is added (surfaced via a fresh [`BoardEvent::Snapshot`],
    ///   exactly as [`Self::upsert_card`] does); its lane is whatever the snapshot
    ///   projection seeded (the queued/in-flight split).
    /// - **Lifecycle is the log-tail's** — for a card that already exists, the
    ///   re-poll does NOT touch `lane`/`activity`/`ci`/`steps`/`merged`; the live
    ///   log feed owns those. A re-poll re-listing an existing card is a no-op for
    ///   that card's lifecycle fields.
    /// - **Drops are graced** — a card absent from `fresh` is dropped only if it
    ///   isn't `merged` / in the `done` lane (terminal cards are never auto-dropped
    ///   here) AND it wasn't touched within [`Self::RECONCILE_GRACE_MS`] of `now`.
    /// - The **worker tile** and **role-saturation problems** are always refreshed
    ///   from the snapshot (the snapshot owns those): new problems are added, stale
    ///   snapshot-derived problems are cleared.
    ///
    /// Deterministic and side-effect-free apart from the state mutation and the
    /// returned events (each advances `seq` via the same path as [`Self::apply`]).
    pub fn reconcile_snapshot(&mut self, fresh: SnapshotState, now: i64) -> Vec<BoardEvent> {
        let mut events = Vec::new();

        // 1. Existence: add cards present in `fresh` but missing here. Existing
        //    cards keep their log-refined lifecycle (no-op), so just skip them.
        for (id, card) in &fresh.cards {
            if !self.state.cards.contains_key(id) {
                self.touch(id, now);
                if let Some(event) = self.upsert_card(card.clone()) {
                    events.push(event);
                }
            } else {
                // Re-listed in the snapshot — extend the grace window so a card
                // the daemon still reports isn't dropped on a later jittered poll.
                self.touch(id, now);
            }
        }

        // 2. Drops: cards no longer in `fresh`, subject to the grace rule. Collect
        //    ids first to avoid mutating the map mid-iteration; deterministic by
        //    BTreeMap order.
        let to_drop: Vec<String> = self
            .state
            .cards
            .iter()
            .filter(|(id, card)| !fresh.cards.contains_key(*id) && self.is_droppable(id, card, now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_drop {
            self.state.cards.remove(&id);
            self.touched.remove(&id);
            // Removal has no dedicated client event; surface it via a snapshot
            // (mirrors how new cards / ci / merged are surfaced).
            let seq = self.next_seq();
            events.push(BoardEvent::Snapshot {
                seq,
                state: self.state.clone(),
            });
        }

        // 3. Worker tile — always refresh from the snapshot.
        if let Some(event) = self.emit_set_workers(fresh.workers) {
            events.push(event);
        }

        // 4. Role-saturation problems — the snapshot owns these. Add the fresh
        //    ones and clear any stale snapshot-derived (`sat:`) problem.
        for (id, problem) in &fresh.problems {
            if let Some(event) = self.emit_add_problem(id.clone(), problem.clone()) {
                events.push(event);
            }
        }
        let stale_problems: Vec<String> = self
            .state
            .problems
            .keys()
            .filter(|id| id.starts_with("sat:") && !fresh.problems.contains_key(*id))
            .cloned()
            .collect();
        for id in stale_problems {
            if let Some(event) = self.emit_clear_problem(&id) {
                events.push(event);
            }
        }

        events
    }

    /// Whether a card absent from a fresh snapshot may be dropped: not terminal
    /// (`merged` / in the `done` lane) and outside the grace window.
    fn is_droppable(&self, id: &str, card: &Card, now: i64) -> bool {
        if card.merged == Some(true) || card.lane == Lane::Done {
            return false;
        }
        let touched_at = self.touched.get(id).copied().unwrap_or(card.entered_at);
        now.saturating_sub(touched_at) > Self::RECONCILE_GRACE_MS
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
        // Bump the grace-window clock for any card-targeting delta, using the
        // delta's own `now` when it carries one (a `MoveCard`), else the latest
        // known card timestamp. This lets the snapshot re-poll's drop rule respect
        // recently-touched cards (see [`Self::reconcile_snapshot`]).
        match &delta {
            Delta::MoveCard { id, now, .. } => self.touch(id, *now),
            Delta::UpsertCard(card) => self.touch(&card.id.clone(), card.entered_at),
            Delta::SetActivity { id, .. }
            | Delta::SetSteps { id, .. }
            | Delta::PushStream { id, .. }
            | Delta::SetCi { id, .. }
            | Delta::SetMerged { id, .. } => self.touch_existing(id),
            Delta::AddProblem { .. } | Delta::ClearProblem { .. } | Delta::SetWorkers(_) => {}
        }
        match delta {
            Delta::UpsertCard(card) => self.upsert_card(card),
            Delta::MoveCard { id, lane, now } => self.emit_card_move(&id, lane, now),
            Delta::SetActivity { id, activity } => self.emit_set_activity(&id, activity),
            Delta::SetSteps { id, steps } => self.emit_set_steps(&id, steps),
            Delta::PushStream { id, event } => self.emit_stream(&id, event),
            Delta::SetCi { id, ci } => self.emit_set_ci(&id, ci),
            Delta::SetMerged { id, merged } => self.emit_set_merged(&id, merged),
            Delta::AddProblem { id, problem } => self.emit_add_problem(id, problem),
            Delta::ClearProblem { id } => self.emit_clear_problem(&id),
            Delta::SetWorkers(workers) => self.emit_set_workers(workers),
        }
    }

    /// Record that a card was touched at `now` (the grace-window clock). The max
    /// keeps the touch monotonic so an out-of-order older `now` can't shrink it.
    fn touch(&mut self, id: &str, now: i64) {
        let entry = self.touched.entry(id.to_string()).or_insert(now);
        *entry = (*entry).max(now);
    }

    /// Touch a card by id at its own latest-known timestamp — used by deltas that
    /// don't carry a `now` (activity/steps/ci/merged refinements). Falls back to
    /// the card's `entered_at` so a refinement still extends the grace window.
    fn touch_existing(&mut self, id: &str) {
        let now = self
            .state
            .cards
            .get(id)
            .map_or(0, |card| card.entered_at)
            .max(self.touched.get(id).copied().unwrap_or(0));
        self.touch(id, now);
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

    fn emit_stream(&mut self, id: &str, event: StreamEvent) -> Option<BoardEvent> {
        let seq = self.next_seq();
        Some(BoardEvent::CardStream {
            seq,
            id: id.to_string(),
            event,
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
        self.state.problems.remove(id)?;
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
