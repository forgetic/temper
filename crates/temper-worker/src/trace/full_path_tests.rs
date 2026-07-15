// SPDX-License-Identifier: MPL-2.0

//! Hermetic capstone for the canonical activity plane. Unlike component tests,
//! this starts with first-party `AgentEvent`s and observes the resulting run
//! only after socket collection, durable forwarding, engine query, and web/OTel
//! projection.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use secrecy::SecretString;
use skein::cx::Cx;
use temper_engine::{
    AgentTraceJournal, Daemon, NoopApplier, TraceJournalConfig, WorkerPoolAuthConfig,
    WorkerPoolPolicy,
};
use temper_log::activity::{CanonicalActivityProjector, InMemoryActivitySpanExporter};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AgentScopeKindV1, CaptureModeV1,
    FailureCodeV1, RunFailedV1,
};
use temper_protocol_worker::{
    Capability, Capacity, FailureClass, Register, WORKER_PROTOCOL_VERSION, WorkerActivityBatch,
    WorkerAuth, WorkerProtocolMessage,
};
use temper_web::trace::{TraceEventPage, TraceRunStatus, TraceRunSummary, board_projection};
use temper_worker_io::{HttpCall, build_http_client, http_call};

use super::forwarder::forward_pending;
use super::full_path_fixture::{
    expected_main_prompt, full_path_policy, produce_first_party_run, workspace_context,
};
use super::full_path_observation::{
    DISTRIBUTED_BEARER_SENTINEL, Observation, READ_TOKEN, REJECTED_BEARER_SENTINEL,
    assert_complete_post_idle_activity, assert_exact_prompt_snapshots,
    assert_large_main_prompt_snapshot, assert_outside_prompt_sentinels_absent,
    observe_authorized_query,
};
use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::{AgentRunner, HttpTransport, OutOfProcessRunner, Transport};

struct TrustedInProcessTransport {
    daemon: Daemon,
}

impl Transport for TrustedInProcessTransport {
    fn send(
        &self,
        _cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let daemon = self.daemon.clone();
        async move {
            daemon
                .deliver_protocol_message_with_auth(message, auth)
                .await
        }
    }
}

struct LoseFirstActivityReply<T> {
    inner: Arc<T>,
    lose: AtomicBool,
}

impl<T> LoseFirstActivityReply<T> {
    fn new(inner: Arc<T>) -> Self {
        Self {
            inner,
            lose: AtomicBool::new(true),
        }
    }
}

impl<T: Transport> Transport for LoseFirstActivityReply<T> {
    fn send(
        &self,
        cx: Cx,
        message: WorkerProtocolMessage,
        auth: Option<WorkerAuth>,
    ) -> impl Future<Output = Result<Option<WorkerProtocolMessage>, String>> + Send {
        let activity = matches!(message, WorkerProtocolMessage::ActivityBatch(_));
        let inner = Arc::clone(&self.inner);
        let lose = activity && self.lose.swap(false, Ordering::SeqCst);
        async move {
            let reply = inner.send(cx, message, auth).await?;
            if lose {
                Err("injected lost acknowledgement after durable ingestion".to_string())
            } else {
                Ok(reply)
            }
        }
    }
}

#[test]
fn first_party_events_cross_the_complete_standalone_and_distributed_paths() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        let standalone = run_standalone(cx.clone(), &handle).await;
        let distributed = run_distributed(cx, &handle).await;

        assert_eq!(standalone.vocabulary, distributed.vocabulary);
        assert_eq!(standalone.scope_shape, distributed.scope_shape);
        assert_eq!(standalone.span_names, distributed.span_names);
    });
}

const CREDENTIAL_FAILURE_SENTINEL: &str = "CREDENTIAL-FAILURE-SENTINEL-353";
const HEADER_FAILURE_SENTINEL: &str = "HEADER-FAILURE-SENTINEL-353";
const ENVIRONMENT_FAILURE_SENTINEL: &str = "ENVIRONMENT-FAILURE-SENTINEL-353";
const STDERR_FAILURE_SENTINEL: &str = "STDERR-FAILURE-SENTINEL-353";
const FAILURE_SENTINELS: [&str; 4] = [
    CREDENTIAL_FAILURE_SENTINEL,
    HEADER_FAILURE_SENTINEL,
    ENVIRONMENT_FAILURE_SENTINEL,
    STDERR_FAILURE_SENTINEL,
];

#[test]
#[cfg(unix)]
fn raw_child_failures_never_cross_the_canonical_trace_plane() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        for capture in [
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ] {
            exercise_sanitized_child_failure(cx.clone(), &handle, capture).await;
        }
        trace_disabled_child_failure_keeps_diagnostics_outside_activity().await;
    });
}

#[cfg(unix)]
async fn exercise_sanitized_child_failure(
    cx: Cx,
    handle: &skein::runtime::RuntimeHandle,
    capture: CaptureModeV1,
) {
    use std::os::unix::fs::PermissionsExt as _;

    let temporary = tempfile::tempdir().expect("failure trace temporary directory");
    let script = temporary.path().join("sentinel-crash.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{CREDENTIAL_FAILURE_SENTINEL}' '{HEADER_FAILURE_SENTINEL}' '{ENVIRONMENT_FAILURE_SENTINEL}' '{STDERR_FAILURE_SENTINEL}' >&2\nexit 17\n"
        ),
    )
    .expect("write crash fixture");
    let mut permissions = std::fs::metadata(&script)
        .expect("crash fixture metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&script, permissions).expect("make crash fixture executable");

    let policy = AgentActivityCapturePolicyV1 {
        capture,
        capture_thinking: capture == CaptureModeV1::Diagnostic,
        ..Default::default()
    };
    let spool_root = temporary.path().join("spool");
    let collector_config = WorkerAgentTraceConfig {
        policy: policy.clone(),
        spool_root: Some(spool_root.clone()),
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_trace_policy(Some(policy.clone()))
        .with_trace_collector(collector_config.clone());
    let context = workspace_context();
    let error = runner
        .run("job-failure-353", &context, temporary.path())
        .await
        .expect_err("crash fixture remains a job failure");
    assert_eq!(error.class, FailureClass::Transient);
    for sentinel in FAILURE_SENTINELS {
        assert!(
            error.message.contains(sentinel),
            "bounded job diagnostics retain {sentinel}"
        );
    }

    let collector = TraceCollector::new(collector_config);
    let recovered = collector.recover().expect("recover failure spool");
    assert_eq!(recovered.len(), 1);
    assert_sentinels_absent("worker spool", &directory_bytes(&spool_root));
    let batch = recovered[0]
        .pending_batch(100)
        .expect("failed run has a forwarding batch");
    let events = batch.events.clone();
    let terminal = events.last().expect("failed run terminal event");
    let AgentActivityEventV1::RunFailed(RunFailedV1 { failure }) = &terminal.event else {
        panic!("failed child must end with run.failed");
    };
    assert_eq!(failure.code, FailureCodeV1::ChildProcess);
    assert_eq!(failure.message, "agent run failed with a transient error");
    assert!(failure.retryable);
    assert_sentinels_absent(
        "canonical worker batch",
        &serde_json::to_vec(&batch).expect("serialize worker batch"),
    );

    let journal = journal(&temporary.path().join("journal"), &policy);
    let daemon = Daemon::new(Arc::new(handle.clone()))
        .with_trace_journal(journal.clone())
        .with_agent_trace_query(journal.clone(), SecretString::from(READ_TOKEN));
    let server = temper_engine::serve(handle, &daemon, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("failure query server");
    let base_url = format!("http://{}", server.local_addr());
    let carrier = TrustedInProcessTransport { daemon };
    let worker_id = format!("failure-worker-{capture:?}");
    carrier
        .send(cx.clone(), register(&worker_id, None), None)
        .await
        .expect("register failure worker");
    let run_id = batch.run_id.clone();
    let activity = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id,
        assignment_id: events[0].assignment.job_id.clone(),
        capture_policy: policy,
        batch,
    });
    assert!(matches!(
        carrier
            .send(cx.clone(), activity, None)
            .await
            .expect("ingest failed run"),
        Some(WorkerProtocolMessage::ActivityAck(_))
    ));

    let journal_bytes = directory_bytes(temporary.path().join("journal").as_path());
    assert_sentinels_absent("engine journal", &journal_bytes);
    let authorization = format!("Bearer {READ_TOKEN}");
    let summary_response = get(
        &cx,
        &base_url,
        &format!("/v1/agent-runs/{run_id}"),
        Some(&authorization),
    )
    .await;
    assert_sentinels_absent("query summary", &summary_response.body);
    let summary: TraceRunSummary = response_json(summary_response);
    assert_eq!(summary.status, TraceRunStatus::Failed);

    let events_response = get(
        &cx,
        &base_url,
        &format!("/v1/agent-runs/{run_id}/events?after_seq=0&limit=100"),
        Some(&authorization),
    )
    .await;
    assert_sentinels_absent("query events", &events_response.body);
    let queried: TraceEventPage = response_json(events_response);
    assert_eq!(queried.events, events);

    let export = get(
        &cx,
        &base_url,
        &format!("/v1/agent-runs/{run_id}/export"),
        Some(&authorization),
    )
    .await;
    assert_eq!(export.status, 200);
    assert_sentinels_absent("JSONL export", &export.body);

    let web = queried
        .events
        .iter()
        .filter_map(board_projection)
        .collect::<Vec<_>>();
    assert_sentinels_absent(
        "web projection",
        &serde_json::to_vec(&web).expect("serialize web projection"),
    );
    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&queried.events);
    assert_sentinels_absent(
        "OpenTelemetry projection",
        format!("{:?}", exporter.finished_spans()).as_bytes(),
    );
}

#[cfg(unix)]
async fn trace_disabled_child_failure_keeps_diagnostics_outside_activity() {
    let temporary = tempfile::tempdir().expect("trace-off temporary directory");
    let script = temporary.path().join("trace-off-crash.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' '{STDERR_FAILURE_SENTINEL}' >&2\nexit 17\n"),
    )
    .expect("write trace-off fixture");
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&script, permissions).unwrap();
    let spool_root = temporary.path().join("spool-off");
    let policy = AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Off,
        ..Default::default()
    };
    let runner = OutOfProcessRunner::new(vec![script.display().to_string()])
        .with_trace_policy(Some(policy.clone()))
        .with_trace_collector(WorkerAgentTraceConfig {
            policy,
            spool_root: Some(spool_root.clone()),
        });
    let error = runner
        .run("job-trace-off-353", &workspace_context(), temporary.path())
        .await
        .expect_err("trace-off crash remains a job failure");
    assert!(error.message.contains(STDERR_FAILURE_SENTINEL));
    assert!(
        !spool_root.exists(),
        "capture off creates no activity spool"
    );
}

fn assert_sentinels_absent(surface: &str, bytes: &[u8]) {
    let rendered = String::from_utf8_lossy(bytes);
    for sentinel in FAILURE_SENTINELS {
        assert!(
            !rendered.contains(sentinel),
            "{surface} leaked failure sentinel {sentinel}"
        );
    }
}

fn directory_bytes(root: &std::path::Path) -> Vec<u8> {
    if root.is_file() {
        return std::fs::read(root).expect("read trace file");
    }
    let mut bytes = Vec::new();
    if !root.exists() {
        return bytes;
    }
    let mut entries = std::fs::read_dir(root)
        .expect("read trace directory")
        .map(|entry| entry.expect("trace directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        bytes.extend(directory_bytes(&entry));
    }
    bytes
}

async fn run_standalone(cx: Cx, handle: &skein::runtime::RuntimeHandle) -> Observation {
    let temporary = tempfile::tempdir().expect("standalone temporary directory");
    let policy = full_path_policy();
    let journal_root = temporary.path().join("journal");
    let journal = journal(&journal_root, &policy);
    let daemon = Daemon::new(Arc::new(handle.clone()))
        .with_trace_journal(journal.clone())
        .with_agent_trace_query(journal.clone(), SecretString::from(READ_TOKEN));
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("standalone address"),
    )
    .await
    .expect("standalone query server");
    let base_url = format!("http://{}", server.local_addr());
    let carrier = Arc::new(TrustedInProcessTransport { daemon });

    exercise_path(
        cx,
        carrier,
        &base_url,
        &journal,
        &journal_root,
        WorkerAgentTraceConfig {
            policy,
            spool_root: Some(temporary.path().join("spool")),
        },
        "standalone-worker",
        None,
        None,
        None,
    )
    .await
}

async fn run_distributed(cx: Cx, handle: &skein::runtime::RuntimeHandle) -> Observation {
    let temporary = tempfile::tempdir().expect("distributed temporary directory");
    let policy = full_path_policy();
    let journal_root = temporary.path().join("journal");
    let journal = journal(&journal_root, &policy);
    let mut pool_auth = WorkerPoolAuthConfig::new();
    pool_auth.insert_pool(
        "builders",
        Some(WorkerAuth::bearer(DISTRIBUTED_BEARER_SENTINEL)),
    );
    let daemon = Daemon::with_applier_and_worker_pools(
        Arc::new(handle.clone()),
        Arc::new(NoopApplier),
        vec![WorkerPoolPolicy::new(
            "builders",
            vec!["engineer".to_string()],
            vec!["ai/temper".to_string()],
            Some(1),
        )],
    )
    .with_worker_pool_auth(pool_auth)
    .with_trace_journal(journal.clone())
    .with_agent_trace_query(journal.clone(), SecretString::from(READ_TOKEN));
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("distributed address"),
    )
    .await
    .expect("distributed daemon server");
    let base_url = format!("http://{}", server.local_addr());
    let carrier = Arc::new(HttpTransport::new(&base_url));

    exercise_path(
        cx,
        carrier,
        &base_url,
        &journal,
        &journal_root,
        WorkerAgentTraceConfig {
            policy,
            spool_root: Some(temporary.path().join("spool")),
        },
        "distributed-worker",
        Some("builders"),
        Some(WorkerAuth::bearer(DISTRIBUTED_BEARER_SENTINEL)),
        Some(WorkerAuth::bearer(REJECTED_BEARER_SENTINEL)),
    )
    .await
}

fn journal(
    journal_root: &std::path::Path,
    policy: &AgentActivityCapturePolicyV1,
) -> AgentTraceJournal {
    AgentTraceJournal::open(TraceJournalConfig {
        root: journal_root.to_path_buf(),
        policy: policy.clone(),
    })
    .expect("open trace journal")
}

#[allow(clippy::too_many_arguments)]
async fn exercise_path<T: Transport>(
    cx: Cx,
    carrier: Arc<T>,
    base_url: &str,
    journal: &AgentTraceJournal,
    journal_root: &std::path::Path,
    collector_config: WorkerAgentTraceConfig,
    worker_id: &str,
    pool: Option<&str>,
    auth: Option<WorkerAuth>,
    rejected_auth: Option<WorkerAuth>,
) -> Observation {
    carrier
        .send(cx.clone(), register(worker_id, pool), auth.clone())
        .await
        .expect("worker registration");

    let capture_policy = collector_config.policy.clone();
    let spool_root = collector_config
        .spool_root
        .clone()
        .expect("full-path spool root");
    let collector = TraceCollector::new(collector_config.clone());
    let (run_id, generated_batch) = produce_first_party_run(&collector);
    generated_batch
        .validate()
        .expect("generated batch validates");
    let event_count = generated_batch.events.len();
    assert_eq!(event_count, 19);
    assert_complete_post_idle_activity(&generated_batch.events);
    assert!(
        expected_main_prompt()
            .to_canonical_json_bytes()
            .expect("canonical main prompt")
            .len()
            > temper_protocol_activity::MAX_INLINE_CONTENT_BYTES,
        "the capstone must exercise attachment transport rather than inline content"
    );
    assert_exact_prompt_snapshots(&generated_batch.events, &generated_batch.blobs);
    assert_outside_prompt_sentinels_absent(
        "worker batch",
        &serde_json::to_vec(&generated_batch).expect("serialize worker batch"),
    );
    assert_outside_prompt_sentinels_absent("worker spool", &directory_bytes(&spool_root));

    if let Some(rejected_auth) = rejected_auth {
        assert!(
            forward_pending(
                cx.clone(),
                &collector,
                Arc::clone(&carrier),
                worker_id,
                Some(rejected_auth),
            )
            .await
            .is_err(),
            "distributed ingestion rejects the wrong worker credential"
        );
        assert!(
            journal
                .events(&run_id)
                .expect("read rejected run")
                .is_empty()
        );
        assert_eq!(
            collector.recover().expect("recover rejected spool")[0].acknowledged_seq,
            0
        );
    }

    let lossy = Arc::new(LoseFirstActivityReply::new(Arc::clone(&carrier)));
    assert!(
        forward_pending(cx.clone(), &collector, lossy, worker_id, auth.clone(),)
            .await
            .is_err(),
        "a lost acknowledgement is visible only to the trace forwarder"
    );
    let durable_after_lost = journal.events(&run_id).expect("durable lost reply");
    assert!(
        durable_after_lost.len() < event_count,
        "the injected lost reply stops before a later bounded batch"
    );
    assert!(durable_after_lost.iter().any(|event| {
        event.scope.kind == AgentScopeKindV1::Main
            && matches!(event.event, AgentActivityEventV1::PromptPrepared(_))
    }));
    let unacknowledged = collector.recover().expect("unacknowledged spool");
    assert_eq!(unacknowledged[0].acknowledged_seq, 0);
    assert_exact_prompt_snapshots(&unacknowledged[0].events, &unacknowledged[0].blobs);

    // Re-open the durable journal before retransmission to model engine
    // recovery after the acknowledgement was lost. Prompt attachments must be
    // available and byte-exact without consulting the worker spool.
    let recovered_journal = AgentTraceJournal::open(TraceJournalConfig {
        root: journal_root.to_path_buf(),
        policy: capture_policy.clone(),
    })
    .expect("reopen engine journal");
    let recovered_run = recovered_journal
        .run(&run_id)
        .expect("recover engine journal")
        .expect("durable prompt run");
    assert_large_main_prompt_snapshot(&recovered_run.events, &recovered_run.attachments);
    assert_outside_prompt_sentinels_absent("engine journal", &directory_bytes(journal_root));
    drop(recovered_journal);

    // Recreate the collector to model a worker restart. The same producer-made
    // batch is retransmitted; the engine converges by (run_id, seq).
    let restarted = TraceCollector::new(collector_config);
    forward_pending(
        cx.clone(),
        &restarted,
        Arc::clone(&carrier),
        worker_id,
        auth.clone(),
    )
    .await
    .expect("restart retransmit is acknowledged");
    assert_eq!(
        journal.events(&run_id).expect("deduplicated run").len(),
        event_count
    );
    let recovered_complete_journal = AgentTraceJournal::open(TraceJournalConfig {
        root: journal_root.to_path_buf(),
        policy: capture_policy.clone(),
    })
    .expect("reopen complete engine journal");
    let recovered_complete_run = recovered_complete_journal
        .run(&run_id)
        .expect("recover complete engine journal")
        .expect("complete durable activity run");
    assert_complete_post_idle_activity(&recovered_complete_run.events);
    drop(recovered_complete_journal);
    let compacted = restarted.recover().expect("compacted spool");
    assert!(compacted[0].events.is_empty());
    assert_eq!(compacted[0].acknowledged_seq, event_count as u64);

    // A later duplicate delivery through the same carrier remains idempotent.
    let duplicate = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        assignment_id: generated_batch.events[0].assignment.job_id.clone(),
        capture_policy,
        batch: generated_batch,
    });
    assert!(matches!(
        carrier
            .send(cx.clone(), duplicate, auth)
            .await
            .expect("duplicate delivery"),
        Some(WorkerProtocolMessage::ActivityAck(_))
    ));
    assert_eq!(
        journal.events(&run_id).expect("post-duplicate run").len(),
        event_count
    );

    observe_authorized_query(cx, base_url, worker_id, &run_id).await
}

fn register(worker_id: &str, pool: Option<&str>) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: pool.map(str::to_string),
        labels: None,
    })
}

pub(super) async fn get(
    cx: &Cx,
    base_url: &str,
    path: &str,
    authorization: Option<&str>,
) -> temper_worker_io::HttpResponseData {
    let headers = authorization.map_or_else(Vec::new, |value| {
        vec![("Authorization".to_string(), value.to_string())]
    });
    http_call(
        cx,
        &build_http_client(),
        HttpCall {
            method: "GET".to_string(),
            url: format!("{base_url}{path}"),
            headers,
            body: Vec::new(),
        },
    )
    .await
    .expect("query request")
}

pub(super) fn response_json<T: serde::de::DeserializeOwned>(
    response: temper_worker_io::HttpResponseData,
) -> T {
    assert_eq!(response.status, 200, "query response status");
    serde_json::from_slice(&response.body).expect("typed query response")
}
