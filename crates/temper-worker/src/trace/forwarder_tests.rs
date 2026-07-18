use std::future::Future;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityAcknowledgement, AgentActivityCapturePolicyV1,
};
use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceRepository, WorkspaceWorkItem,
};
use temper_protocol_worker::{WorkerActivityAcknowledgement, WorkerAuth, WorkerProtocolMessage};

use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::trace::TraceCollector;
use crate::trace::tests::usage_frame;

struct ReplyTransport {
    lose_first_reply: AtomicBool,
    malformed: bool,
    partial: bool,
    sent: Mutex<Vec<WorkerProtocolMessage>>,
}

impl ReplyTransport {
    fn new(lose_first_reply: bool, malformed: bool) -> Self {
        Self {
            lose_first_reply: AtomicBool::new(lose_first_reply),
            malformed,
            partial: false,
            sent: Mutex::new(Vec::new()),
        }
    }

    fn partial() -> Self {
        Self {
            lose_first_reply: AtomicBool::new(false),
            malformed: false,
            partial: true,
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
        let mut highest = if self.partial {
            request.batch.events.first().expect("batch event").seq
        } else {
            request.batch.events.last().expect("batch event").seq
        };
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

async fn wait_until(mut predicate: impl FnMut() -> bool, message: &str) {
    for _ in 0..200 {
        if predicate() {
            return;
        }
        sleep_for(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {message}");
}

#[test]
fn durable_appends_wake_the_idle_forwarder_and_coalesce_by_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().to_path_buf()),
    });
    let transport = Arc::new(ReplyTransport::new(false, false));
    let cancellation = WorkerCancellation::default();

    temper_worker_io::block_on_with(move |_cx, spawner| {
        let collector = collector.clone();
        let transport = Arc::clone(&transport);
        let cancellation = cancellation.clone();
        async move {
            let joined = spawn_activity_forwarder(
                spawner,
                collector.clone(),
                Arc::clone(&transport),
                "worker-notified".to_string(),
                None,
                cancellation.clone(),
            )
            .expect("forwarder enabled");

            wait_until(
                || collector.append_waiter_count() == 1,
                "startup recovery to become idle",
            )
            .await;
            // A successful cycle stays notification-driven rather than
            // dropping back into the former 100 ms spool scan.
            sleep_for(Duration::from_millis(150)).await;
            assert_eq!(collector.append_waiter_count(), 1);

            // These synchronous durable appends happen before the woken
            // forwarder can run again, so the dirty set contains one run
            // at its newest generation and recovery observes every record.
            let run = collector
                .begin_run("job-notified", &context())
                .expect("begin trace")
                .expect("enabled trace");
            let run_id = run.run_id().to_string();
            run.accept_frame(usage_frame(7)).expect("append usage");
            let terminal = run.finish_success(None).expect("finish trace");
            drop(run);

            assert!(
                collector
                    .await_acknowledged(&run_id, terminal, Duration::from_secs(1))
                    .await,
                "notified run should flush without waiting for the recovery backstop"
            );
            let sent_sequences = transport
                .sent
                .lock()
                .expect("sent messages")
                .iter()
                .filter_map(|message| match message {
                    WorkerProtocolMessage::ActivityBatch(request)
                        if request.batch.run_id == run_id =>
                    {
                        Some(
                            request
                                .batch
                                .events
                                .iter()
                                .map(|event| event.seq)
                                .collect::<Vec<_>>(),
                        )
                    }
                    _ => None,
                })
                .flatten()
                .collect::<Vec<_>>();
            assert_eq!(sent_sequences, vec![1, 2, 3]);

            cancellation.cancel();
            assert!(joined.recv().await.is_some());
            assert_eq!(collector.append_waiter_count(), 0);
        }
    });
}

#[test]
fn startup_recovers_pending_spool_and_retries_a_lost_reply() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().to_path_buf()),
    };
    let producer = TraceCollector::new(config.clone());
    let run = producer
        .begin_run("job-restart", &context())
        .expect("begin trace")
        .expect("enabled trace");
    let run_id = run.run_id().to_string();
    let terminal = run.finish_success(None).expect("finish trace");
    drop(run);

    // A distinct collector has no in-memory dirty notification. Only the
    // immediate startup recovery can discover this durable run.
    let restarted = TraceCollector::new(config);
    let transport = Arc::new(ReplyTransport::new(true, false));
    let cancellation = WorkerCancellation::default();
    temper_worker_io::block_on_with(move |_cx, spawner| {
        let restarted = restarted.clone();
        let transport = Arc::clone(&transport);
        let cancellation = cancellation.clone();
        async move {
            let joined = spawn_activity_forwarder(
                spawner,
                restarted.clone(),
                Arc::clone(&transport),
                "worker-restart".to_string(),
                None,
                cancellation.clone(),
            )
            .expect("forwarder enabled");
            assert!(
                restarted
                    .await_acknowledged(&run_id, terminal, Duration::from_secs(2))
                    .await,
                "lost reply should retry before the 30 second recovery backstop"
            );
            assert_eq!(transport.sent.lock().expect("sent messages").len(), 2);
            cancellation.cancel();
            assert!(joined.recv().await.is_some());
        }
    });
}

#[test]
fn corrupt_sibling_does_not_block_a_healthy_run() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().to_path_buf()),
    });
    let corrupt = collector
        .begin_run("job-corrupt", &context())
        .expect("begin corrupt trace")
        .expect("enabled corrupt trace");
    corrupt.finish_success(None).expect("finish corrupt trace");
    let mut records = std::fs::OpenOptions::new()
        .append(true)
        .open(corrupt.spool_dir().join("events.jsonl"))
        .expect("open corrupt trace records");
    records.write_all(b"not-json\n").expect("corrupt trace");
    records.sync_all().expect("sync corruption");
    drop(records);
    drop(corrupt);

    let healthy = collector
        .begin_run("job-healthy", &context())
        .expect("begin healthy trace")
        .expect("enabled healthy trace");
    let healthy_id = healthy.run_id().to_string();
    let terminal = healthy.finish_success(None).expect("finish healthy trace");
    drop(healthy);
    let transport = Arc::new(ReplyTransport::new(false, false));

    temper_worker_io::block_on_with(move |cx, _spawner| async move {
        forward_pending(
            cx,
            &collector,
            Arc::clone(&transport),
            "worker-siblings",
            None,
        )
        .await
        .expect("healthy sibling forwards");
        assert!(
            collector
                .await_acknowledged(&healthy_id, terminal, Duration::from_millis(50))
                .await
        );
        assert_eq!(transport.sent.lock().expect("sent messages").len(), 1);
    });
}

#[test]
fn partial_acknowledgements_continue_at_the_next_indexed_offset() {
    let temp = tempfile::tempdir().expect("tempdir");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1::default(),
        spool_root: Some(temp.path().to_path_buf()),
    });
    let run = collector
        .begin_run("job-partial", &context())
        .expect("begin trace")
        .expect("enabled trace");
    run.accept_frame(usage_frame(8)).expect("append usage");
    let run_id = run.run_id().to_string();
    let terminal = run.finish_success(None).expect("finish trace");
    drop(run);
    let transport = Arc::new(ReplyTransport::partial());

    temper_worker_io::block_on_with(move |cx, _spawner| async move {
        forward_pending(
            cx,
            &collector,
            Arc::clone(&transport),
            "worker-partial",
            None,
        )
        .await
        .expect("partial acknowledgements converge");
        let first_sequences = transport
            .sent
            .lock()
            .expect("sent messages")
            .iter()
            .map(|message| match message {
                WorkerProtocolMessage::ActivityBatch(request) => request.batch.first_seq,
                _ => panic!("forwarder sent non-activity message"),
            })
            .collect::<Vec<_>>();
        assert_eq!(first_sequences, vec![1, 2, 3]);
        assert!(
            collector
                .await_acknowledged(&run_id, terminal, Duration::from_millis(50))
                .await
        );
    });
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
            let unacknowledged = collector.recover().expect("recover");
            assert_eq!(unacknowledged[0].acknowledged_seq, 0);
            let index: serde_json::Value = serde_json::from_slice(
                &std::fs::read(temp.path().join(&run_id).join(".forwarding-index.json"))
                    .expect("forwarding index after lost reply"),
            )
            .expect("parse forwarding index after lost reply");
            assert_eq!(index["highest_contiguous_seq"], 0);

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
            assert!(recovered[0].events.is_empty());
            assert!(temp.path().join(&run_id).join("compacted.json").is_file());
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
