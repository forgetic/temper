// SPDX-License-Identifier: MPL-2.0

//! Structured debug measurements derived from pure wake-coordinator decisions.

use temper_engine_io::EngineTime;
use temper_forge::{ChangeKind, RepositoryPath};

use super::machine::{DaemonMachine, DaemonRequest};
use super::wake_coordinator::{
    BroadMode, WakeDecision, WakeLane, WakeOutcome, WakePromotion, WakeScope,
};

/// Debug-only, machine-readable accounting for one coordinator decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WakeMeasurement {
    pub(super) repo: String,
    pub(super) role: Option<String>,
    pub(super) run_id: Option<String>,
    pub(super) reason: String,
    pub(super) scope: String,
    pub(super) outcome: &'static str,
    pub(super) phase: &'static str,
    pub(super) pending_target_count: usize,
    pub(super) in_flight_repository_count: usize,
    pub(super) queue_latency_ms: u64,
    pub(super) execution_duration_ms: u64,
    pub(super) error: Option<String>,
}

impl WakeMeasurement {
    pub(super) fn suppressed_heartbeat(repo: &RepositoryPath) -> Self {
        Self {
            repo: repository_label(repo),
            role: None,
            run_id: None,
            reason: "lease_heartbeat".to_string(),
            scope: "targeted".to_string(),
            outcome: "suppressed",
            phase: "decision",
            pending_target_count: 0,
            in_flight_repository_count: 0,
            queue_latency_ms: 0,
            execution_duration_ms: 0,
            error: None,
        }
    }
}

impl DaemonMachine {
    pub(super) fn wake_decision_requests(
        &self,
        decisions: Vec<WakeDecision>,
    ) -> Vec<DaemonRequest> {
        let mut requests = Vec::new();
        let in_flight_repository_count = self.wake_coordinator.in_flight_repositories();
        for decision in decisions {
            match decision {
                WakeDecision::Accepted { repo, lane, scope } => {
                    requests.push(DaemonRequest::WakeMeasurement(self.wake_measurement(
                        &repo,
                        Some(&lane),
                        None,
                        &scope,
                        "accepted",
                        "decision",
                        0,
                        0,
                        None,
                    )));
                }
                WakeDecision::Coalesced { repo, lane, scope } => {
                    requests.push(DaemonRequest::WakeMeasurement(self.wake_measurement(
                        &repo,
                        Some(&lane),
                        None,
                        &scope,
                        "coalesced",
                        "decision",
                        0,
                        0,
                        None,
                    )));
                }
                WakeDecision::Deferred { repo } => {
                    requests.push(DaemonRequest::WakeMeasurement(WakeMeasurement {
                        repo: repository_label(&repo),
                        role: None,
                        run_id: None,
                        reason: "apply_active".to_string(),
                        scope: "mixed".to_string(),
                        outcome: "deferred",
                        phase: "decision",
                        pending_target_count: self.wake_coordinator.pending_target_count(&repo),
                        in_flight_repository_count,
                        queue_latency_ms: 0,
                        execution_duration_ms: 0,
                        error: None,
                    }));
                }
                WakeDecision::BroadPromoted { repo, lane, mode } => {
                    let scope = WakeScope::broad(mode);
                    requests.push(DaemonRequest::WakeMeasurement(self.wake_measurement(
                        &repo,
                        Some(&lane),
                        None,
                        &scope,
                        "promoted",
                        "decision",
                        0,
                        0,
                        None,
                    )));
                }
                WakeDecision::Promoted { repo, destination } => {
                    requests.push(DaemonRequest::WakeMeasurement(WakeMeasurement {
                        repo: repository_label(&repo),
                        role: None,
                        run_id: None,
                        reason: match destination {
                            WakePromotion::Pending => "apply_finished",
                            WakePromotion::Dirty => "apply_finished_in_flight",
                        }
                        .to_string(),
                        scope: "mixed".to_string(),
                        outcome: "promoted",
                        phase: "decision",
                        pending_target_count: self.wake_coordinator.pending_target_count(&repo),
                        in_flight_repository_count,
                        queue_latency_ms: 0,
                        execution_duration_ms: 0,
                        error: None,
                    }));
                }
                WakeDecision::StartTimer {
                    repo,
                    generation,
                    delay,
                } => requests.push(DaemonRequest::StartWakeTimer {
                    repo,
                    generation,
                    delay,
                }),
                WakeDecision::Started { work } => {
                    let queue_latency_ms = elapsed_ms(work.queued_at, work.started_at);
                    let run_id = work.run_id();
                    for (lane, scope) in work.batch.lanes() {
                        requests.push(DaemonRequest::WakeMeasurement(self.wake_measurement(
                            &work.repo,
                            Some(lane),
                            Some(run_id.clone()),
                            scope,
                            "started",
                            "start",
                            queue_latency_ms,
                            0,
                            None,
                        )));
                    }
                    requests.push(DaemonRequest::RunWake { work });
                }
                WakeDecision::DirtyFollowUp {
                    repo,
                    generation,
                    lanes,
                } => {
                    let run_id = wake_run_id(&repo, generation);
                    for lane in lanes {
                        requests.push(DaemonRequest::WakeMeasurement(WakeMeasurement {
                            repo: repository_label(&repo),
                            role: Some(wake_lane_role(&lane).to_string()),
                            run_id: Some(run_id.clone()),
                            reason: "in_flight_hint".to_string(),
                            scope: "mixed".to_string(),
                            outcome: "dirty_follow_up",
                            phase: "decision",
                            pending_target_count: self.wake_coordinator.pending_target_count(&repo),
                            in_flight_repository_count,
                            queue_latency_ms: 0,
                            execution_duration_ms: 0,
                            error: None,
                        }));
                    }
                }
                WakeDecision::Finished { work, outcome } => {
                    let (wake_outcome, error) = match &outcome {
                        WakeOutcome::Succeeded => ("completed", None),
                        WakeOutcome::Failed { reason } => ("failed", Some(reason.clone())),
                    };
                    let queue_latency_ms = elapsed_ms(work.queued_at, work.started_at);
                    let execution_duration_ms = elapsed_ms(work.started_at, self.now);
                    let run_id = work.run_id();
                    for (lane, scope) in work.batch.lanes() {
                        requests.push(DaemonRequest::WakeMeasurement(self.wake_measurement(
                            &work.repo,
                            Some(lane),
                            Some(run_id.clone()),
                            scope,
                            wake_outcome,
                            "finish",
                            queue_latency_ms,
                            execution_duration_ms,
                            error.clone(),
                        )));
                    }
                }
                WakeDecision::IgnoredUnknownRepository { repo } => {
                    requests.push(DaemonRequest::WakeMeasurement(WakeMeasurement {
                        repo: repository_label(&repo),
                        role: None,
                        run_id: None,
                        reason: "unknown_repository".to_string(),
                        scope: "broad".to_string(),
                        outcome: "suppressed",
                        phase: "decision",
                        pending_target_count: 0,
                        in_flight_repository_count,
                        queue_latency_ms: 0,
                        execution_duration_ms: 0,
                        error: None,
                    }));
                }
                WakeDecision::IgnoredStaleTimer { repo, generation }
                | WakeDecision::IgnoredStaleCompletion { repo, generation } => {
                    requests.push(DaemonRequest::WakeMeasurement(WakeMeasurement {
                        repo: repository_label(&repo),
                        role: None,
                        run_id: Some(wake_run_id(&repo, generation)),
                        reason: "stale_generation".to_string(),
                        scope: "mixed".to_string(),
                        outcome: "coalesced",
                        phase: "decision",
                        pending_target_count: self.wake_coordinator.pending_target_count(&repo),
                        in_flight_repository_count,
                        queue_latency_ms: 0,
                        execution_duration_ms: 0,
                        error: None,
                    }));
                }
            }
        }
        requests
    }

    #[allow(clippy::too_many_arguments)]
    fn wake_measurement(
        &self,
        repo: &RepositoryPath,
        lane: Option<&WakeLane>,
        run_id: Option<String>,
        scope: &WakeScope,
        outcome: &'static str,
        phase: &'static str,
        queue_latency_ms: u64,
        execution_duration_ms: u64,
        error: Option<String>,
    ) -> WakeMeasurement {
        WakeMeasurement {
            repo: repository_label(repo),
            role: lane.map(wake_lane_role).map(str::to_string),
            run_id,
            reason: wake_scope_reason(scope).to_string(),
            scope: match scope {
                WakeScope::Targeted(_) => "targeted",
                WakeScope::Broad { targets, .. } if targets.is_empty() => "broad",
                WakeScope::Broad { .. } => "mixed",
            }
            .to_string(),
            outcome,
            phase,
            pending_target_count: self.wake_coordinator.pending_target_count(repo),
            in_flight_repository_count: self.wake_coordinator.in_flight_repositories(),
            queue_latency_ms,
            execution_duration_ms,
            error,
        }
    }
}

fn repository_label(repo: &RepositoryPath) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

fn wake_run_id(repo: &RepositoryPath, generation: u64) -> String {
    format!("{}:{generation}", repository_label(repo))
}

fn wake_lane_role(lane: &WakeLane) -> &str {
    match lane {
        WakeLane::Role(role) => role.as_str(),
        WakeLane::Mechanical => "mechanical",
    }
}

fn wake_scope_reason(scope: &WakeScope) -> &'static str {
    match scope {
        WakeScope::Targeted(targets) => targets
            .values()
            .next()
            .map(change_reason)
            .unwrap_or("targeted"),
        WakeScope::Broad { mode, .. } => broad_reason(*mode),
    }
}

fn change_reason(change: &ChangeKind) -> &'static str {
    match change {
        ChangeKind::Created => "created",
        ChangeKind::Edited => "edited",
        ChangeKind::Body => "body",
        ChangeKind::Title => "title",
        ChangeKind::State => "state",
        ChangeKind::Label => "label",
        ChangeKind::Dependency => "dependency",
        ChangeKind::Assignee => "assignee",
        ChangeKind::Comment => "comment",
        ChangeKind::Review => "review",
        ChangeKind::Push => "push",
        ChangeKind::Ci => "ci",
        ChangeKind::Unknown => "unknown",
    }
}

fn broad_reason(mode: BroadMode) -> &'static str {
    match mode {
        BroadMode::Repository => "repository",
        BroadMode::Unknown => "unknown",
        BroadMode::Push => "push",
        BroadMode::Recovery => "recovery",
        BroadMode::Poll => "poll",
        BroadMode::Startup => "startup",
        BroadMode::Overflow => "target_overflow",
    }
}

fn elapsed_ms(start: EngineTime, end: EngineTime) -> u64 {
    end.as_nanos()
        .saturating_sub(start.as_nanos())
        .saturating_div(1_000_000)
}
