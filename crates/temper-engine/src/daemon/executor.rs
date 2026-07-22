// SPDX-License-Identifier: MPL-2.0

//! The daemon's imperative shell: performs each machine request on the engine
//! runtime and feeds the resulting completions back into the queue.

use std::sync::Arc;
use std::time::Instant;

use temper_engine_io::{CqSender, Executor as EngineExecutor, Spawner, arm_timer};
use temper_protocol_worker::{
    ContextResponse, ForgeContextErrorCode, ForgeContextOperation, ForgeContextResult,
    WORKER_PROTOCOL_VERSION, WorkerActivityAcknowledgement, WorkerProtocolMessage,
};
use tracing::Instrument;

use crate::applier::{ApplyOutcome, ResultApplier};

use super::WakeExecutor;
use super::machine::{DaemonCompletion, DaemonMachine, DaemonRequest};
use super::wake_coordinator::WakeOutcome;

pub(super) struct DaemonExecutor {
    pub(super) spawner: Arc<dyn Spawner>,
    pub(super) cq: CqSender<DaemonCompletion>,
    pub(super) applier: Arc<dyn ResultApplier>,
    pub(super) wake_executor_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeExecutor>>>>,
    pub(super) context_reader_slot:
        Arc<std::sync::Mutex<Option<Arc<dyn super::context_reader::ContextReader>>>>,
    pub(super) trace_query_slot:
        Arc<std::sync::Mutex<Option<crate::trace_query::TraceQueryService>>>,
    pub(super) trace_journal_slot: Arc<std::sync::Mutex<Option<crate::AgentTraceJournal>>>,
}

fn context_operation_name(operation: &ForgeContextOperation) -> &'static str {
    match operation {
        ForgeContextOperation::ForgeGetItem(_) => "forge_get_item",
        ForgeContextOperation::ForgeListRelated(_) => "forge_list_related",
    }
}

fn context_error_name(code: ForgeContextErrorCode) -> &'static str {
    match code {
        ForgeContextErrorCode::InvalidRequest => "invalid_request",
        ForgeContextErrorCode::NotAuthorized => "not_authorized",
        ForgeContextErrorCode::NotFound => "not_found",
        ForgeContextErrorCode::ForgeUnavailable => "forge_unavailable",
        ForgeContextErrorCode::LimitExceeded => "limit_exceeded",
    }
}

fn context_result_metrics(
    result: &Result<ForgeContextResult, ForgeContextErrorCode>,
) -> (&'static str, usize, bool) {
    match result {
        Ok(ForgeContextResult::Item(item)) => ("success", 1, item.truncation.is_truncated()),
        Ok(ForgeContextResult::Related(related)) => (
            "success",
            related.items.len(),
            related.truncation.is_truncated(),
        ),
        Err(code) => (context_error_name(*code), 0, false),
    }
}

fn apply_outcome_name(outcome: &ApplyOutcome) -> &'static str {
    match outcome {
        ApplyOutcome::Applied => "applied",
        ApplyOutcome::RetryReleased => "retry_released",
        ApplyOutcome::ConvergencePending { .. } => "convergence_pending",
        ApplyOutcome::Stale => "stale",
        ApplyOutcome::Retryable { .. } => "retryable",
        ApplyOutcome::Rejected { .. } => "rejected",
    }
}

fn emit_wake_measurement(measurement: &super::machine::WakeMeasurement) {
    let run_id = measurement.run_id.as_deref().unwrap_or("");
    let error = measurement.error.as_deref().unwrap_or("");
    if let Some(role) = measurement.role.as_deref() {
        tracing::debug!(
            target: "temper::engine",
            service = "engine",
            repo = %measurement.repo,
            role,
            wake.run_id = run_id,
            wake.reason = %measurement.reason,
            wake.scope = %measurement.scope,
            wake.outcome = measurement.outcome,
            wake.phase = measurement.phase,
            wake.pending_target_count = measurement.pending_target_count,
            wake.in_flight_repository_count = measurement.in_flight_repository_count,
            wake.queue_latency_ms = measurement.queue_latency_ms,
            wake.execution_duration_ms = measurement.execution_duration_ms,
            error,
            "engine: wake decision"
        );
    } else {
        tracing::debug!(
            target: "temper::engine",
            service = "engine",
            repo = %measurement.repo,
            wake.run_id = run_id,
            wake.reason = %measurement.reason,
            wake.scope = %measurement.scope,
            wake.outcome = measurement.outcome,
            wake.phase = measurement.phase,
            wake.pending_target_count = measurement.pending_target_count,
            wake.in_flight_repository_count = measurement.in_flight_repository_count,
            wake.queue_latency_ms = measurement.queue_latency_ms,
            wake.execution_duration_ms = measurement.execution_duration_ms,
            error,
            "engine: wake decision"
        );
    }
}

impl EngineExecutor<DaemonMachine> for DaemonExecutor {
    fn execute(&self, request: DaemonRequest) {
        match request {
            DaemonRequest::Respond {
                responder,
                response,
            } => {
                responder.respond(response);
            }
            DaemonRequest::RespondAssignment {
                responder,
                response,
                job,
                context,
            } => {
                if !responder.try_respond(response) {
                    let _ = self
                        .cq
                        .send(DaemonCompletion::AssignmentDeliveryFailed { job, context });
                }
            }
            DaemonRequest::StartPollTimer { id, delay } => {
                arm_timer(&*self.spawner, &self.cq, delay, move || {
                    DaemonCompletion::PollDeadline { id }
                });
            }
            DaemonRequest::StartStartupRecoveryGrace { delay, reply } => {
                arm_timer(&*self.spawner, &self.cq, delay, move || {
                    DaemonCompletion::StartupRecoveryGraceElapsed { reply }
                });
            }
            DaemonRequest::RunApplyAndRespond {
                admission,
                job,
                result,
                recovered_context,
                responder,
            } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                let job_id = job.job_id.clone();
                let span = tracing::debug_span!(
                    target: "temper::engine",
                    "apply",
                    apply.id = %job_id
                );
                self.spawner.spawn_with_cx(move |_cx| {
                    async move {
                        let started = Instant::now();
                        let applied_result = result.clone();
                        let outcome = match recovered_context {
                            Some(context) => {
                                applier.apply_recovered(job, applied_result, context).await
                            }
                            None => applier.apply(job, applied_result).await,
                        };
                        tracing::debug!(
                            target: "temper::engine",
                            service = "engine",
                            apply.id = %job_id,
                            apply.outcome = apply_outcome_name(&outcome),
                            duration_ms = u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                            "engine: result apply finished"
                        );
                        let _ = cq.send(DaemonCompletion::ApplyAndRespondFinished {
                            admission,
                            result,
                            responder,
                            outcome,
                        });
                    }
                    .instrument(span)
                });
            }
            DaemonRequest::RunClaim {
                admission,
                job,
                worker_id,
                daemon_boot_id,
                assign,
                responder,
            } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let outcome = applier
                        .claim(
                            job,
                            crate::applier::ClaimContext {
                                worker_id: worker_id.clone(),
                                daemon_boot_id,
                            },
                        )
                        .await;
                    let _ = cq.send(DaemonCompletion::ClaimFinished {
                        admission,
                        assign,
                        worker_id,
                        responder,
                        outcome,
                    });
                });
            }
            DaemonRequest::RunClaimRollback {
                job,
                context,
                admission,
            } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    applier.release_claim(job, context).await;
                    if let Some(admission) = admission {
                        let _ = cq.send(DaemonCompletion::ClaimRollbackFinished { admission });
                    }
                });
            }
            DaemonRequest::RunRecoveredHeartbeats {
                checks,
                worker_id,
                reports,
                responder,
                response,
            } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let mut outcomes = Vec::with_capacity(checks.len());
                    for check in checks {
                        let outcome = applier.heartbeat(check.job, check.context).await;
                        outcomes.push((check.key, outcome));
                    }
                    let _ = cq.send(DaemonCompletion::RecoveredHeartbeatsFinished {
                        worker_id,
                        reports,
                        outcomes,
                        responder,
                        response,
                    });
                });
            }
            DaemonRequest::RunShutdownRelease { assignments, reply } => {
                let applier = Arc::clone(&self.applier);
                self.spawner.spawn_with_cx(move |_cx| async move {
                    for (job, context) in assignments {
                        applier.release_claim(job, context).await;
                    }
                    reply.send(());
                });
            }
            DaemonRequest::RunPullRequestFreshnessCheck { check, responder } => {
                let applier = Arc::clone(&self.applier);
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let response = applier.check_pull_request_freshness(check).await;
                    responder.respond(temper_engine_io::http::HttpResponseData::json(
                        200,
                        &serde_json::to_value(&response).expect("freshness response serializes"),
                    ));
                });
            }
            DaemonRequest::RunFetchContext {
                admission,
                request,
                role,
                responder,
            } => {
                let reader = self
                    .context_reader_slot
                    .lock()
                    .expect("context reader slot")
                    .clone();
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let started = Instant::now();
                    let result = match reader {
                        Some(reader) => reader.read(request.operation.clone()).await,
                        None => Err(ForgeContextErrorCode::ForgeUnavailable),
                    };
                    let (status, result_count, truncated) = context_result_metrics(&result);
                    temper_log::emit::emit_forge_context_read(temper_log::emit::ForgeContextRead {
                        worker_id: &request.worker_id,
                        job_id: &request.job_id,
                        role: &role,
                        operation: context_operation_name(&request.operation),
                        repository: request.operation.repository(),
                        item_number: request.operation.number(),
                        status,
                        result_count,
                        truncated,
                        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
                    });
                    let response = match result {
                        Ok(result) => ContextResponse::success(&request, result),
                        Err(code) => ContextResponse::error(&request, code),
                    };
                    let _ = cq.send(DaemonCompletion::FetchContextFinished {
                        admission,
                        responder,
                        response: super::protocol::protocol_response(Some(
                            WorkerProtocolMessage::ContextResponse(response),
                        )),
                    });
                });
            }
            DaemonRequest::RunTraceQuery { request, responder } => {
                let service = self
                    .trace_query_slot
                    .lock()
                    .expect("trace query slot")
                    .clone();
                match service {
                    Some(service) => {
                        self.spawner.spawn_with_cx(move |_cx| async move {
                            responder.respond(service.handle(request));
                        });
                    }
                    None => responder.respond(crate::trace_query::disabled_trace_response()),
                }
            }
            DaemonRequest::IngestActivity {
                request,
                binding,
                responder,
            } => {
                let journal = self
                    .trace_journal_slot
                    .lock()
                    .expect("trace journal slot")
                    .clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let Some(journal) = journal else {
                        responder
                            .respond(temper_engine_io::http::HttpResponseData::status_only(503));
                        return;
                    };
                    let run_id = request.batch.run_id.clone();
                    let worker_id = request.worker_id.clone();
                    let batch = request.batch;
                    let outcome =
                        skein::runtime::spawn_blocking(move || journal.ingest(&binding, &batch))
                            .await;
                    match outcome {
                        Ok(acknowledgement) => {
                            responder.respond(super::protocol::protocol_response(Some(
                                WorkerProtocolMessage::ActivityAck(WorkerActivityAcknowledgement {
                                    protocol_version: WORKER_PROTOCOL_VERSION,
                                    worker_id,
                                    acknowledgement,
                                }),
                            )))
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "temper::engine",
                                service = "engine",
                                event = "agent.activity.ingest_failed",
                                run_id,
                                %error,
                                "engine rejected or could not persist an agent activity batch"
                            );
                            responder.respond(
                                temper_engine_io::http::HttpResponseData::status_only(503),
                            );
                        }
                    }
                });
            }
            DaemonRequest::RespondContext {
                response,
                audit,
                responder,
            } => {
                temper_log::emit::emit_forge_context_read(temper_log::emit::ForgeContextRead {
                    worker_id: &audit.worker_id,
                    job_id: &audit.job_id,
                    role: &audit.role,
                    operation: &audit.operation,
                    repository: &audit.repository,
                    item_number: audit.item_number,
                    status: &audit.status,
                    result_count: 0,
                    truncated: false,
                    duration_ms: 0,
                });
                responder.respond(super::protocol::protocol_response(Some(
                    WorkerProtocolMessage::ContextResponse(response),
                )));
            }
            DaemonRequest::StartWakeTimer {
                repo,
                generation,
                delay,
            } => {
                arm_timer(&*self.spawner, &self.cq, delay, move || {
                    DaemonCompletion::WakeTimerElapsed { repo, generation }
                });
            }
            DaemonRequest::RunWake { work } => {
                let executor = self
                    .wake_executor_slot
                    .lock()
                    .expect("wake executor slot")
                    .clone();
                let cq = self.cq.clone();
                match executor {
                    Some(executor) => {
                        let run_id = work.run_id();
                        let repo = format!("{}/{}", work.repo.owner, work.repo.name);
                        let span = tracing::debug_span!(
                            target: "temper::engine",
                            "wake",
                            wake.run_id = %run_id,
                            repo = %repo
                        );
                        self.spawner.spawn_with_cx(move |_cx| {
                            async move {
                                let outcome = executor.run(work.clone()).await;
                                let _ = cq.send(DaemonCompletion::WakeFinished { work, outcome });
                            }
                            .instrument(span)
                        });
                    }
                    None => {
                        let _ = cq.send(DaemonCompletion::WakeFinished {
                            work,
                            outcome: WakeOutcome::Failed {
                                reason: "no wake executor is configured".to_string(),
                            },
                        });
                    }
                }
            }
            DaemonRequest::RoleSaturated {
                role,
                concurrency,
                waiting,
            } => {
                temper_log::emit::emit_role_saturated(temper_log::emit::RoleSaturated {
                    role: &role,
                    concurrency,
                    waiting: &waiting,
                });
            }
            DaemonRequest::WakeMeasurement(measurement) => {
                emit_wake_measurement(&measurement);
            }
            // Per-job daemon-protocol traces (`engine: assigned`, `engine:
            // result received`, webhook/enqueue book-keeping).
            // These are between-step chatter, NOT the closed §7 info catalog (§5),
            // so they sit at debug; `RUST_LOG=info` shows only the §7 events +
            // startup banner. The §7 events go through `emit_*`, not this sink.
            DaemonRequest::Log(line) => tracing::debug!("{line}"),
            DaemonRequest::WorkstreamActiveReply(reply, active) => {
                reply.send(active);
            }

            #[cfg(test)]
            DaemonRequest::QueuedJobsReply(reply, jobs) => {
                reply.send(jobs);
            }
        }
    }
}
