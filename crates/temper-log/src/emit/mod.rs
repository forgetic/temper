// SPDX-License-Identifier: MPL-2.0

//! The `emit_*` constructors: one `info`-level event, three projections.
//!
//! Each function here is the **single emit site** for one [`Event`] (§3 rule 3).
//! It expands to a [`tracing::info!`] with:
//!
//! - the correct `target` for the event's [`Service`] (a literal, as the
//!   [`tracing`] macro requires),
//! - the §3 structured fields as **real tracing fields** — `service`, `event`,
//!   `repo`, `artifact.ref` (dotted keys use the `a.b = value` macro form),
//!   `pr.ref` when relevant, `role`, `transition`, `queue.to`, `labels.delta`,
//!   `duration_ms`, … — never a JSON string in the message,
//! - the §7 human line as the trailing positional argument, rendered by the pure
//!   helpers in [`human`] from the *same* inputs, so the two projections cannot
//!   disagree.
//!
//! These are the functions WI-2/WI-3 call. They take typed input structs (one
//! per event family) so call sites stay readable and the field set is checked at
//! compile time. The startup/health lines (§7's boot block) are plain
//! info-string helpers grouped at the end — they carry no work-item identity, so
//! they only set `service`/`event`-free targets and a message.
//!
//! [`Event`]: crate::Event
//! [`Service`]: crate::Service

pub mod human;

use std::borrow::Cow;

use crate::WorkItemRef;
use crate::event::Event;
use crate::service::Service;

pub use temper_protocol_activity::AgentTerminalReasonV1;

// ---------------------------------------------------------------------------
// Input structs — one per event, so call sites name their fields.
// ---------------------------------------------------------------------------

/// Inputs for [`emit_issue_opened`] (`trigger` / `issue.opened`).
#[derive(Clone, Debug)]
pub struct IssueOpened<'a> {
    /// The opened issue.
    pub item: &'a WorkItemRef,
    /// Forge handle of the issue author.
    pub author: &'a str,
    /// Issue title (free text; previewed + redacted in the human line).
    pub title: &'a str,
}

/// Inputs for [`emit_wake_received`] (`trigger` / `wake.received`).
#[derive(Clone, Debug)]
pub struct WakeReceived<'a> {
    /// The artifact the wake concerns.
    pub item: &'a WorkItemRef,
    /// Workflow artifact kind (e.g. `intake`).
    pub artifact_kind: &'a str,
    /// Queue the wake routes into (e.g. `raw_intake`).
    pub queue: &'a str,
}

/// Inputs for [`emit_ci_completed`] (`trigger` / `ci.completed`).
#[derive(Clone, Debug)]
pub struct CiCompleted<'a> {
    /// The pull request whose CI finished.
    pub item: &'a WorkItemRef,
    /// CI conclusion token (e.g. `success`, `failure`).
    pub conclusion: &'a str,
    /// Wall-clock CI duration in milliseconds (numeric field; human-formatted).
    pub duration_ms: u64,
}

/// Inputs for [`emit_lease_claimed`] (`worker` / `lease.claimed`).
#[derive(Clone, Debug)]
pub struct LeaseClaimed<'a> {
    /// The leased work item.
    pub item: &'a WorkItemRef,
    /// Role that claimed the lease.
    pub role: &'a str,
    /// Human TTL rendering for the message (e.g. `10m`).
    pub ttl_human: &'a str,
    /// Numeric lease TTL in milliseconds (machine field).
    pub ttl_ms: u64,
    /// What the holder is about to run (e.g. `mark_untriaged`).
    pub running: &'a str,
}

/// Inputs for [`emit_lease_released`] (`worker` / `lease.released`).
#[derive(Clone, Debug)]
pub struct LeaseReleased<'a> {
    /// The work item whose lease was released.
    pub item: &'a WorkItemRef,
    /// Role that held the lease.
    pub role: &'a str,
}

/// Inputs for [`emit_lease_lost`] (`worker` / `lease.lost`).
#[derive(Clone, Debug)]
pub struct LeaseLost<'a> {
    /// The work item whose lease was lost.
    pub item: &'a WorkItemRef,
    /// Role that held the (now lost) lease.
    pub role: &'a str,
    /// Why the lease was lost (e.g. `expired (held 11m)`).
    pub reason: &'a str,
}

/// Inputs for [`emit_agent_started`] (`agent` / `agent.started`).
#[derive(Clone, Debug)]
pub struct AgentStarted<'a> {
    /// The work item the agent run is for.
    pub item: &'a WorkItemRef,
    /// Role driving the run (e.g. `architect`).
    pub role: &'a str,
    /// Run kind (e.g. `triage`, `coding`).
    pub kind: &'a str,
    /// One-line detail of what the run is doing.
    pub detail: &'a str,
}

/// Typed outcome of an `agent.finished` boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentTerminalStatus {
    Succeeded,
    Failed,
    Cancelled,
}

impl AgentTerminalStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    const fn human_verb(self) -> &'static str {
        match self {
            Self::Succeeded => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Inputs for [`emit_agent_finished`] (`agent` / `agent.finished`).
#[derive(Clone, Debug)]
pub struct AgentFinished<'a> {
    /// The work item the agent run was for.
    pub item: &'a WorkItemRef,
    /// Role that drove the run.
    pub role: &'a str,
    /// Run kind (e.g. `triage`, `coding`).
    pub kind: &'a str,
    /// Typed terminal status used by both structured and human projections.
    pub status: AgentTerminalStatus,
    /// Typed agent-machine terminal reason, when the run reached that boundary.
    pub terminal_reason: Option<AgentTerminalReasonV1>,
    /// Run duration in milliseconds (numeric field; human-formatted).
    pub duration_ms: u64,
    /// One-line result summary (free text; previewed + redacted).
    pub summary: &'a str,
}

/// Inputs for [`emit_transition_applied`] (`engine` / `transition.applied`).
#[derive(Clone, Debug)]
pub struct TransitionApplied<'a> {
    /// The work item the transition was applied to.
    pub item: &'a WorkItemRef,
    /// Transition id (e.g. `triage_intake_to_code`).
    pub transition: &'a str,
    /// Optional human detail segment (e.g. `body rewritten as code spec`).
    pub detail: &'a str,
    /// Label delta in §7 form (e.g. `-untriaged +code +ready`); may be empty.
    pub labels_delta: &'a str,
}

/// Inputs for [`emit_queue_entered`] (`engine` / `queue.entered`).
#[derive(Clone, Debug)]
pub struct QueueEntered<'a> {
    /// The work item that moved queues.
    pub item: &'a WorkItemRef,
    /// Destination queue (the `queue.to` field, e.g. `code_ready`).
    pub queue_to: &'a str,
    /// Optional trailing note (e.g. `awaiting engineer`); may be empty.
    pub note: &'a str,
}

/// Inputs for [`emit_gate_evaluated`] (`engine` / `gate.evaluated`).
#[derive(Clone, Debug)]
pub struct GateEvaluated<'a> {
    /// The pull request whose gates were evaluated.
    pub item: &'a WorkItemRef,
    /// Pre-joined gate summary (e.g. `ci_gate=pending dependency_gate=ok`).
    pub gates: &'a str,
    /// Optional trailing note (e.g. `waiting on CI`); may be empty.
    pub note: &'a str,
}

/// Structured implementation PR handoff facts attached to `pr.opened` and
/// `pr.updated` events when a worker-authored handoff is applied.
#[derive(Clone, Debug)]
pub struct PrHandoffFacts<'a> {
    /// Source issue/work item (`source_artifact` join key, e.g. `acme/widgets#42`).
    pub source_artifact: Cow<'a, str>,
    /// Confirmed PR title after the create/update.
    pub title: Cow<'a, str>,
    /// Source of the confirmed title (`agent` or a fallback descriptor).
    pub title_source: Cow<'a, str>,
    /// Source of the confirmed body/report (`agent` or a fallback descriptor).
    pub body_source: Cow<'a, str>,
    /// Workflow metadata kind stamped on the PR body (e.g. `implementation_pr`).
    pub metadata_kind: Cow<'a, str>,
    /// Workflow metadata parent/source reference (e.g. `acme/widgets#42`).
    pub metadata_parent_ref: Cow<'a, str>,
    /// Workflow correlation key (e.g. `pr-for-code-42`).
    pub correlation_key: Cow<'a, str>,
    /// Result application action (`created` or `refreshed`).
    pub action: Cow<'a, str>,
}

/// Inputs for [`emit_pr_opened`] (`engine` / `pr.opened`).
#[derive(Clone, Debug)]
pub struct PrOpened<'a> {
    /// The newly opened pull request.
    pub item: &'a WorkItemRef,
    /// PR title (free text; previewed + redacted in the human message).
    pub title: &'a str,
    /// PR kind (e.g. `implementation`).
    pub kind: &'a str,
    /// Issue number the PR implements (the `for #n` alias).
    pub for_issue: u64,
    /// Optional implementation handoff facts when the PR was opened from a
    /// worker-authored success result.
    pub handoff: Option<PrHandoffFacts<'a>>,
}

/// Inputs for [`emit_pr_updated`] (`engine` / `pr.updated`).
#[derive(Clone, Debug)]
pub struct PrUpdated<'a> {
    /// The updated pull request.
    pub item: &'a WorkItemRef,
    /// PR kind (e.g. `implementation`).
    pub kind: &'a str,
    /// Issue number the PR implements (the `for #n` alias).
    pub for_issue: u64,
    /// Implementation handoff facts confirmed after the update.
    pub handoff: PrHandoffFacts<'a>,
}

/// Inputs for [`emit_pr_merged`] (`engine` / `pr.merged`).
#[derive(Clone, Debug)]
pub struct PrMerged<'a> {
    /// The merged pull request.
    pub item: &'a WorkItemRef,
    /// Target branch (e.g. `main`).
    pub target_branch: &'a str,
    /// Merge method (e.g. `squash`).
    pub merge_method: &'a str,
    /// Short merge commit sha (e.g. `e3f9a1c`).
    pub commit: &'a str,
    /// Label delta (e.g. `+landed`); may be empty.
    pub labels_delta: &'a str,
}

/// Inputs for [`emit_item_resolved`] (`engine` / `item.resolved`).
#[derive(Clone, Debug)]
pub struct ItemResolved<'a> {
    /// The resolved work item.
    pub item: &'a WorkItemRef,
    /// Resolution note (e.g. `implemented by PR#44`).
    pub resolution: &'a str,
    /// Lifecycle start state (e.g. `intake`).
    pub from_state: &'a str,
    /// Lifecycle end state (e.g. `landed`).
    pub to_state: &'a str,
    /// End-to-end duration in milliseconds (numeric field; human-formatted).
    pub duration_ms: u64,
}

/// Inputs for [`emit_forge_context_read`] (`engine` / `forge.context.read`).
#[derive(Clone, Debug)]
pub struct ForgeContextRead<'a> {
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub role: &'a str,
    pub operation: &'a str,
    pub repository: &'a str,
    pub item_number: u64,
    pub status: &'a str,
    pub result_count: usize,
    pub truncated: bool,
    pub duration_ms: u64,
}

/// Inputs for [`emit_role_saturated`] (`worker` / `role.saturated`).
///
/// A *global* line about a shared resource: it carries no subject tag but names
/// the cross-repo wait queue. `waiting` holds the `artifact.ref` strings of the
/// queued items (use [`WorkItemRef::to_string`] to build them).
#[derive(Clone, Debug)]
pub struct RoleSaturated<'a> {
    /// The saturated role (e.g. `architect`).
    pub role: &'a str,
    /// Per-role concurrency limit (the `(concurrency=N)` figure).
    pub concurrency: u64,
    /// `artifact.ref` strings of the items queued behind the holder.
    pub waiting: &'a [String],
}

// ---------------------------------------------------------------------------
// Emit constructors. Each is the single info-level site for its event.
//
// `target:` must be a literal for the tracing macro, so the literal is written
// out per function rather than via Service::target(); the matching Service is
// still named in `service =` for the machine field and is asserted-consistent in
// tests.
// ---------------------------------------------------------------------------

/// Prepends the service's fixed-width human prefix to a rendered body line.
///
/// The `human::*` renderers intentionally return the prefix-less §7 body so they
/// stay unit-testable string-by-string; the `engine:  ` / `worker:  ` /
/// `agent:   ` / `trigger: ` prefix is an emit-site concern (§2/§7's "service
/// prefix on every line, padded so the message column aligns"). Only the
/// positional human MESSAGE gets the prefix — the structured `service=` field
/// stays the bare token.
fn prefixed(service: Service, body: String) -> String {
    format!("{}{body}", service.human_prefix())
}

/// Emits `trigger` / `issue.opened`.
pub fn emit_issue_opened(ev: IssueOpened<'_>) {
    let message = prefixed(
        Service::Trigger,
        human::issue_opened(ev.item, ev.author, ev.title),
    );
    tracing::info!(
        target: "temper::trigger",
        service = Service::Trigger.as_str(),
        event = Event::IssueOpened.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.item.kind().as_str(),
        author = ev.author,
        "{message}",
    );
}

/// Emits `trigger` / `wake.received`.
pub fn emit_wake_received(ev: WakeReceived<'_>) {
    let message = prefixed(
        Service::Trigger,
        human::wake_received(ev.item, ev.artifact_kind, ev.queue),
    );
    tracing::info!(
        target: "temper::trigger",
        service = Service::Trigger.as_str(),
        event = Event::WakeReceived.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.artifact_kind,
        queue.to = ev.queue,
        "{message}",
    );
}

/// Emits `trigger` / `ci.completed`.
pub fn emit_ci_completed(ev: CiCompleted<'_>) {
    let message = prefixed(
        Service::Trigger,
        human::ci_completed(ev.item, ev.conclusion, ev.duration_ms),
    );
    tracing::info!(
        target: "temper::trigger",
        service = Service::Trigger.as_str(),
        event = Event::CiCompleted.as_str(),
        repo = ev.item.repo(),
        pr.ref = %ev.item,
        artifact.ref = %ev.item,
        conclusion = ev.conclusion,
        duration_ms = ev.duration_ms,
        "{message}",
    );
}

/// Emits `worker` / `lease.claimed`.
pub fn emit_lease_claimed(ev: LeaseClaimed<'_>) {
    let message = prefixed(
        Service::Worker,
        human::lease_claimed(ev.item, ev.role, ev.ttl_human, ev.running),
    );
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        event = Event::LeaseClaimed.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        role = ev.role,
        ttl_ms = ev.ttl_ms,
        running = ev.running,
        "{message}",
    );
}

/// Emits `worker` / `lease.released`.
pub fn emit_lease_released(ev: LeaseReleased<'_>) {
    let message = prefixed(Service::Worker, human::lease_released(ev.item, ev.role));
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        event = Event::LeaseReleased.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        role = ev.role,
        "{message}",
    );
}

/// Emits `worker` / `lease.lost`.
pub fn emit_lease_lost(ev: LeaseLost<'_>) {
    let message = prefixed(
        Service::Worker,
        human::lease_lost(ev.item, ev.role, ev.reason),
    );
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        event = Event::LeaseLost.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        role = ev.role,
        reason = ev.reason,
        "{message}",
    );
}

/// Emits `agent` / `agent.started`.
pub fn emit_agent_started(ev: AgentStarted<'_>) {
    let message = prefixed(
        Service::Agent,
        human::agent_started(ev.item, ev.role, ev.kind, ev.detail),
    );
    tracing::info!(
        target: "temper::agent",
        service = Service::Agent.as_str(),
        event = Event::AgentStarted.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        role = ev.role,
        kind = ev.kind,
        "{message}",
    );
}

/// Emits `agent` / `agent.finished`.
pub fn emit_agent_finished(ev: AgentFinished<'_>) {
    let reason = ev.terminal_reason.map(AgentTerminalReasonV1::as_str);
    let message = prefixed(
        Service::Agent,
        human::agent_finished(
            ev.item,
            ev.role,
            ev.kind,
            ev.status,
            ev.terminal_reason,
            ev.duration_ms,
            ev.summary,
        ),
    );
    tracing::info!(
        target: "temper::agent",
        service = Service::Agent.as_str(),
        event = Event::AgentFinished.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        role = ev.role,
        kind = ev.kind,
        status = ev.status.as_str(),
        reason = reason.unwrap_or(""),
        duration_ms = ev.duration_ms,
        "{message}",
    );
}

/// Emits `engine` / `transition.applied`.
pub fn emit_transition_applied(ev: TransitionApplied<'_>) {
    let message = prefixed(
        Service::Engine,
        human::transition_applied(ev.item, ev.transition, ev.detail, ev.labels_delta),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::TransitionApplied.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        transition = ev.transition,
        labels.delta = ev.labels_delta,
        "{message}",
    );
}

/// Emits `engine` / `queue.entered`.
pub fn emit_queue_entered(ev: QueueEntered<'_>) {
    let message = prefixed(
        Service::Engine,
        human::queue_entered(ev.item, ev.queue_to, ev.note),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::QueueEntered.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        queue.to = ev.queue_to,
        "{message}",
    );
}

/// Emits `engine` / `gate.evaluated`.
pub fn emit_gate_evaluated(ev: GateEvaluated<'_>) {
    let message = prefixed(
        Service::Engine,
        human::gate_evaluated(ev.item, ev.gates, ev.note),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::GateEvaluated.as_str(),
        repo = ev.item.repo(),
        pr.ref = %ev.item,
        artifact.ref = %ev.item,
        gates = ev.gates,
        "{message}",
    );
}

/// Emits `engine` / `pr.opened`.
pub fn emit_pr_opened(ev: PrOpened<'_>) {
    let message = prefixed(
        Service::Engine,
        human::pr_opened(ev.item, ev.title, ev.kind, ev.for_issue),
    );
    let (
        source_artifact,
        pr_title,
        title_source,
        body_source,
        metadata_kind,
        metadata_parent_ref,
        correlation_key,
        action,
    ) = handoff_fields(ev.handoff.as_ref());
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::PrOpened.as_str(),
        repo = ev.item.repo(),
        pr.ref = %ev.item,
        artifact.ref = %ev.item,
        kind = ev.kind,
        for_issue = ev.for_issue,
        source_artifact = source_artifact,
        pr.title = pr_title,
        title.source = title_source,
        body.source = body_source,
        metadata.kind = metadata_kind,
        metadata.parent.ref = metadata_parent_ref,
        correlation.key = correlation_key,
        action = action,
        "{message}",
    );
}

/// Emits `engine` / `pr.updated`.
pub fn emit_pr_updated(ev: PrUpdated<'_>) {
    let message = prefixed(
        Service::Engine,
        human::pr_updated(
            ev.item,
            ev.handoff.title.as_ref(),
            ev.kind,
            ev.for_issue,
            ev.handoff.action.as_ref(),
        ),
    );
    let handoff = Some(&ev.handoff);
    let (
        source_artifact,
        pr_title,
        title_source,
        body_source,
        metadata_kind,
        metadata_parent_ref,
        correlation_key,
        action,
    ) = handoff_fields(handoff);
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::PrUpdated.as_str(),
        repo = ev.item.repo(),
        pr.ref = %ev.item,
        artifact.ref = %ev.item,
        kind = ev.kind,
        for_issue = ev.for_issue,
        source_artifact = source_artifact,
        pr.title = pr_title,
        title.source = title_source,
        body.source = body_source,
        metadata.kind = metadata_kind,
        metadata.parent.ref = metadata_parent_ref,
        correlation.key = correlation_key,
        action = action,
        "{message}",
    );
}

fn handoff_fields<'a>(
    handoff: Option<&'a PrHandoffFacts<'a>>,
) -> (
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
) {
    handoff.map_or(("", "", "", "", "", "", "", ""), |handoff| {
        (
            handoff.source_artifact.as_ref(),
            handoff.title.as_ref(),
            handoff.title_source.as_ref(),
            handoff.body_source.as_ref(),
            handoff.metadata_kind.as_ref(),
            handoff.metadata_parent_ref.as_ref(),
            handoff.correlation_key.as_ref(),
            handoff.action.as_ref(),
        )
    })
}

/// Emits `engine` / `pr.merged`.
pub fn emit_pr_merged(ev: PrMerged<'_>) {
    let message = prefixed(
        Service::Engine,
        human::pr_merged(
            ev.item,
            ev.target_branch,
            ev.merge_method,
            ev.commit,
            ev.labels_delta,
        ),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::PrMerged.as_str(),
        repo = ev.item.repo(),
        pr.ref = %ev.item,
        artifact.ref = %ev.item,
        target_branch = ev.target_branch,
        merge_method = ev.merge_method,
        commit = ev.commit,
        labels.delta = ev.labels_delta,
        "{message}",
    );
}

/// Emits `engine` / `item.resolved`.
pub fn emit_item_resolved(ev: ItemResolved<'_>) {
    let message = prefixed(
        Service::Engine,
        human::item_resolved(
            ev.item,
            ev.resolution,
            ev.from_state,
            ev.to_state,
            ev.duration_ms,
        ),
    );
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ItemResolved.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        resolution = ev.resolution,
        duration_ms = ev.duration_ms,
        "{message}",
    );
}

/// Emits the debug-level audit record for one bounded Forge context read.
pub fn emit_forge_context_read(ev: ForgeContextRead<'_>) {
    tracing::debug!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ForgeContextRead.as_str(),
        worker_id = ev.worker_id,
        job_id = ev.job_id,
        role = ev.role,
        operation = ev.operation,
        repo = ev.repository,
        item_number = ev.item_number,
        status = ev.status,
        result_count = ev.result_count,
        truncated = ev.truncated,
        duration_ms = ev.duration_ms,
        "engine: forge context read",
    );
}

/// Emits `worker` / `role.saturated` (a global line, no subject tag).
pub fn emit_role_saturated(ev: RoleSaturated<'_>) {
    let message = prefixed(
        Service::Worker,
        human::role_saturated(ev.role, ev.concurrency, ev.waiting),
    );
    let queued = ev.waiting.join(",");
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        event = Event::RoleSaturated.as_str(),
        role = ev.role,
        concurrency = ev.concurrency,
        queued = %queued,
        queued_count = ev.waiting.len(),
        "{message}",
    );
}

// ---------------------------------------------------------------------------
// Startup / health lines (§7's boot block). These carry no work-item identity;
// they are simple info-string helpers on the relevant service target. They
// still set `service` so the JSON projection can filter by plane.
// ---------------------------------------------------------------------------

/// Emits an `engine` startup/health line (no event vocabulary).
///
/// Use for the boot block in §7 (`temper 0.9.0 starting`, `config loaded …`,
/// `watching 2 repos: …`, `ready -- watching …, idle`). The `message` should
/// already carry the human body *without* the `engine:  ` prefix.
pub fn emit_engine_status(message: impl AsRef<str>) {
    let message = prefixed(Service::Engine, message.as_ref().to_string());
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        "{message}",
    );
}

/// Emits a `worker` startup/health line (e.g. the `capacity:` line).
pub fn emit_worker_status(message: impl AsRef<str>) {
    let message = prefixed(Service::Worker, message.as_ref().to_string());
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        "{message}",
    );
}

/// Emits a `trigger` startup/health line (e.g. the webhook-listener line).
pub fn emit_trigger_status(message: impl AsRef<str>) {
    let message = prefixed(Service::Trigger, message.as_ref().to_string());
    tracing::info!(
        target: "temper::trigger",
        service = Service::Trigger.as_str(),
        "{message}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-event service the emit site hard-codes in its `target:` literal
    /// must match the `Service` whose `as_str()` it writes into `service=`. We
    /// can't read the literal back at runtime, so this guards the *mapping* the
    /// design fixes (§2): the spec assigns each event to exactly one plane.
    #[test]
    fn event_service_mapping_is_the_spec_assignment() {
        // (Event, Service) pairs straight from §2's "Example events" column.
        let mapping = [
            (Event::IssueOpened, Service::Trigger),
            (Event::WakeReceived, Service::Trigger),
            (Event::CiCompleted, Service::Trigger),
            (Event::LeaseClaimed, Service::Worker),
            (Event::LeaseReleased, Service::Worker),
            (Event::LeaseLost, Service::Worker),
            (Event::ForgeContextRead, Service::Engine),
            (Event::RoleSaturated, Service::Worker),
            (Event::AgentStarted, Service::Agent),
            (Event::AgentFinished, Service::Agent),
            (Event::AgentToolConfigured, Service::Agent),
            (Event::AgentToolExposed, Service::Agent),
            (Event::AgentToolHidden, Service::Agent),
            (Event::McpServerStarted, Service::Agent),
            (Event::McpToolCalled, Service::Agent),
            (Event::McpToolResult, Service::Agent),
            (Event::WorkspaceDiffProduced, Service::Worker),
            (Event::TransitionApplied, Service::Engine),
            (Event::QueueEntered, Service::Engine),
            (Event::GateEvaluated, Service::Engine),
            (Event::PrOpened, Service::Engine),
            (Event::PrUpdated, Service::Engine),
            (Event::PrMerged, Service::Engine),
            (Event::ItemResolved, Service::Engine),
        ];
        // Every event in the catalog is mapped exactly once.
        assert_eq!(mapping.len(), Event::ALL.len());
        for event in Event::ALL {
            assert_eq!(
                mapping.iter().filter(|(e, _)| *e == event).count(),
                1,
                "{event:?} is not mapped to exactly one service"
            );
        }
    }
}
