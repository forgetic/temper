// SPDX-License-Identifier: MPL-2.0

//! Safe structured projections for durable bounded model-recovery decisions.

use temper_protocol_activity::ModelFailureV1;

use crate::{Event, Service, WorkItemRef};

/// Typed fields shared by the worker rotation and engine parking projections.
///
/// Strings are the already-validated identities/location from the worker
/// protocol. The model diagnostic is normalized again at this final logging
/// boundary so malformed or sensitive detail fails closed.
#[derive(Clone, Debug)]
pub struct ModelRecoveryDecision<'a> {
    pub attempt_id: &'a str,
    pub failure_epoch: u32,
    pub failure_count: u32,
    pub action: &'a str,
    pub current_session_id: &'a str,
    pub prior_session_id: Option<&'a str>,
    pub new_session_id: Option<&'a str>,
    pub evidence_location: &'a str,
    pub model_failure: &'a ModelFailureV1,
}

/// Inputs for `worker` / `model.session.rotated`.
#[derive(Clone, Debug)]
pub struct ModelSessionRotated<'a> {
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub decision: ModelRecoveryDecision<'a>,
}

/// Inputs for `engine` / `model.failure.parked`.
#[derive(Clone, Debug)]
pub struct ModelFailureParked<'a> {
    pub item: &'a WorkItemRef,
    pub worker_id: &'a str,
    pub job_id: &'a str,
    pub decision: ModelRecoveryDecision<'a>,
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
        action = ev.decision.action,
        current_session_id = ev.decision.current_session_id,
        prior_session_id = ev.decision.prior_session_id,
        new_session_id = ev.decision.new_session_id,
        evidence_location = ev.decision.evidence_location,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        model_failure_message = failure.message.as_str(),
        "worker:  durable model failure rotated the agent session"
    );
}

/// Emits the engine decision after the human-attention park projection has
/// converged. The decision itself was already durable in the worker ledger and
/// carried in the accepted result; activity forwarding is not consulted.
pub fn emit_model_failure_parked(ev: ModelFailureParked<'_>) {
    let mut failure = ev.decision.model_failure.clone();
    failure.normalize();
    let status = failure.http_status.map(u64::from);
    let message = format!(
        "{}{} bounded model recovery exhausted; parked for operator action",
        Service::Engine.human_prefix(),
        ev.item.human_tag(),
    );
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
        action = ev.decision.action,
        current_session_id = ev.decision.current_session_id,
        prior_session_id = ev.decision.prior_session_id,
        new_session_id = ev.decision.new_session_id,
        evidence_location = ev.decision.evidence_location,
        provider = failure.provider.as_str(),
        model = failure.model.as_str(),
        category = failure.category.as_str(),
        retryable = failure.retryable,
        http_status = status,
        provider_request_id = failure.provider_request_id.as_deref(),
        provider_error_code = failure.provider_error_code.as_deref(),
        detail_redacted = failure.detail_redacted,
        model_failure_message = failure.message.as_str(),
        "{message}"
    );
}
