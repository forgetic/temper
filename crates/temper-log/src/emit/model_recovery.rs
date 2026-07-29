// SPDX-License-Identifier: MPL-2.0

//! Safe structured projections for bounded model-recovery decisions.

use temper_protocol_activity::ModelFailureV1;

use crate::{Event, Service, WorkItemRef};

/// Typed fields shared by worker and engine recovery projections.
///
/// The diagnostic and decision are already normalized and validated by their
/// authoritative boundaries. This final logging boundary normalizes the
/// diagnostic again, and has no field capable of carrying provider prose,
/// prompts, response bodies, stderr, or credentials.
#[derive(Clone, Debug)]
pub struct ModelRecoveryDecision<'a> {
    pub attempt_id: &'a str,
    pub failure_epoch: u32,
    pub failure_count: u32,
    pub session_number: u32,
    pub session_failure_count: u32,
    pub elapsed_ms: u64,
    pub action: &'a str,
    pub deferral_count: u32,
    pub generation: u32,
    pub current_session_id: &'a str,
    pub prior_session_id: Option<&'a str>,
    pub new_session_id: Option<&'a str>,
    pub evidence_location: &'a str,
    pub model_failure: &'a ModelFailureV1,
}

/// Inputs for `agent` / `model.turn.retrying`.
#[derive(Clone, Debug)]
pub struct ModelTurnRetrying<'a> {
    pub scope: &'a str,
    pub scope_id: &'a str,
    pub call_id: &'a str,
    pub attempt: u32,
    pub next_attempt: u32,
    pub delay_ms: u64,
    pub duration_ms: u64,
    pub model_failure: &'a ModelFailureV1,
}

/// Inputs for `worker` / `model.session.rotated`.
#[derive(Clone, Debug)]
pub struct ModelSessionRotated<'a> {
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub decision: ModelRecoveryDecision<'a>,
}

/// Inputs for `engine` / `model.provider.deferred`.
#[derive(Clone, Debug)]
pub struct ModelProviderDeferred<'a> {
    pub item: &'a WorkItemRef,
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub workstream_id: &'a str,
    pub decision: ModelRecoveryDecision<'a>,
}

/// Inputs for `engine` / `model.provider.wake`.
#[derive(Clone, Debug)]
pub struct ModelProviderWake<'a> {
    pub item: &'a WorkItemRef,
    pub workstream_id: &'a str,
    pub failure_epoch: u32,
    pub failure_count: u32,
    pub elapsed_ms: u64,
    pub deferral_count: u32,
    pub generation: u32,
    pub action: &'a str,
    pub event_id: &'a str,
    pub disposition: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub category: &'a str,
    pub boundary: &'a str,
    pub event_kind: &'a str,
    pub status_present: bool,
    pub code_present: bool,
    pub http_status: Option<u16>,
    pub provider_request_id: Option<&'a str>,
    pub provider_error_code: Option<&'a str>,
}

/// Inputs for `engine` / `model.recovery.cleared`.
#[derive(Clone, Debug)]
pub struct ModelRecoveryCleared<'a> {
    pub item: &'a WorkItemRef,
    pub workstream_id: &'a str,
    pub failure_epoch: u32,
    pub failure_count: u32,
    pub elapsed_ms: u64,
    pub generation: u32,
}

/// Inputs for `engine` / `model.failure.parked`.
#[derive(Clone, Debug)]
pub struct ModelFailureParked<'a> {
    pub item: &'a WorkItemRef,
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub decision: ModelRecoveryDecision<'a>,
}

/// Emits the immediate retry of only the failed side-effect-free model request.
pub fn emit_model_turn_retrying(ev: ModelTurnRetrying<'_>) {
    let mut failure = ev.model_failure.clone();
    failure.normalize();
    let status = failure.http_status.map(u64::from);
    tracing::debug!(
        target: "temper::agent",
        service = Service::Agent.as_str(),
        event = Event::ModelTurnRetrying.as_str(),
        scope = ev.scope,
        scope_id = ev.scope_id,
        call_id = ev.call_id,
        attempt = ev.attempt,
        next_attempt = ev.next_attempt,
        delay_ms = ev.delay_ms,
        duration_ms = ev.duration_ms,
        disposition = failure.disposition.as_str(),
        final_disposition = failure.disposition.as_str(),
        boundary = failure.boundary.as_str(),
        event_kind = failure.event_kind.as_str(),
        status_present = failure.status_present,
        code_present = failure.code_present,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        "agent: retrying failed model turn after bounded backoff"
    );
}

/// Emits the durable worker-owned session rotation decision.
pub fn emit_model_session_rotated(ev: ModelSessionRotated<'_>) {
    let mut failure = ev.decision.model_failure.clone();
    failure.normalize();
    let status = failure.http_status.map(u64::from);
    tracing::info!(
        target: "temper::worker",
        service = Service::Worker.as_str(),
        event = Event::ModelSessionRotated.as_str(),
        worker_id = ev.worker_id,
        job_id = ev.job_id,
        attempt_id = ev.decision.attempt_id,
        failure_epoch = ev.decision.failure_epoch,
        failure_count = ev.decision.failure_count,
        cumulative_failure_count = ev.decision.failure_count,
        session_number = ev.decision.session_number,
        session_failure_count = ev.decision.session_failure_count,
        elapsed_ms = ev.decision.elapsed_ms,
        action = ev.decision.action,
        deferral_count = ev.decision.deferral_count,
        generation = ev.decision.generation,
        current_session_id = ev.decision.current_session_id,
        prior_session_id = ev.decision.prior_session_id,
        new_session_id = ev.decision.new_session_id,
        evidence_location = ev.decision.evidence_location,
        disposition = failure.disposition.as_str(),
        final_disposition = failure.disposition.as_str(),
        boundary = failure.boundary.as_str(),
        event_kind = failure.event_kind.as_str(),
        status_present = failure.status_present,
        code_present = failure.code_present,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        "worker: durable model failure rotated the agent session"
    );
}

/// Emits automatic provider deferral after its durable Forge marker converges.
pub fn emit_model_provider_deferred(ev: ModelProviderDeferred<'_>) {
    let mut failure = ev.decision.model_failure.clone();
    failure.normalize();
    let status = failure.http_status.map(u64::from);
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ModelProviderDeferred.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.item.kind().as_str(),
        worker_id = ev.worker_id,
        job_id = ev.job_id,
        workstream_id = ev.workstream_id,
        attempt_id = ev.decision.attempt_id,
        failure_epoch = ev.decision.failure_epoch,
        failure_count = ev.decision.failure_count,
        cumulative_failure_count = ev.decision.failure_count,
        session_number = ev.decision.session_number,
        session_failure_count = ev.decision.session_failure_count,
        elapsed_ms = ev.decision.elapsed_ms,
        action = ev.decision.action,
        deferral_count = ev.decision.deferral_count,
        generation = ev.decision.generation,
        current_session_id = ev.decision.current_session_id,
        prior_session_id = ev.decision.prior_session_id,
        evidence_location = ev.decision.evidence_location,
        disposition = failure.disposition.as_str(),
        final_disposition = failure.disposition.as_str(),
        boundary = failure.boundary.as_str(),
        event_kind = failure.event_kind.as_str(),
        status_present = failure.status_present,
        code_present = failure.code_present,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        "engine: provider recovery deferred automatically without human parking"
    );
}

/// Emits an authenticated provider-health wake after its durable generation advances.
pub fn emit_model_provider_wake(ev: ModelProviderWake<'_>) {
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ModelProviderWake.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.item.kind().as_str(),
        workstream_id = ev.workstream_id,
        failure_epoch = ev.failure_epoch,
        failure_count = ev.failure_count,
        cumulative_failure_count = ev.failure_count,
        elapsed_ms = ev.elapsed_ms,
        deferral_count = ev.deferral_count,
        action = ev.action,
        generation = ev.generation,
        health_event_id = ev.event_id,
        disposition = ev.disposition,
        final_disposition = ev.disposition,
        provider = ev.provider,
        model = ev.model,
        category = ev.category,
        boundary = ev.boundary,
        event_kind = ev.event_kind,
        status_present = ev.status_present,
        code_present = ev.code_present,
        http_status = ev.http_status.map(u64::from),
        provider_request_id = ev.provider_request_id,
        provider_error_code = ev.provider_error_code,
        "engine: authenticated provider-health signal advanced deferred recovery"
    );
}

/// Emits successful clearing of a durable provider-recovery fence.
pub fn emit_model_recovery_cleared(ev: ModelRecoveryCleared<'_>) {
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ModelRecoveryCleared.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.item.kind().as_str(),
        workstream_id = ev.workstream_id,
        failure_epoch = ev.failure_epoch,
        failure_count = ev.failure_count,
        cumulative_failure_count = ev.failure_count,
        elapsed_ms = ev.elapsed_ms,
        generation = ev.generation,
        action = "success_clear",
        final_disposition = "succeeded",
        "engine: authoritative success cleared durable provider recovery"
    );
}

/// Emits genuinely actionable human parking after its Forge projection converges.
pub fn emit_model_failure_parked(ev: ModelFailureParked<'_>) {
    let mut failure = ev.decision.model_failure.clone();
    failure.normalize();
    let status = failure.http_status.map(u64::from);
    tracing::info!(
        target: "temper::engine",
        service = Service::Engine.as_str(),
        event = Event::ModelFailureParked.as_str(),
        repo = ev.item.repo(),
        artifact.ref = %ev.item,
        artifact.kind = ev.item.kind().as_str(),
        worker_id = ev.worker_id,
        job_id = ev.job_id,
        attempt_id = ev.decision.attempt_id,
        failure_epoch = ev.decision.failure_epoch,
        failure_count = ev.decision.failure_count,
        cumulative_failure_count = ev.decision.failure_count,
        session_number = ev.decision.session_number,
        session_failure_count = ev.decision.session_failure_count,
        elapsed_ms = ev.decision.elapsed_ms,
        action = ev.decision.action,
        deferral_count = ev.decision.deferral_count,
        generation = ev.decision.generation,
        current_session_id = ev.decision.current_session_id,
        prior_session_id = ev.decision.prior_session_id,
        evidence_location = ev.decision.evidence_location,
        disposition = failure.disposition.as_str(),
        final_disposition = failure.disposition.as_str(),
        boundary = failure.boundary.as_str(),
        event_kind = failure.event_kind.as_str(),
        status_present = failure.status_present,
        code_present = failure.code_present,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        "engine: bounded model recovery parked for actionable operator repair"
    );
}
