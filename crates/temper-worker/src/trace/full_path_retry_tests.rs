// SPDX-License-Identifier: MPL-2.0

//! Retry-specific privacy capstone kept separate from the main full-path trace
//! scenarios so the sentinel coverage remains readable and file-size bounded.

use std::sync::Arc;

use secrecy::SecretString;
use skein::cx::Cx;
use temper_engine::{AgentTraceJournal, AuthenticatedWorkerBinding, Daemon, TraceJournalConfig};
use temper_log::activity::{CanonicalActivityProjector, InMemoryActivitySpanExporter};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1, CaptureModeV1,
    FailureCodeV1, FailureInfoV1, MODEL_CALL_RETRY_FAILURE_MESSAGE, ModelCallRetryingV1,
};
use temper_web::trace::{TraceEventPage, TraceRunSummary, board_projection};
use temper_worker_io::{HttpCall, build_http_client, http_call};

use super::full_path_fixture::workspace_context;
use super::*;
use crate::config::WorkerAgentTraceConfig;

const READ_TOKEN: &str = "retry-full-path-read-token";
const CREDENTIAL_RETRY_SENTINEL: &str = "CREDENTIAL-RETRY-SENTINEL-355";
const HEADER_RETRY_SENTINEL: &str = "HEADER-RETRY-SENTINEL-355";
const ENVIRONMENT_RETRY_SENTINEL: &str = "ENVIRONMENT-RETRY-SENTINEL-355";
const PROVIDER_RESPONSE_RETRY_SENTINEL: &str = "PROVIDER-RESPONSE-RETRY-SENTINEL-355";
const RETRY_SENTINELS: [&str; 4] = [
    CREDENTIAL_RETRY_SENTINEL,
    HEADER_RETRY_SENTINEL,
    ENVIRONMENT_RETRY_SENTINEL,
    PROVIDER_RESPONSE_RETRY_SENTINEL,
];

#[test]
fn retry_diagnostics_are_sanitized_across_every_canonical_surface() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        for capture in [
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ] {
            exercise_sanitized_retry(cx.clone(), &handle, capture).await;
        }
        retry_with_disabled_capture_creates_no_activity_spool();
    });
}

async fn exercise_sanitized_retry(
    cx: Cx,
    handle: &skein::runtime::RuntimeHandle,
    capture: CaptureModeV1,
) {
    let temporary = tempfile::tempdir().expect("retry trace temporary directory");
    let policy = AgentActivityCapturePolicyV1 {
        capture,
        capture_thinking: capture == CaptureModeV1::Diagnostic,
        ..Default::default()
    };
    let spool_root = temporary.path().join("spool");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: policy.clone(),
        spool_root: Some(spool_root.clone()),
    });
    let run = collector
        .begin_run("job-retry-355", &workspace_context())
        .expect("begin retry trace")
        .expect("capture enabled");
    let diagnostics = RETRY_SENTINELS.join(" ");
    run.accept_frame(AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
        elapsed_ms: 12,
        scope: AgentScopeV1 {
            id: "untrusted-child-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(2),
        event: AgentActivityEventV1::ModelCallRetrying(ModelCallRetryingV1 {
            call_id: "model-call-retry-355".to_string(),
            next_attempt: 7,
            delay_ms: 1_500,
            disposition: temper_protocol_activity::ModelFailureDispositionV1::Unknown,
            failure: FailureInfoV1 {
                code: FailureCodeV1::Timeout,
                message: diagnostics.clone(),
                retryable: false,
            },
        }),
    })
    .expect("worker sanitizes an untrusted child retry frame");
    run.finish_success(None).expect("finish retry trace");
    let run_id = run.run_id().to_string();
    assert_sentinels_absent("worker spool", &directory_bytes(&spool_root));
    drop(run);

    let recovered = collector.recover().expect("recover retry spool");
    let batch = recovered[0]
        .pending_batch(100)
        .expect("retry run has a forwarding batch");
    assert_retry_is_allowlisted(&batch.events);
    assert_sentinels_absent(
        "canonical worker batch",
        &serde_json::to_vec(&batch).expect("serialize retry batch"),
    );

    // Forge a direct engine batch to prove ingestion independently restores the
    // invariant even when both the producer and worker boundary are bypassed.
    let mut forged_batch = batch;
    let forged_retry = forged_batch
        .events
        .iter_mut()
        .find_map(|event| match &mut event.event {
            AgentActivityEventV1::ModelCallRetrying(retry) => Some(retry),
            _ => None,
        })
        .expect("forged batch retry");
    forged_retry.failure.message = diagnostics;
    assert!(
        forged_batch.validate().is_err(),
        "raw retry diagnostics are not valid canonical protocol data"
    );

    let journal = journal(temporary.path(), &policy);
    let assignment = forged_batch.events[0].assignment.clone();
    let binding = AuthenticatedWorkerBinding {
        worker_id: format!("retry-worker-{capture:?}"),
        assignment_id: assignment.job_id.clone(),
        assignment,
        agent_session_id: forged_batch.events[0].agent_session_id.clone(),
        capture_policy: policy.clone(),
    };
    journal
        .ingest(&binding, &forged_batch)
        .expect("engine sanitizes a forged retry batch");
    let mut retransmit = forged_batch.clone();
    let retry = retransmit
        .events
        .iter_mut()
        .find_map(|event| match &mut event.event {
            AgentActivityEventV1::ModelCallRetrying(retry) => Some(retry),
            _ => None,
        })
        .expect("retransmitted retry");
    retry.failure.message = format!("{PROVIDER_RESPONSE_RETRY_SENTINEL}-RETRANSMIT");
    journal
        .ingest(&binding, &retransmit)
        .expect("raw diagnostics do not affect canonical retransmit identity");
    assert_sentinels_absent(
        "engine journal",
        &directory_bytes(&temporary.path().join("journal")),
    );
    let durable_events = journal.events(&run_id).expect("read durable retry events");
    assert_retry_is_allowlisted(&durable_events);

    let daemon = Daemon::new(Arc::new(handle.clone()))
        .with_agent_trace_query(journal, SecretString::from(READ_TOKEN));
    let server = temper_engine::serve(handle, &daemon, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("retry query server");
    let base_url = format!("http://{}", server.local_addr());
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
    assert_eq!(summary.capture_mode, capture);

    let events_response = get(
        &cx,
        &base_url,
        &format!("/v1/agent-runs/{run_id}/events?after_seq=0&limit=100"),
        Some(&authorization),
    )
    .await;
    assert_sentinels_absent("query events", &events_response.body);
    let queried: TraceEventPage = response_json(events_response);
    assert_retry_is_allowlisted(&queried.events);

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
        &serde_json::to_vec(&web).expect("serialize retry web projection"),
    );
    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&queried.events);
    assert_sentinels_absent(
        "OpenTelemetry projection",
        format!("{:?}", exporter.finished_spans()).as_bytes(),
    );
}

fn retry_with_disabled_capture_creates_no_activity_spool() {
    let temporary = tempfile::tempdir().expect("disabled retry trace temporary directory");
    let spool_root = temporary.path().join("spool");
    let collector = TraceCollector::new(WorkerAgentTraceConfig {
        policy: AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Off,
            ..Default::default()
        },
        spool_root: Some(spool_root.clone()),
    });
    assert!(
        collector
            .begin_run("job-retry-off-355", &workspace_context())
            .expect("disabled collector remains non-fatal")
            .is_none()
    );
    assert!(!spool_root.exists(), "capture off creates no retry spool");
}

fn assert_retry_is_allowlisted(events: &[AgentRunEventV1]) {
    let retry = events
        .iter()
        .find_map(|event| match &event.event {
            AgentActivityEventV1::ModelCallRetrying(retry) => Some(retry),
            _ => None,
        })
        .expect("model retry event");
    assert_eq!(retry.call_id, "model-call-retry-355");
    assert_eq!(retry.next_attempt, 7);
    assert_eq!(retry.delay_ms, 1_500);
    assert_eq!(retry.failure.code, FailureCodeV1::Timeout);
    assert!(!retry.failure.retryable);
    assert_eq!(retry.failure.message, MODEL_CALL_RETRY_FAILURE_MESSAGE);
}

fn journal(
    temporary_root: &std::path::Path,
    policy: &AgentActivityCapturePolicyV1,
) -> AgentTraceJournal {
    AgentTraceJournal::open(TraceJournalConfig {
        root: temporary_root.join("journal"),
        policy: policy.clone(),
    })
    .expect("open retry trace journal")
}

fn assert_sentinels_absent(surface: &str, bytes: &[u8]) {
    let rendered = String::from_utf8_lossy(bytes);
    for sentinel in RETRY_SENTINELS {
        assert!(
            !rendered.contains(sentinel),
            "{surface} leaked retry sentinel {sentinel}"
        );
    }
}

fn directory_bytes(root: &std::path::Path) -> Vec<u8> {
    if root.is_file() {
        return std::fs::read(root).expect("read retry trace file");
    }
    let mut bytes = Vec::new();
    if !root.exists() {
        return bytes;
    }
    let mut entries = std::fs::read_dir(root)
        .expect("read retry trace directory")
        .map(|entry| entry.expect("retry trace directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        bytes.extend(directory_bytes(&entry));
    }
    bytes
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
    .expect("retry query request")
}

fn response_json<T: serde::de::DeserializeOwned>(
    response: temper_worker_io::HttpResponseData,
) -> T {
    assert_eq!(response.status, 200, "retry query response status");
    serde_json::from_slice(&response.body).expect("typed retry query response")
}
