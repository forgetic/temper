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
use temper_log::activity::{
    ActivitySpanKind, CanonicalActivityProjector, InMemoryActivitySpanExporter,
};
use temper_protocol_activity::{
    AgentActivityCapturePolicyV1, AgentActivityEventV1, AgentScopeKindV1, CaptureModeV1,
    FailureCodeV1, RunFailedV1, RunFinishedV1, RunStatusV1,
};
use temper_protocol_worker::{
    Capability, Capacity, FailureClass, Register, WORKER_PROTOCOL_VERSION, WorkerActivityBatch,
    WorkerAuth, WorkerProtocolMessage,
};
use temper_web::trace::{TraceEventPage, TraceRunStatus, TraceRunSummary, board_projection};
use temper_worker_io::{HttpCall, build_http_client, http_call};

use super::forwarder::forward_pending;
use super::full_path_fixture::{
    ARGUMENT_SENTINEL, DELTA_SENTINEL, MESSAGE_SENTINEL, produce_first_party_run, workspace_context,
};
use super::*;
use crate::config::WorkerAgentTraceConfig;
use crate::{AgentRunner, HttpTransport, OutOfProcessRunner, Transport};

const READ_TOKEN: &str = "full-path-read-token";

#[derive(Debug)]
struct Observation {
    vocabulary: Vec<String>,
    scope_shape: Vec<(AgentScopeKindV1, bool)>,
    span_names: Vec<&'static str>,
}

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

    let journal = journal(temporary.path(), &policy);
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
    let policy = AgentActivityCapturePolicyV1::default();
    let journal = journal(temporary.path(), &policy);
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
    let policy = AgentActivityCapturePolicyV1::default();
    let journal = journal(temporary.path(), &policy);
    let mut pool_auth = WorkerPoolAuthConfig::new();
    pool_auth.insert_pool("builders", Some(WorkerAuth::bearer("builder-secret")));
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
        WorkerAgentTraceConfig {
            policy,
            spool_root: Some(temporary.path().join("spool")),
        },
        "distributed-worker",
        Some("builders"),
        Some(WorkerAuth::bearer("builder-secret")),
        Some(WorkerAuth::bearer("wrong-secret")),
    )
    .await
}

fn journal(
    temporary_root: &std::path::Path,
    policy: &AgentActivityCapturePolicyV1,
) -> AgentTraceJournal {
    AgentTraceJournal::open(TraceJournalConfig {
        root: temporary_root.join("journal"),
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

    let collector = TraceCollector::new(collector_config.clone());
    let (run_id, generated_batch) = produce_first_party_run(&collector);
    generated_batch
        .validate()
        .expect("generated batch validates");
    let event_count = generated_batch.events.len();
    assert_eq!(event_count, 13);

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
    assert_eq!(
        journal.events(&run_id).expect("durable lost reply").len(),
        event_count
    );
    assert_eq!(
        collector.recover().expect("unacknowledged spool")[0].acknowledged_seq,
        0
    );

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
    let compacted = restarted.recover().expect("compacted spool");
    assert!(compacted[0].events.is_empty());
    assert_eq!(compacted[0].acknowledged_seq, event_count as u64);

    // A later duplicate delivery through the same carrier remains idempotent.
    let duplicate = WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        assignment_id: generated_batch.events[0].assignment.job_id.clone(),
        capture_policy: AgentActivityCapturePolicyV1::default(),
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

async fn observe_authorized_query(
    cx: Cx,
    base_url: &str,
    worker_id: &str,
    run_id: &str,
) -> Observation {
    let events_path = format!("/v1/agent-runs/{run_id}/events?after_seq=0&limit=100");
    assert_eq!(get(&cx, base_url, &events_path, None).await.status, 401);
    assert_eq!(
        get(&cx, base_url, &events_path, Some("Bearer wrong-read-token"))
            .await
            .status,
        403
    );
    let summary: TraceRunSummary = response_json(
        get(
            &cx,
            base_url,
            &format!("/v1/agent-runs/{run_id}"),
            Some(&format!("Bearer {READ_TOKEN}")),
        )
        .await,
    );
    let page: TraceEventPage = response_json(
        get(
            &cx,
            base_url,
            &events_path,
            Some(&format!("Bearer {READ_TOKEN}")),
        )
        .await,
    );
    assert!(!page.has_more);
    assert_eq!(page.next_after_seq, 13);
    assert_eq!(summary.status, TraceRunStatus::Succeeded);
    assert_eq!(summary.identity.worker_id, worker_id);
    assert_eq!(summary.identity.job_id, "job-full-path-350");
    assert_eq!(summary.identity.repository, "ai/temper");
    assert_eq!(summary.identity.artifact_ref, "ai/temper#350");
    assert_eq!(summary.identity.role, "engineer");
    assert_eq!(summary.identity.action, "open_pr");
    assert_eq!(summary.identity.correlation_key, "pr-for-code-350");
    assert_eq!(
        summary.identity.agent_session_id.as_deref(),
        Some("session-350")
    );
    assert_eq!(summary.capture_mode, CaptureModeV1::Metadata);
    assert_eq!(summary.counts.events, 13);
    assert_eq!(summary.counts.scopes, 2);
    assert_eq!(summary.counts.turns, 1);
    assert_eq!(summary.counts.model_calls, 1);
    assert_eq!(summary.counts.tool_calls, 1);

    let events = page.events;
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=13).collect::<Vec<_>>()
    );
    assert!(events.iter().all(|event| {
        event.run_id == run_id
            && event.assignment.job_id == "job-full-path-350"
            && event.agent_session_id.as_deref() == Some("session-350")
    }));
    assert!(matches!(
        events.last().map(|event| &event.event),
        Some(AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            ..
        }))
    ));

    let tool = events
        .iter()
        .find_map(|event| match &event.event {
            AgentActivityEventV1::ToolStarted(tool) => Some(tool),
            _ => None,
        })
        .expect("queried tool.started boundary");
    assert_eq!(tool.call_id, "tool-call-350");
    assert_eq!(tool.name, "read");
    assert_eq!(tool.arguments, None);
    let canonical_json = serde_json::to_string(&events).expect("canonical JSON");
    for excluded in [ARGUMENT_SENTINEL, MESSAGE_SENTINEL, DELTA_SENTINEL] {
        assert!(!canonical_json.contains(excluded));
    }

    let main_scope = &events[0].scope.id;
    let scope_shape = events
        .iter()
        .filter_map(|event| match event.event {
            AgentActivityEventV1::ScopeStarted(_) => {
                Some((event.scope.kind, event.scope.parent_id.is_some()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scope_shape,
        vec![
            (AgentScopeKindV1::Main, false),
            (AgentScopeKindV1::SubAgent, true)
        ]
    );
    let child_scope = events
        .iter()
        .find(|event| {
            matches!(event.event, AgentActivityEventV1::ScopeStarted(_))
                && event.scope.kind == AgentScopeKindV1::SubAgent
        })
        .expect("child scope");
    assert_ne!(&child_scope.scope.id, main_scope);
    assert_eq!(
        child_scope.scope.parent_id.as_deref(),
        Some(main_scope.as_str())
    );

    let projected = events
        .iter()
        .filter_map(board_projection)
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), 10);
    let web_json = serde_json::to_string(&projected).expect("web projection JSON");
    assert!(web_json.contains("tool"));
    assert!(!web_json.contains(ARGUMENT_SENTINEL));

    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&events);
    projector.project_all(&events);
    let spans = exporter.finished_spans();
    assert_eq!(spans.len(), 6, "replay must not duplicate projected spans");
    let run_span = spans
        .iter()
        .find(|span| span.start.kind == ActivitySpanKind::Run)
        .expect("run span");
    assert_eq!(
        run_span
            .start
            .remote_parent
            .as_ref()
            .map(|context| context.traceparent.as_str()),
        Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
    );
    assert!(!format!("{spans:?}").contains(ARGUMENT_SENTINEL));
    let mut span_names = spans
        .iter()
        .map(|span| span.start.kind.name())
        .collect::<Vec<_>>();
    span_names.sort_unstable();

    Observation {
        vocabulary: events
            .iter()
            .map(|event| event.event.event_type().to_string())
            .collect(),
        scope_shape,
        span_names,
    }
}

async fn get(
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

fn response_json<T: serde::de::DeserializeOwned>(
    response: temper_worker_io::HttpResponseData,
) -> T {
    assert_eq!(response.status, 200, "query response status");
    serde_json::from_slice(&response.body).expect("typed query response")
}
