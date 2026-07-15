use std::sync::Arc;
use std::time::Duration;

use skein::cx::Cx;
use temper_protocol_activity::CaptureModeV1;
use temper_protocol_worker::{
    WORKER_PROTOCOL_VERSION, WorkerActivityBatch, WorkerAuth, WorkerProtocolMessage,
};
use temper_worker_io::{OneshotReceiver, Spawner, oneshot, sleep_for};

use crate::config::WorkerAgentTraceConfig;
use crate::transport::Transport;
use crate::worker_shell::WorkerCancellation;

use super::TraceCollector;

pub(crate) const FORWARD_BATCH_EVENT_LIMIT: usize = 50;
pub(crate) const FORWARD_BATCH_ENCODED_BYTE_LIMIT: usize = 64 * 1024;
const SCAN_INTERVAL: Duration = Duration::from_millis(100);
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

pub(crate) fn spawn_activity_forwarder<T, S>(
    spawner: S,
    config: WorkerAgentTraceConfig,
    transport: Arc<T>,
    worker_id: String,
    auth: Option<WorkerAuth>,
    cancellation: WorkerCancellation,
) -> Option<OneshotReceiver<()>>
where
    T: Transport,
    S: Spawner,
{
    if config.policy.capture == CaptureModeV1::Off || config.spool_root.is_none() {
        return None;
    }
    let collector = TraceCollector::new(config);
    let (joined_tx, joined) = oneshot();
    spawner.spawn_task_with_cx(move |cx| async move {
        let mut backoff = INITIAL_RETRY_BACKOFF;
        loop {
            let attempt = cancellation
                .run(forward_pending(
                    cx.clone(),
                    &collector,
                    Arc::clone(&transport),
                    &worker_id,
                    auth.clone(),
                ))
                .await;
            let Some(attempt) = attempt else {
                break;
            };
            let delay = match attempt {
                Ok(()) => {
                    backoff = INITIAL_RETRY_BACKOFF;
                    SCAN_INTERVAL
                }
                Err(error) => {
                    tracing::warn!(
                        target: "temper::worker",
                        service = "worker",
                        event = "agent.activity.forward_failed",
                        worker_id,
                        %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "worker could not forward durable agent activity; product work will continue"
                    );
                    let delay = backoff;
                    backoff = backoff.saturating_mul(2).min(MAX_RETRY_BACKOFF);
                    delay
                }
            };
            if cancellation.run(sleep_for(delay)).await.is_none() {
                break;
            }
        }
        joined_tx.send(());
    });
    Some(joined)
}

/// Scans all restart-readable spools and forwards every pending contiguous
/// batch. This operation is deliberately independent of the worker job/result
/// state machine, so old terminal spools resume after worker restart.
pub(crate) async fn forward_pending<T: Transport>(
    cx: Cx,
    collector: &TraceCollector,
    transport: Arc<T>,
    worker_id: &str,
    auth: Option<WorkerAuth>,
) -> Result<(), String> {
    let runs = collector
        .recover_forwardable()
        .map_err(|error| format!("recover activity spools: {error}"))?;
    for mut run in runs {
        while let Some(batch) =
            run.pending_batch_bounded(FORWARD_BATCH_EVENT_LIMIT, FORWARD_BATCH_ENCODED_BYTE_LIMIT)
        {
            let last_sent = batch
                .events
                .last()
                .map(|event| event.seq)
                .ok_or_else(|| "forwarder built an empty activity batch".to_string())?;
            let message = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
                protocol_version: WORKER_PROTOCOL_VERSION,
                worker_id: worker_id.to_string(),
                // Job IDs are the current durable assignment identity in the
                // worker protocol. Keeping this explicit leaves room for a
                // distinct attempt ID in a future protocol version.
                assignment_id: run.manifest.assignment.job_id.clone(),
                capture_policy: run.manifest.policy.clone(),
                batch,
            });
            let reply = transport
                .send(cx.clone(), message, auth.clone())
                .await?
                .ok_or_else(|| "daemon returned an empty activity acknowledgement".to_string())?;
            let WorkerProtocolMessage::ActivityAck(reply) = reply else {
                return Err("daemon returned a non-activity acknowledgement".to_string());
            };
            if reply.protocol_version != WORKER_PROTOCOL_VERSION || reply.worker_id != worker_id {
                return Err("activity acknowledgement worker identity mismatch".to_string());
            }
            reply
                .acknowledgement
                .validate()
                .map_err(|error| format!("malformed activity acknowledgement: {error}"))?;
            if reply.acknowledgement.run_id != run.manifest.run_id
                || reply.acknowledgement.highest_contiguous_seq <= run.acknowledged_seq
                || reply.acknowledgement.highest_contiguous_seq > last_sent
            {
                return Err("activity acknowledgement cursor is outside the sent batch".to_string());
            }
            collector
                .acknowledge(
                    &run.manifest.run_id,
                    reply.acknowledgement.highest_contiguous_seq,
                )
                .map_err(|error| format!("persist activity acknowledgement: {error}"))?;
            run.acknowledged_seq = reply.acknowledgement.highest_contiguous_seq;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use temper_protocol_activity::{
        ACTIVITY_PROTOCOL_VERSION, AgentActivityAcknowledgement, AgentActivityCapturePolicyV1,
    };
    use temper_protocol_agent::{
        AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
    };
    use temper_protocol_worker::{
        WorkerActivityAcknowledgement, WorkerAuth, WorkerProtocolMessage,
    };

    use super::*;
    use crate::trace::TraceCollector;

    struct ReplyTransport {
        lose_first_reply: AtomicBool,
        malformed: bool,
        sent: Mutex<Vec<WorkerProtocolMessage>>,
    }

    impl ReplyTransport {
        fn new(lose_first_reply: bool, malformed: bool) -> Self {
            Self {
                lose_first_reply: AtomicBool::new(lose_first_reply),
                malformed,
                sent: Mutex::new(Vec::new()),
            }
        }
    }

    impl Transport for ReplyTransport {
        fn send(
            &self,
            _cx: Cx,
            message: WorkerProtocolMessage,
            _auth: Option<WorkerAuth>,
        ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
            let WorkerProtocolMessage::ActivityBatch(request) = &message else {
                panic!("forwarder sent a non-activity message");
            };
            let run_id = request.batch.run_id.clone();
            let mut highest = request.batch.events.last().expect("batch event").seq;
            if self.malformed {
                highest = highest.saturating_add(1);
            }
            let worker_id = request.worker_id.clone();
            self.sent.lock().expect("sent messages").push(message);
            let lost = self.lose_first_reply.swap(false, Ordering::SeqCst);
            async move {
                if lost {
                    return Err("reply lost after durable ingest".to_string());
                }
                Ok(Some(WorkerProtocolMessage::ActivityAck(
                    WorkerActivityAcknowledgement {
                        protocol_version: WORKER_PROTOCOL_VERSION,
                        worker_id,
                        acknowledgement: AgentActivityAcknowledgement {
                            version: ACTIVITY_PROTOCOL_VERSION,
                            run_id,
                            highest_contiguous_seq: highest,
                        },
                    },
                )))
            }
        }
    }

    fn context() -> WorkspaceContext {
        WorkspaceContext {
            trace_context: None,
            repos: vec![WorkspaceRepository {
                id: "forgejo:ai/temper".to_string(),
                owner: "ai".to_string(),
                name: "temper".to_string(),
                default_branch: "main".to_string(),
                dir: "temper".to_string(),
                access: "writable".to_string(),
                base_branch: "main".to_string(),
                branch_hint: Some("agent/run".to_string()),
            }],
            work_item: WorkspaceWorkItem {
                role: "engineer".to_string(),
                queue: "code_ready".to_string(),
                kind: "code".to_string(),
                target: "Issue { number: ItemNumber(310) }".to_string(),
                context: serde_json::json!({
                    "artifact": {"type": "issue", "number": 310}
                })
                .to_string(),
            },
            artifact_context: None,
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-310".to_string(),
            checkout: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Default::default(),
            pull_request_freshness: None,
            agent_session: Some(AgentSessionState::new("session-310")),
        }
    }

    #[test]
    fn lost_reply_retransmits_and_restart_observes_the_durable_cursor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = WorkerAgentTraceConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            spool_root: Some(temp.path().to_path_buf()),
        };
        let collector = TraceCollector::new(config.clone());
        let run = collector
            .begin_run("job-310", &context())
            .expect("begin trace")
            .expect("enabled trace");
        let run_id = run.run_id().to_string();
        run.finish_success(None).expect("finish trace");
        drop(run);
        let transport = Arc::new(ReplyTransport::new(true, false));

        temper_worker_io::block_on_with(move |cx, _handle| {
            let collector = collector.clone();
            let config = config.clone();
            let transport = Arc::clone(&transport);
            async move {
                assert!(
                    forward_pending(
                        cx.clone(),
                        &collector,
                        Arc::clone(&transport),
                        "worker-310",
                        None,
                    )
                    .await
                    .is_err()
                );
                assert_eq!(collector.recover().expect("recover")[0].acknowledged_seq, 0);

                forward_pending(
                    cx.clone(),
                    &collector,
                    Arc::clone(&transport),
                    "worker-310",
                    None,
                )
                .await
                .expect("retry succeeds");
                let recovered = collector.recover().expect("recover acked");
                assert_eq!(recovered[0].manifest.run_id, run_id);
                assert_eq!(recovered[0].acknowledged_seq, 2);
                assert_eq!(transport.sent.lock().expect("sent").len(), 2);

                // A new collector models a worker restart. The old spool is
                // recovered without rerunning an agent and produces no send.
                let restarted = TraceCollector::new(config);
                forward_pending(cx, &restarted, Arc::clone(&transport), "worker-310", None)
                    .await
                    .expect("restart scan");
                assert_eq!(transport.sent.lock().expect("sent").len(), 2);
            }
        });
    }

    #[test]
    fn malformed_acknowledgement_never_advances_the_spool() {
        let temp = tempfile::tempdir().expect("tempdir");
        let collector = TraceCollector::new(WorkerAgentTraceConfig {
            policy: AgentActivityCapturePolicyV1::default(),
            spool_root: Some(temp.path().to_path_buf()),
        });
        let run = collector
            .begin_run("job-310", &context())
            .expect("begin trace")
            .expect("enabled trace");
        run.finish_success(None).expect("finish trace");
        drop(run);
        let transport = Arc::new(ReplyTransport::new(false, true));

        temper_worker_io::block_on_with(move |cx, _handle| async move {
            assert!(
                forward_pending(cx, &collector, transport, "worker-310", None)
                    .await
                    .is_err()
            );
            assert_eq!(collector.recover().expect("recover")[0].acknowledged_seq, 0);
        });
    }
}
