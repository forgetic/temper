// SPDX-License-Identifier: MPL-2.0

//! Adversarial model-failure coverage across the worker and engine trace plane.

use std::sync::Arc;

use secrecy::SecretString;
use skein::cx::Cx;
use temper_engine::{AgentTraceJournal, AuthenticatedWorkerBinding, Daemon, TraceJournalConfig};
use temper_log::activity::{CanonicalActivityProjector, InMemoryActivitySpanExporter};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1, CaptureModeV1,
    FailureCodeV1, ModelCallFinishedV1, ModelCallStatusV1, ModelFailureCategoryV1, ModelFailureV1,
    REDACTED_MODEL_FAILURE_MESSAGE, RunFailedV1, StopReasonV1,
};
use temper_web::trace::{TraceEventPage, TraceRunStatus, TraceRunSummary, board_projection};

use super::full_path_fixture::workspace_context;
use super::full_path_tests::{get, response_json};
use super::*;
use crate::config::WorkerAgentTraceConfig;

const READ_TOKEN: &str = "model-failure-full-path-read-token";
const CREDENTIAL_SENTINEL: &str = "CREDENTIAL-MODEL-DIAGNOSTIC-SENTINEL-531";
const AUTHORIZATION_HEADER_SENTINEL: &str = "AUTHORIZATION-HEADER-MODEL-DIAGNOSTIC-SENTINEL-531";
const PROMPT_SENTINEL: &str = "PROMPT-MODEL-DIAGNOSTIC-SENTINEL-531";
const RAW_BODY_SENTINEL: &str = "RAW-BODY-MODEL-DIAGNOSTIC-SENTINEL-531";
const ENVIRONMENT_SENTINEL: &str = "ENVIRONMENT-MODEL-DIAGNOSTIC-SENTINEL-531";
const PROVIDER_RESPONSE_SENTINEL: &str = "PROVIDER-RESPONSE-MODEL-DIAGNOSTIC-SENTINEL-531";
const SENTINELS: [&str; 6] = [
    CREDENTIAL_SENTINEL,
    AUTHORIZATION_HEADER_SENTINEL,
    PROMPT_SENTINEL,
    RAW_BODY_SENTINEL,
    ENVIRONMENT_SENTINEL,
    PROVIDER_RESPONSE_SENTINEL,
];

#[test]
fn model_failures_are_sanitized_across_every_canonical_surface() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        for capture in [
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ] {
            exercise_sanitized_model_failure(cx.clone(), &handle, capture).await;
        }
    });
}

async fn exercise_sanitized_model_failure(
    cx: Cx,
    handle: &skein::runtime::RuntimeHandle,
    capture: CaptureModeV1,
) {
    let temporary = tempfile::tempdir().expect("model failure trace temporary directory");
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
        .begin_run("job-model-failure-531", &workspace_context())
        .expect("begin model failure trace")
        .expect("capture enabled");
    run.accept_frame(AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-18T11:09:03.000Z".to_string(),
        elapsed_ms: 12,
        scope: AgentScopeV1 {
            id: "untrusted-model-failure-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(2),
        event: AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
            call_id: "model-call-failure-531".to_string(),
            attempt: 3,
            status: ModelCallStatusV1::Failed,
            duration_ms: 1_200,
            time_to_first_token_ms: Some(100),
            stop_reason: Some(StopReasonV1::Error),
            failure: Some(poisoned_failure("child")),
        }),
    })
    .expect("worker sanitizes an untrusted child model failure");
    run.accept_frame(AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-18T11:09:04.000Z".to_string(),
        elapsed_ms: 13,
        scope: AgentScopeV1 {
            id: "untrusted-model-failure-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(2),
        event: AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
            call_id: "legacy-error-stop-531".to_string(),
            attempt: 0,
            status: ModelCallStatusV1::Succeeded,
            duration_ms: 1,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReasonV1::Error),
            failure: None,
        }),
    })
    .expect("worker canonicalizes a newly ingested legacy error stop");
    run.finish_failure(
        FailureCodeV1::Provider,
        temper_protocol_worker::FailureClass::Transient,
    )
    .expect("host terminal remains independent of model detail");
    let run_id = run.run_id().to_string();
    assert_sentinels_absent("worker spool", &directory_bytes(&spool_root));
    drop(run);

    let recovered = collector.recover().expect("recover model failure spool");
    let batch = recovered[0]
        .pending_batch(100)
        .expect("model failure run has a forwarding batch");
    assert_safe_model_failure(&batch.events);
    assert_host_terminal_is_fixed(&batch.events);
    assert_sentinels_absent(
        "forwarded worker batch",
        &serde_json::to_vec(&batch).expect("serialize model failure batch"),
    );

    // Bypass both producer and worker validation. The direct journal API must
    // normalize before validation, digesting, policy stripping, or persistence.
    let mut forged_batch = batch.clone();
    let forged_failure = model_failure_mut(&mut forged_batch.events);
    *forged_failure = poisoned_failure("direct");
    assert!(
        forged_batch.validate().is_err(),
        "unsafe model diagnostics are not canonical protocol data"
    );

    let journal_root = temporary.path().join("journal");
    let journal = AgentTraceJournal::open(TraceJournalConfig {
        root: journal_root.clone(),
        policy: policy.clone(),
    })
    .expect("open model failure journal");
    let assignment = forged_batch.events[0].assignment.clone();
    let binding = AuthenticatedWorkerBinding {
        worker_id: format!("model-failure-worker-{capture:?}"),
        assignment_id: assignment.job_id.clone(),
        assignment,
        agent_session_id: forged_batch.events[0].agent_session_id.clone(),
        capture_policy: policy.clone(),
    };
    journal
        .ingest(&binding, &forged_batch)
        .expect("engine sanitizes a forged direct batch");

    let mut retransmit = forged_batch;
    *model_failure_mut(&mut retransmit.events) = poisoned_failure("retransmit");
    journal
        .ingest(&binding, &retransmit)
        .expect("unsafe detail does not change canonical retransmit identity");
    journal.recover().expect("sanitized journal recovers");
    assert_sentinels_absent("engine journal", &directory_bytes(&journal_root));
    let durable_events = journal.events(&run_id).expect("read durable model failure");
    assert_safe_model_failure(&durable_events);
    assert_host_terminal_is_fixed(&durable_events);

    let daemon = Daemon::new(Arc::new(handle.clone()))
        .with_agent_trace_query(journal, SecretString::from(READ_TOKEN));
    let server = temper_engine::serve(handle, &daemon, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("model failure query server");
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
    assert_eq!(summary.status, TraceRunStatus::Failed);
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
    assert_safe_model_failure(&queried.events);

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
        "web trace projection",
        &serde_json::to_vec(&web).expect("serialize model failure web projection"),
    );
    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&queried.events);
    assert_sentinels_absent(
        "OpenTelemetry projection",
        format!("{:?}", exporter.finished_spans()).as_bytes(),
    );
}

fn poisoned_failure(source: &str) -> ModelFailureV1 {
    ModelFailureV1 {
        provider: "openai-codex".to_string(),
        model: "gpt-5.6-sol".to_string(),
        category: ModelFailureCategoryV1::Provider,
        retryable: true,
        http_status: Some(502),
        provider_request_id: Some(format!("request {source} {ENVIRONMENT_SENTINEL}")),
        provider_error_code: Some(format!("raw/body/{source}/{RAW_BODY_SENTINEL}")),
        message: format!("Authorization: Bearer {source} {}", SENTINELS.join(" ")),
        detail_redacted: false,
    }
}

fn model_failure_mut(events: &mut [AgentRunEventV1]) -> &mut ModelFailureV1 {
    events
        .iter_mut()
        .find_map(|event| match &mut event.event {
            AgentActivityEventV1::ModelCallFinished(finished) => finished.failure.as_mut(),
            _ => None,
        })
        .expect("model failure event")
}

fn assert_safe_model_failure(events: &[AgentRunEventV1]) {
    let finishes = events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ModelCallFinished(finished) => Some(finished),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), 2);
    let finished = finishes[0];
    assert_eq!(finished.status, ModelCallStatusV1::Failed);
    assert_eq!(finished.stop_reason, Some(StopReasonV1::Error));
    let failure = finished.failure.as_ref().expect("explicit safe diagnostic");
    assert_eq!(failure.provider, "openai-codex");
    assert_eq!(failure.model, "gpt-5.6-sol");
    assert_eq!(failure.category, ModelFailureCategoryV1::RedactedUnknown);
    assert!(failure.retryable);
    assert_eq!(failure.http_status, Some(502));
    assert_eq!(failure.provider_request_id, None);
    assert_eq!(failure.provider_error_code, None);
    assert_eq!(failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
    assert!(failure.detail_redacted);

    assert_eq!(finishes[1].status, ModelCallStatusV1::Failed);
    let legacy_failure = finishes[1]
        .failure
        .as_ref()
        .expect("legacy error stop receives explicit detail");
    assert_eq!(
        legacy_failure.category,
        ModelFailureCategoryV1::RedactedUnknown
    );
    assert_eq!(legacy_failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
}

fn assert_host_terminal_is_fixed(events: &[AgentRunEventV1]) {
    let Some(AgentActivityEventV1::RunFailed(RunFailedV1 { failure })) =
        events.last().map(|event| &event.event)
    else {
        panic!("failed trace has a host terminal event");
    };
    assert_eq!(failure.code, FailureCodeV1::Provider);
    assert_eq!(failure.message, "agent run failed with a transient error");
    assert!(failure.retryable);
}

fn assert_sentinels_absent(surface: &str, bytes: &[u8]) {
    let rendered = String::from_utf8_lossy(bytes);
    for sentinel in SENTINELS {
        assert!(
            !rendered.contains(sentinel),
            "{surface} leaked model failure sentinel {sentinel}"
        );
    }
}

fn directory_bytes(root: &std::path::Path) -> Vec<u8> {
    if root.is_file() {
        return std::fs::read(root).expect("read model failure trace file");
    }
    let mut bytes = Vec::new();
    if !root.exists() {
        return bytes;
    }
    let mut entries = std::fs::read_dir(root)
        .expect("read model failure trace directory")
        .map(|entry| entry.expect("model failure trace directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        bytes.extend(directory_bytes(&entry));
    }
    bytes
}
