// SPDX-License-Identifier: MPL-2.0

//! Feed 1b — the log-tail adapter (UX Appendix A.2).
//!
//! `temper-log` emits one structured `tracing::info!` per workflow event; under
//! `TEMPER_LOG_FORMAT=json` (or `journalctl -o json`) each becomes a JSON line
//! whose `fields` object carries flat, dotted keys: `event`, `artifact.ref`,
//! `repo`, `role`, `transition`, `labels.delta`, `conclusion`, … This adapter
//! parses those lines and maps the closed event vocabulary into board [`Delta`]s,
//! joined onto cards by `artifact.ref` (the same key the snapshot projection
//! derives card ids from).
//!
//! We deserialize the lines with our OWN structs (we must not modify
//! `temper-log`), pinning the field set with fixtures the TS side also reads.
//!
//! ## Tailing semantics (which events refine vs. create)
//!
//! The board relies on **Part A's periodic snapshot re-poll for card
//! *existence***; the live tail (opened with `from_end = true`, see
//! [`crate::logsource`]) only **refines** cards that already exist. There is no
//! bounded history replay: events emitted before temper-web started are not
//! re-read, by design — the snapshot re-poll backfills any in-flight item, after
//! which the tail keeps it current. Consequently a [`Delta::MoveCard`] for a card
//! the snapshot has not seeded yet is a harmless no-op in the read-model
//! (advances `seq`, changes nothing visible) rather than an error.
//!
//! ## Handled vs. intentionally-ignored events
//!
//! Handled: `transition.applied`, `queue.entered`, `gate.evaluated`, `pr.opened`,
//! `pr.merged`, `ci.completed`, `lease.lost`, `agent.started`, `agent.finished`,
//! `role.saturated`, `item.resolved`. Everything else hits `_ => Vec::new()` and
//! is dropped (forward-compatible). `lease.claimed` is intentionally NOT mapped:
//! `agent.started` already supplies the richer activity hint and clears the
//! lease-lost problem, so a `lease.claimed` arm would only duplicate it.
//!
//! ## The injectable source seam
//!
//! Lines arrive through a [`LogLineSource`] iterator. Today that wraps a
//! file/reader (or a test's canned `Vec`); a future in-process broadcast seam in
//! `temper-log` swaps in behind the same trait with no change to the read-model
//! or the UI (UX §6.1 a→b).

use serde::Deserialize;
use temper_log::strip_provider_scheme;

use crate::board::{Activity, ActivityKind, CiStatus, Problem, Sev};
use crate::project::lanes::LaneMap;
use crate::project::snapshot::card_id_for_ref;
use crate::readmodel::Delta;

/// A source of `temper-log` JSON lines. The production source wraps a tailed
/// file/journald reader; tests inject a canned line list. A future broadcast
/// seam implements the same trait.
pub trait LogLineSource {
    /// The next raw JSON line, or `None` when the source is exhausted (a live
    /// tail blocks instead of returning `None`; this is the pull shape tests and
    /// the file-replay source use).
    fn next_line(&mut self) -> Option<String>;
}

impl LogLineSource for std::vec::IntoIter<String> {
    fn next_line(&mut self) -> Option<String> {
        self.next()
    }
}

/// One parsed `temper-log` JSON line. tracing's json sink nests the structured
/// fields under `fields`; spans (carrying `artifact.ref`) appear separately, but
/// every event we care about also stamps `artifact.ref` directly into `fields`.
#[derive(Debug, Clone, Deserialize)]
struct LogLine {
    #[serde(default)]
    fields: LogFields,
}

/// The flat, dotted `fields` object of a `temper-log` JSON line.
#[derive(Debug, Clone, Default, Deserialize)]
struct LogFields {
    #[serde(default)]
    event: Option<String>,
    #[serde(rename = "artifact.ref", default)]
    artifact_ref: Option<String>,
    #[serde(rename = "pr.ref", default)]
    pr_ref: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    transition: Option<String>,
    #[serde(rename = "labels.delta", default)]
    labels_delta: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    concurrency: Option<u64>,
    #[serde(default)]
    queued: Option<String>,
    /// Destination queue/stage of a `queue.entered` line (`temper-log` emits this
    /// as the `queue.to` field, e.g. `code_ready`, `triage`, `landing`).
    #[serde(rename = "queue.to", default)]
    queued_to: Option<String>,
    /// Pre-joined gate summary of a `gate.evaluated` line (`temper-log` emits this
    /// as the `gates` field, e.g. `ci_gate=pending dependency_gate=ok`). The
    /// adapter inspects the per-gate `name=state` tokens to decide block vs pass.
    #[serde(default)]
    gates: Option<String>,
}

impl LogFields {
    /// The join ref for this line, preferring `artifact.ref` then `pr.ref`.
    fn join_ref(&self) -> Option<&str> {
        self.artifact_ref.as_deref().or(self.pr_ref.as_deref())
    }
}

/// Maps `temper-log` JSON lines to board deltas. Stateless except for the lane
/// resolver it carries (so lifecycle transitions land in the right column).
#[derive(Debug, Clone)]
pub struct LogTailAdapter {
    lanes: LaneMap,
    now: i64,
}

impl LogTailAdapter {
    /// Build an adapter that resolves lifecycle lanes via `lanes` and stamps
    /// `now` onto card moves / problem `since` (the caller supplies the clock so
    /// the adapter stays pure and testable).
    #[must_use]
    pub fn new(lanes: LaneMap, now: i64) -> Self {
        Self { lanes, now }
    }

    /// Advance the adapter's clock (the server bumps this before draining).
    pub fn set_now(&mut self, now: i64) {
        self.now = now;
    }

    /// Parse one raw JSON line and map it to board deltas. Unparseable lines and
    /// lines outside the known vocabulary yield an empty vec (forward-compatible,
    /// matching the worker's "ignore unparseable lines" stance).
    #[must_use]
    pub fn deltas_for_line(&self, line: &str) -> Vec<Delta> {
        let Ok(parsed) = serde_json::from_str::<LogLine>(line) else {
            return Vec::new();
        };
        self.deltas_for_fields(&parsed.fields)
    }

    /// Drain a [`LogLineSource`] into all the board deltas it yields, in order.
    #[must_use]
    pub fn drain<S: LogLineSource>(&self, source: &mut S) -> Vec<Delta> {
        let mut deltas = Vec::new();
        while let Some(line) = source.next_line() {
            deltas.extend(self.deltas_for_line(&line));
        }
        deltas
    }

    fn deltas_for_fields(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(event) = fields.event.as_deref() else {
            return Vec::new();
        };
        match event {
            "transition.applied" => self.on_transition(fields),
            "queue.entered" => self.on_queue_entered(fields),
            "gate.evaluated" => self.on_gate_evaluated(fields),
            "pr.opened" => self.on_pr_opened(fields),
            "pr.merged" => self.on_pr_merged(fields),
            "ci.completed" => self.on_ci_completed(fields),
            "lease.lost" => self.on_lease_lost(fields),
            "agent.started" => self.on_agent_started(fields),
            "agent.finished" => self.on_agent_finished(fields),
            "role.saturated" => self.on_role_saturated(fields),
            "item.resolved" => self.on_item_resolved(fields),
            _ => Vec::new(),
        }
    }

    /// `transition.applied` — the primary lane-movement signal. The lane comes
    /// from the lifecycle state the labels move TO; we read it out of the
    /// `+state` tokens in `labels.delta`, falling back to the transition name.
    fn on_transition(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let lane = added_label(fields.labels_delta.as_deref())
            .map(|token| self.lanes.lane_for_token(token))
            .or_else(|| {
                fields
                    .transition
                    .as_deref()
                    .map(|t| self.lanes.lane_for_token(t))
            });
        match lane {
            Some(lane) => vec![Delta::MoveCard {
                id: card,
                lane,
                now: self.now,
            }],
            None => Vec::new(),
        }
    }

    /// `queue.entered` — the natural "an item entered a stage" signal. The
    /// destination lane comes from the queue the item entered (`temper-log`'s
    /// `queue.to` field) resolved through [`LaneMap::lane_for_token`]; a line
    /// without a `queue.to` (or without a join ref) yields no delta so future
    /// queue events stay forward-compatible.
    ///
    /// Note: a `queue.entered` for a card the read-model has not seen yet is a
    /// no-op in the projection — [`Delta::MoveCard`] on an unknown id advances
    /// `seq` but changes nothing visible. That is acceptable: Part A's periodic
    /// snapshot re-poll is authoritative for card *existence* and seeds the card,
    /// after which a later `queue.entered` refines its lane. The live tail only
    /// refines; it does not create cards on its own.
    fn on_queue_entered(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let Some(queue) = fields.queued_to.as_deref() else {
            return Vec::new();
        };
        vec![Delta::MoveCard {
            id: card,
            lane: self.lanes.lane_for_token(queue),
            now: self.now,
        }]
    }

    /// `gate.evaluated` — the source of the gate/attention badge (UX §4.3). The
    /// `gates` field is a pre-joined `name=state name=state` summary (e.g.
    /// `ci_gate=pending dependency_gate=ok`) where each state is `ok` (passed),
    /// `failed` (hard block) or `pending` (soft wait). When every gate is `ok` the
    /// card is cleared of its `gate:{card}` problem; otherwise a problem is raised
    /// — `Sev::Bad` if any gate is a hard `failed`, else `Sev::Warn` for a pending
    /// wait. Reuses the existing `AddProblem`/`ClearProblem` pair so no new wire
    /// field or `BoardEvent` variant is needed.
    fn on_gate_evaluated(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let gates = fields.gates.as_deref().unwrap_or("");
        match gate_verdict(gates) {
            GateVerdict::Passed => vec![Delta::ClearProblem {
                id: gate_problem_id(&card),
            }],
            GateVerdict::Blocked { hard, detail } => {
                let join = fields.join_ref().unwrap_or(&card).to_string();
                vec![Delta::AddProblem {
                    id: gate_problem_id(&card),
                    problem: Problem {
                        sev: if hard { Sev::Bad } else { Sev::Warn },
                        msg: format!("{join} gate blocked: {detail}"),
                        card: card.clone(),
                        since: self.now,
                    },
                }]
            }
        }
    }

    /// `pr.opened` — a PR entered review; clear any failed-CI overlay it carries.
    fn on_pr_opened(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        vec![Delta::MoveCard {
            id: card,
            lane: crate::board::Lane::Review,
            now: self.now,
        }]
    }

    /// `pr.merged` — terminal; the card lands in `done` and clears CI/problems.
    fn on_pr_merged(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        vec![
            Delta::MoveCard {
                id: card.clone(),
                lane: crate::board::Lane::Done,
                now: self.now,
            },
            Delta::SetMerged {
                id: card.clone(),
                merged: true,
            },
            Delta::ClearProblem {
                id: ci_problem_id(&card),
            },
        ]
    }

    /// `ci.completed` — set/clear the card's CI badge and a problem on failure.
    fn on_ci_completed(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let failed = fields
            .conclusion
            .as_deref()
            .is_some_and(|c| !c.eq_ignore_ascii_case("success"));
        if failed {
            let join = fields.join_ref().unwrap_or(&card).to_string();
            vec![
                Delta::SetCi {
                    id: card.clone(),
                    ci: Some(CiStatus::Failed),
                },
                Delta::AddProblem {
                    id: ci_problem_id(&card),
                    problem: Problem {
                        sev: Sev::Bad,
                        msg: format!("{join} CI failed"),
                        card: card.clone(),
                        since: self.now,
                    },
                },
            ]
        } else {
            vec![
                Delta::SetCi {
                    id: card.clone(),
                    ci: None,
                },
                Delta::ClearProblem {
                    id: ci_problem_id(&card),
                },
            ]
        }
    }

    /// `lease.lost` — the worker lost the lease; raise a stuck/attention problem.
    fn on_lease_lost(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let join = fields.join_ref().unwrap_or(&card).to_string();
        let reason = fields.reason.as_deref().unwrap_or("no reassignment");
        vec![Delta::AddProblem {
            id: lease_problem_id(&card),
            problem: Problem {
                sev: Sev::Bad,
                msg: format!("{join} lease lost — {reason}"),
                card: card.clone(),
                since: self.now,
            },
        }]
    }

    /// `agent.started` — the agent began working; show a live activity hint and
    /// clear any prior lease-lost problem (it was reassigned).
    fn on_agent_started(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        let kind = fields.kind.as_deref().unwrap_or("working");
        let role = fields.role.as_deref().unwrap_or("agent");
        vec![
            Delta::ClearProblem {
                id: lease_problem_id(&card),
            },
            Delta::SetActivity {
                id: card,
                activity: Activity {
                    kind: ActivityKind::Think,
                    text: format!("{role}: {kind}"),
                },
            },
        ]
    }

    /// `agent.finished` — clear the live activity hint (the run ended).
    fn on_agent_finished(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        vec![Delta::SetActivity {
            id: card,
            activity: Activity {
                kind: ActivityKind::Think,
                text: "idle".to_string(),
            },
        }]
    }

    /// `role.saturated` — a cross-repo attention overlay: one problem per queued
    /// ref behind the busy role (NOT a lane, per A.4).
    fn on_role_saturated(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(role) = fields.role.as_deref() else {
            return Vec::new();
        };
        let concurrency = fields.concurrency.unwrap_or(1);
        fields
            .queued
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|waiting| !waiting.is_empty())
            .map(|waiting| {
                let card = card_id_for_ref(waiting);
                Delta::AddProblem {
                    id: format!("sat:{role}:{waiting}"),
                    problem: Problem {
                        sev: Sev::Warn,
                        msg: format!(
                            "{role} saturated ({concurrency} in flight) — {waiting} waiting"
                        ),
                        card,
                        since: self.now,
                    },
                }
            })
            .collect()
    }

    /// `item.resolved` — the work item finished; land it in `done` and clear its
    /// activity. (A non-PR item that resolved without a merge.)
    fn on_item_resolved(&self, fields: &LogFields) -> Vec<Delta> {
        let Some(card) = self.card_id(fields) else {
            return Vec::new();
        };
        vec![Delta::MoveCard {
            id: card,
            lane: crate::board::Lane::Done,
            now: self.now,
        }]
    }

    fn card_id(&self, fields: &LogFields) -> Option<String> {
        fields.join_ref().map(card_id_for_ref)
    }
}

/// The first `+state` token of a `labels.delta` string (`-untriaged +code
/// +ready` → `code`): the lifecycle state the artifact moved INTO. Skips the
/// generic `+ready` readiness flag so it does not shadow the real state.
fn added_label(labels_delta: Option<&str>) -> Option<&str> {
    let delta = labels_delta?;
    delta
        .split_whitespace()
        .filter_map(|token| token.strip_prefix('+'))
        .find(|token| *token != "ready")
}

/// Stable problem id for a card's CI failure (so the matching clear lines up).
fn ci_problem_id(card: &str) -> String {
    format!("ci:{card}")
}

/// Stable problem id for a card's lost lease.
fn lease_problem_id(card: &str) -> String {
    format!("lease:{card}")
}

/// Stable problem id for a card's blocking gate (so the matching clear lines up).
fn gate_problem_id(card: &str) -> String {
    format!("gate:{card}")
}

/// The verdict of a `gate.evaluated` `gates` summary.
enum GateVerdict {
    /// Every gate passed (`ok`) — the card's gate problem should be cleared.
    Passed,
    /// At least one gate is not `ok`. `hard` is true when any gate `failed`
    /// outright (a hard block → `Sev::Bad`) rather than merely being `pending`
    /// (a soft wait → `Sev::Warn`). `detail` is the offending `name=state` tokens.
    Blocked { hard: bool, detail: String },
}

/// Classify a pre-joined `name=state name=state` gate summary
/// (`ci_gate=pending dependency_gate=ok`). A gate is satisfied when its state is
/// `ok`/`passed`/`success`; any other state blocks. A `failed` (or `error`)
/// state is a hard block. An empty/`name`-less summary counts as passed (nothing
/// to block on), matching the forward-compatible "unknown → don't flag" stance.
fn gate_verdict(gates: &str) -> GateVerdict {
    let mut hard = false;
    let mut blocking = Vec::new();
    for token in gates.split_whitespace() {
        let state = token.split_once('=').map_or(token, |(_, s)| s);
        if matches!(
            state.to_ascii_lowercase().as_str(),
            "ok" | "passed" | "pass" | "success" | "green"
        ) {
            continue;
        }
        if matches!(
            state.to_ascii_lowercase().as_str(),
            "failed" | "fail" | "error"
        ) {
            hard = true;
        }
        blocking.push(token);
    }
    if blocking.is_empty() {
        GateVerdict::Passed
    } else {
        GateVerdict::Blocked {
            hard,
            detail: blocking.join(" "),
        }
    }
}

/// Normalise a `repo` field by stripping any `provider:` scheme — reused from
/// `temper-log` so the join key matches the daemon's. Exposed for the server's
/// future repo-scoped filtering.
#[must_use]
pub fn bare_repo(repo: &str) -> &str {
    strip_provider_scheme(repo)
}

#[cfg(test)]
#[path = "logtail_tests.rs"]
mod logtail_tests;
