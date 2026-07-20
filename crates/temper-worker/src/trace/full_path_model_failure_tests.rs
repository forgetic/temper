// SPDX-License-Identifier: MPL-2.0

//! Adversarial model-failure coverage across the worker and engine trace plane.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use secrecy::SecretString;
use skein::cx::Cx;
use temper_engine::{AgentTraceJournal, AuthenticatedWorkerBinding, Daemon, TraceJournalConfig};
use temper_log::WorkItemRef;
use temper_log::activity::{
    ActivitySpanKind, CanonicalActivityProjector, InMemoryActivitySpanExporter,
};
use temper_log::emit::{AgentFinished, AgentTerminalStatus, emit_agent_finished};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityCapturePolicyV1, AgentActivityEventV1,
    AgentActivityFrameV1, AgentRunEventV1, AgentScopeKindV1, AgentScopeV1, AgentTerminalReasonV1,
    CaptureModeV1, FailureCodeV1, ModelCallFinishedV1, ModelCallStartedV1, ModelCallStatusV1,
    ModelFailureCategoryV1, ModelFailureV1, REDACTED_MODEL_FAILURE_MESSAGE, RunFailedV1,
    StopReasonV1, UNKNOWN_MODEL_FAILURE_IDENTITY,
};
use temper_web::trace::{TraceEventPage, TraceRunStatus, TraceRunSummary, board_projection};
use tracing_subscriber::fmt::MakeWriter;

use super::full_path_fixture::workspace_context;
use super::full_path_tests::{get, response_json};
use super::*;
use crate::config::WorkerAgentTraceConfig;

const READ_TOKEN: &str = "model-failure-full-path-read-token";
const SENTINELS: [&str; 5] = [
    "PROMPT-SENTINEL-555",
    "RAW-BODY-SENTINEL-555",
    "ENVIRONMENT-SENTINEL-555",
    "CREDENTIAL-SENTINEL-555",
    "PROVIDER-RESPONSE-SENTINEL-555",
];

#[derive(Clone, Copy)]
enum ForgedField {
    Message,
    ProviderCode,
    RequestId,
    Provider,
    Model,
}

#[derive(Clone, Copy)]
struct ForgeryCase {
    field: ForgedField,
    sentinel: &'static str,
}

const FORGERY_CASES: [ForgeryCase; 5] = [
    ForgeryCase {
        field: ForgedField::Message,
        sentinel: SENTINELS[0],
    },
    ForgeryCase {
        field: ForgedField::ProviderCode,
        sentinel: SENTINELS[1],
    },
    ForgeryCase {
        field: ForgedField::RequestId,
        sentinel: SENTINELS[2],
    },
    ForgeryCase {
        field: ForgedField::Provider,
        sentinel: SENTINELS[3],
    },
    ForgeryCase {
        field: ForgedField::Model,
        sentinel: SENTINELS[4],
    },
];

#[test]
fn valid_looking_forged_model_failures_are_redacted_across_every_surface() {
    temper_engine_io::block_on_with(|cx, handle| async move {
        for capture in [
            CaptureModeV1::Metadata,
            CaptureModeV1::Transcript,
            CaptureModeV1::Diagnostic,
        ] {
            exercise_sanitized_model_failure(cx.clone(), &handle, capture).await;
        }
        forged_model_failures_with_disabled_capture_create_no_spool();
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
        .begin_run("job-model-failure-555", &workspace_context())
        .expect("begin model failure trace")
        .expect("capture enabled");

    for (index, case) in FORGERY_CASES.into_iter().enumerate() {
        let call_id = format!("forged-child-{index}");
        run.accept_frame(model_frame(
            10 + index as u64 * 2,
            AgentActivityEventV1::ModelCallStarted(ModelCallStartedV1 {
                call_id: call_id.clone(),
                provider: "openai-codex".to_string(),
                model: "gpt-5.6-sol".to_string(),
                attempt: 3,
            }),
        ))
        .expect("accept model start");
        run.accept_frame(model_frame(
            11 + index as u64 * 2,
            AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
                call_id,
                attempt: 3,
                status: ModelCallStatusV1::Failed,
                duration_ms: 1_200,
                time_to_first_token_ms: Some(100),
                stop_reason: Some(StopReasonV1::Error),
                failure: Some(forged_failure(case)),
            }),
        ))
        .expect("worker redacts a syntactically valid forged child diagnostic");
    }
    run.accept_frame(model_frame(
        30,
        AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
            call_id: "legacy-error-stop-555".to_string(),
            attempt: 0,
            status: ModelCallStatusV1::Succeeded,
            duration_ms: 1,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReasonV1::Error),
            failure: None,
        }),
    ))
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
    assert_safe_model_failures(&batch.events);
    assert_host_terminal_is_fixed(&batch.events);
    assert_sentinels_absent(
        "forwarded worker batch",
        &serde_json::to_vec(&batch).expect("serialize model failure batch"),
    );

    // Bypass both producer and worker normalization. Every diagnostic is valid
    // protocol data and carries exactly one opaque sentinel in one string field.
    let mut forged_batch = batch.clone();
    replace_model_failures(&mut forged_batch.events, 0);
    forged_batch
        .validate()
        .expect("valid-looking direct diagnostics pass syntax validation");

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
        .expect("engine redacts a forged direct batch");

    // Rotate which valid field contains each sentinel. Successful deduplication
    // proves retransmit digests are computed from the redacted representation.
    let mut retransmit = forged_batch;
    replace_model_failures(&mut retransmit.events, 1);
    retransmit
        .validate()
        .expect("rotated direct diagnostics remain syntactically valid");
    journal
        .ingest(&binding, &retransmit)
        .expect("untrusted text does not change canonical retransmit identity");
    journal.recover().expect("sanitized journal recovers");
    assert_sentinels_absent("engine journal", &directory_bytes(&journal_root));
    let durable_events = journal.events(&run_id).expect("read durable model failure");
    assert_safe_model_failures(&durable_events);
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
    assert_safe_model_failures(&queried.events);

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

    let log_bytes = render_model_failure_logs(&queried.events);
    assert_sentinels_absent("structured and human logs", &log_bytes);
    let rendered_logs = String::from_utf8_lossy(&log_bytes);
    assert!(
        rendered_logs.contains(REDACTED_MODEL_FAILURE_MESSAGE),
        "structured logs retain the explicit redacted diagnostic"
    );
    assert!(
        rendered_logs.contains("model_error | unknown/unknown category=redacted_unknown"),
        "human logs retain safe typed facts"
    );

    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&queried.events);
    let spans = exporter.finished_spans();
    assert_sentinels_absent("OpenTelemetry projection", format!("{spans:?}").as_bytes());
    let model_spans = spans
        .iter()
        .filter(|span| span.start.kind == ActivitySpanKind::ModelCall)
        .collect::<Vec<_>>();
    assert_eq!(model_spans.len(), FORGERY_CASES.len());
    for span in model_spans {
        assert_redacted_failure(
            span.attributes
                .model_failure
                .as_ref()
                .expect("failed model span carries explicit diagnostic"),
            true,
            Some(502),
        );
    }
}

fn model_frame(elapsed_ms: u64, event: AgentActivityEventV1) -> AgentActivityFrameV1 {
    AgentActivityFrameV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        occurred_at: "2026-07-18T11:09:03.000Z".to_string(),
        elapsed_ms,
        scope: AgentScopeV1 {
            id: "untrusted-model-failure-main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: Some(2),
        event,
    }
}

fn forged_failure(case: ForgeryCase) -> ModelFailureV1 {
    let mut failure = ModelFailureV1 {
        provider: "openai-codex".to_string(),
        model: "gpt-5.6-sol".to_string(),
        category: ModelFailureCategoryV1::Provider,
        retryable: true,
        http_status: Some(502),
        provider_request_id: Some("req_safe_555".to_string()),
        provider_error_code: Some("provider_error".to_string()),
        message: "Provider request failed.".to_string(),
        detail_redacted: false,
    };
    match case.field {
        ForgedField::Message => failure.message = case.sentinel.to_string(),
        ForgedField::ProviderCode => {
            failure.provider_error_code = Some(case.sentinel.to_string());
        }
        ForgedField::RequestId => {
            failure.provider_request_id = Some(case.sentinel.to_string());
        }
        ForgedField::Provider => failure.provider = case.sentinel.to_string(),
        ForgedField::Model => failure.model = case.sentinel.to_string(),
    }
    failure
        .validate()
        .expect("standalone sentinel is syntactically valid");
    let encoded = serde_json::to_string(&failure).expect("encode forged diagnostic");
    for sentinel in SENTINELS {
        assert_eq!(
            encoded.contains(sentinel),
            sentinel == case.sentinel,
            "each forged diagnostic contains exactly its isolated sentinel"
        );
    }
    failure
}

fn replace_model_failures(events: &mut [AgentRunEventV1], rotation: usize) {
    let failures = events
        .iter_mut()
        .filter_map(|event| match &mut event.event {
            AgentActivityEventV1::ModelCallFinished(finished) => finished.failure.as_mut(),
            _ => None,
        });
    for (failure, offset) in failures.zip(0..FORGERY_CASES.len()) {
        *failure = forged_failure(FORGERY_CASES[(offset + rotation) % FORGERY_CASES.len()]);
    }
}

fn assert_safe_model_failures(events: &[AgentRunEventV1]) {
    let finishes = events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ModelCallFinished(finished) => Some(finished),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finishes.len(), FORGERY_CASES.len() + 1);
    for finished in &finishes[..FORGERY_CASES.len()] {
        assert_eq!(finished.status, ModelCallStatusV1::Failed);
        assert_eq!(finished.stop_reason, Some(StopReasonV1::Error));
        assert_redacted_failure(
            finished.failure.as_ref().expect("explicit safe diagnostic"),
            true,
            Some(502),
        );
    }

    let legacy = finishes.last().expect("legacy error stop");
    assert_eq!(legacy.status, ModelCallStatusV1::Failed);
    assert_redacted_failure(
        legacy
            .failure
            .as_ref()
            .expect("legacy error stop receives explicit detail"),
        false,
        None,
    );
}

fn assert_redacted_failure(failure: &ModelFailureV1, retryable: bool, status: Option<u16>) {
    assert_eq!(failure.provider, UNKNOWN_MODEL_FAILURE_IDENTITY);
    assert_eq!(failure.model, UNKNOWN_MODEL_FAILURE_IDENTITY);
    assert_eq!(failure.category, ModelFailureCategoryV1::RedactedUnknown);
    assert_eq!(failure.retryable, retryable);
    assert_eq!(failure.http_status, status);
    assert_eq!(failure.provider_request_id, None);
    assert_eq!(failure.provider_error_code, None);
    assert_eq!(failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
    assert!(failure.detail_redacted);
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

fn forged_model_failures_with_disabled_capture_create_no_spool() {
    let temporary = tempfile::tempdir().expect("disabled model failure trace directory");
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
            .begin_run("job-model-failure-off-555", &workspace_context())
            .expect("disabled collector remains non-fatal")
            .is_none()
    );
    assert!(
        !spool_root.exists(),
        "capture off creates no model failure spool"
    );
}

#[derive(Clone, Default)]
struct SharedBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SharedBuffer {
    type Writer = SharedBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn render_model_failure_logs(events: &[AgentRunEventV1]) -> Vec<u8> {
    let failures = events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ModelCallFinished(finished) => finished.failure.as_ref(),
            _ => None,
        })
        .collect::<Vec<_>>();

    let json_buffer = SharedBuffer::default();
    let json_subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(json_buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(json_subscriber, || emit_failure_logs(&failures));

    let human_buffer = SharedBuffer::default();
    let human_subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(human_buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::with_default(human_subscriber, || emit_failure_logs(&failures));

    let mut bytes = json_buffer.0.lock().unwrap().clone();
    bytes.extend(human_buffer.0.lock().unwrap().iter());
    bytes
}

fn emit_failure_logs(failures: &[&ModelFailureV1]) {
    let item = WorkItemRef::issue("ai/temper", 555);
    for failure in failures {
        emit_agent_finished(AgentFinished {
            item: &item,
            role: "engineer",
            kind: "coding",
            status: AgentTerminalStatus::Failed,
            terminal_reason: Some(AgentTerminalReasonV1::ModelError),
            model_failure: Some(failure),
            duration_ms: 1_200,
            summary: "model call failed",
        });
    }
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
