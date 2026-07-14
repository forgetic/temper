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

use crate::applier::ResultApplier;

use super::WakeScanner;
use super::machine::{DaemonCompletion, DaemonMachine, DaemonRequest};

pub(super) struct DaemonExecutor {
    pub(super) spawner: Arc<dyn Spawner>,
    pub(super) cq: CqSender<DaemonCompletion>,
    pub(super) applier: Arc<dyn ResultApplier>,
    pub(super) scanner_slot: Arc<std::sync::Mutex<Option<Arc<dyn WakeScanner>>>>,
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
            DaemonRequest::RunApply { job, result } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                let job_id = job.job_id.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let outcome = applier.apply(job, result).await;
                    let _ = cq.send(DaemonCompletion::ApplyFinished { job_id, outcome });
                });
            }
            DaemonRequest::RunApplyAndRespond {
                job,
                result,
                responder,
                response,
            } => {
                let applier = Arc::clone(&self.applier);
                let cq = self.cq.clone();
                let job_id = job.job_id.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    let outcome = applier.apply(job, result).await;
                    let _ = cq.send(DaemonCompletion::ApplyFinished { job_id, outcome });
                    responder.respond(response);
                });
            }
            DaemonRequest::RunClaim {
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
                        assign,
                        worker_id,
                        responder,
                        outcome,
                    });
                });
            }
            DaemonRequest::RunClaimRollback { job, context } => {
                let applier = Arc::clone(&self.applier);
                self.spawner.spawn_with_cx(move |_cx| async move {
                    applier.release_claim(job, context).await;
                });
            }
            DaemonRequest::RunHeartbeatsAndRespond {
                assignments,
                responder,
                response,
            } => {
                let applier = Arc::clone(&self.applier);
                self.spawner.spawn_with_cx(move |_cx| async move {
                    for (job, context) in assignments {
                        applier.heartbeat(job, context).await;
                    }
                    responder.respond(response);
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
                request,
                role,
                responder,
            } => {
                let reader = self
                    .context_reader_slot
                    .lock()
                    .expect("context reader slot")
                    .clone();
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
                    responder.respond(super::protocol::protocol_response(Some(
                        WorkerProtocolMessage::ContextResponse(response),
                    )));
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
            DaemonRequest::RunWakeScan { token, hint } => {
                let scanner = self.scanner_slot.lock().expect("scanner slot").clone();
                let cq = self.cq.clone();
                match scanner {
                    Some(scanner) => {
                        self.spawner.spawn_with_cx(move |_cx| async move {
                            scanner.scan(hint).await;
                            let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
                        });
                    }
                    None => {
                        let _ = cq.send(DaemonCompletion::WakeScanFinished { token });
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
