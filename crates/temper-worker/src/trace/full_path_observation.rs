// SPDX-License-Identifier: MPL-2.0

//! Assertions over the authorized query/export and privacy projections used by
//! the standalone/distributed full-path trace capstone.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use skein::cx::Cx;
use temper_log::activity::{
    ActivitySpanKind, CanonicalActivityProjector, InMemoryActivitySpanExporter,
};
use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, AgentScopeKindV1, BlobAttachmentV1, CaptureModeV1,
    CapturedContentV1, PromptSnapshotV1, RunFinishedV1, RunStatusV1, TraceExportRecordV1,
};
use temper_web::trace::{TraceEventPage, TraceRunStatus, TraceRunSummary, board_projection};

use super::full_path_fixture::{
    ARGUMENT_SENTINEL, DELTA_SENTINEL, LARGE_PROMPT_REPETITIONS, MAIN_USER_PREFIX,
    MESSAGE_SENTINEL, PROMPT_TOOL_DESCRIPTION, expected_child_prompt, expected_main_prompt,
};
use super::full_path_tests::{get, response_json};

pub(super) const READ_TOKEN: &str = "full-path-read-token";
pub(super) const DISTRIBUTED_BEARER_SENTINEL: &str = "builder-secret";
pub(super) const REJECTED_BEARER_SENTINEL: &str = "wrong-secret";
const OUTSIDE_PROMPT_SENTINELS: [&str; 3] = [
    READ_TOKEN,
    DISTRIBUTED_BEARER_SENTINEL,
    REJECTED_BEARER_SENTINEL,
];

#[derive(Debug)]
pub(super) struct Observation {
    pub(super) vocabulary: Vec<String>,
    pub(super) scope_shape: Vec<(AgentScopeKindV1, bool)>,
    pub(super) span_names: Vec<&'static str>,
}

/// Proves the worker-assigned sequence stays contiguous across the injected
/// model-call idle gap and that every boundary after it retains its order and
/// scope identity.
pub(super) fn assert_complete_post_idle_activity(events: &[AgentRunEventV1]) {
    assert_eq!(
        events.iter().map(|event| event.seq).collect::<Vec<_>>(),
        (1..=19).collect::<Vec<_>>(),
        "the durable activity stream must have no sequence gap"
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.event.event_type())
            .collect::<Vec<_>>(),
        vec![
            "run.started",
            "scope.started",
            "prompt.prepared",
            "turn.started",
            "model.call.started",
            "model.call.finished",
            "usage",
            "assistant.message",
            "tool.started",
            "tool.finished",
            "scope.started",
            "scope.started",
            "prompt.prepared",
            "prompt.prepared",
            "scope.finished",
            "scope.finished",
            "turn.finished",
            "scope.finished",
            "run.finished",
        ],
        "post-idle activity boundaries must remain complete and ordered"
    );

    let main_scope = &events[1].scope.id;
    assert_eq!(events[1].scope.kind, AgentScopeKindV1::Main);
    assert_eq!(events[4].scope.id, *main_scope);
    let AgentActivityEventV1::ModelCallStarted(model_started) = &events[4].event else {
        unreachable!("event vocabulary fixes model.call.started at sequence 5");
    };
    let AgentActivityEventV1::ModelCallFinished(model_finished) = &events[5].event else {
        unreachable!("event vocabulary fixes model.call.finished at sequence 6");
    };
    assert_eq!(model_started.call_id, "model-call-350");
    assert_eq!(model_finished.call_id, model_started.call_id);
    assert_eq!(events[5].scope.id, *main_scope);
    assert_eq!(events[5].turn, Some(0));

    for (started_index, finished_index) in [(10, 14), (11, 15)] {
        let child = &events[started_index];
        assert_eq!(child.scope.kind, AgentScopeKindV1::SubAgent);
        assert_eq!(child.scope.parent_id.as_deref(), Some(main_scope.as_str()));
        assert_eq!(events[finished_index].scope.id, child.scope.id);
        assert_eq!(
            events[finished_index].scope.kind,
            AgentScopeKindV1::SubAgent
        );
        assert!(matches!(
            events[finished_index].event,
            AgentActivityEventV1::ScopeFinished(_)
        ));
    }

    assert_eq!(events[16].scope.id, *main_scope);
    assert_eq!(events[16].turn, Some(0));
    assert!(matches!(
        events[16].event,
        AgentActivityEventV1::TurnFinished(_)
    ));
    assert_eq!(events[17].scope.id, *main_scope);
    assert!(matches!(
        events[17].event,
        AgentActivityEventV1::ScopeFinished(_)
    ));
    assert_eq!(events[18].scope.id, *main_scope);
    assert!(matches!(
        events[18].event,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            ..
        })
    ));
}

pub(super) async fn observe_authorized_query(
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
    assert_eq!(page.next_after_seq, 19);
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
    assert_eq!(summary.capture_mode, CaptureModeV1::Transcript);
    assert!(!summary.has_truncated_content);
    assert_eq!(summary.counts.events, 19);
    assert_eq!(summary.counts.scopes, 3);
    assert_eq!(summary.counts.turns, 1);
    assert_eq!(summary.counts.model_calls, 1);
    assert_eq!(summary.counts.tool_calls, 1);

    let events = page.events;
    assert_complete_post_idle_activity(&events);
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
    let Some(CapturedContentV1::Inline(arguments)) = &tool.arguments else {
        panic!("transcript mode retains the bounded model-visible argument preview");
    };
    assert_eq!(arguments.text, ARGUMENT_SENTINEL);
    let canonical_json = serde_json::to_string(&events).expect("canonical JSON");
    assert!(canonical_json.contains(ARGUMENT_SENTINEL));
    assert!(canonical_json.contains(MESSAGE_SENTINEL));
    assert!(!canonical_json.contains(DELTA_SENTINEL));
    assert_outside_prompt_sentinels_absent("authorized event query", canonical_json.as_bytes());

    let main_scope = events
        .iter()
        .find(|event| {
            matches!(event.event, AgentActivityEventV1::ScopeStarted(_))
                && event.scope.kind == AgentScopeKindV1::Main
        })
        .expect("main scope")
        .scope
        .id
        .clone();
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
            (AgentScopeKindV1::SubAgent, true),
            (AgentScopeKindV1::SubAgent, true),
        ]
    );
    let child_scopes = events
        .iter()
        .filter(|event| {
            matches!(event.event, AgentActivityEventV1::ScopeStarted(_))
                && event.scope.kind == AgentScopeKindV1::SubAgent
        })
        .collect::<Vec<_>>();
    assert_eq!(child_scopes.len(), 2);
    assert_ne!(child_scopes[0].scope.id, child_scopes[1].scope.id);
    assert!(
        child_scopes
            .iter()
            .all(|event| { event.scope.parent_id.as_deref() == Some(main_scope.as_str()) })
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, AgentActivityEventV1::PromptPrepared(_)))
            .count(),
        3,
        "main, investigate, and delegate each emit exactly one prompt boundary"
    );

    let projected = events
        .iter()
        .filter_map(board_projection)
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), 12);
    let web_json = serde_json::to_string(&projected).expect("web projection JSON");
    assert!(web_json.contains("tool"));
    for prompt_body in [MAIN_USER_PREFIX, PROMPT_TOOL_DESCRIPTION] {
        assert!(
            !web_json.contains(prompt_body),
            "global board projection leaked {prompt_body}"
        );
    }
    assert!(!web_json.contains(ARGUMENT_SENTINEL));
    assert!(!web_json.contains(MESSAGE_SENTINEL));
    assert_outside_prompt_sentinels_absent("global web projection", web_json.as_bytes());

    let exporter = InMemoryActivitySpanExporter::default();
    let mut projector = CanonicalActivityProjector::new(Arc::new(exporter.clone()));
    projector.project_all(&events);
    projector.project_all(&events);
    let spans = exporter.finished_spans();
    assert_eq!(spans.len(), 7, "replay must not duplicate projected spans");
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
    let rendered_spans = format!("{spans:?}");
    for source_body in [
        ARGUMENT_SENTINEL,
        MESSAGE_SENTINEL,
        MAIN_USER_PREFIX,
        PROMPT_TOOL_DESCRIPTION,
    ] {
        assert!(
            !rendered_spans.contains(source_body),
            "OTel projection leaked {source_body}"
        );
    }
    assert_outside_prompt_sentinels_absent("OpenTelemetry projection", rendered_spans.as_bytes());
    let mut span_names = spans
        .iter()
        .map(|span| span.start.kind.name())
        .collect::<Vec<_>>();
    span_names.sort_unstable();

    let export = get(
        &cx,
        base_url,
        &format!("/v1/agent-runs/{run_id}/export"),
        Some(&format!("Bearer {READ_TOKEN}")),
    )
    .await;
    assert_eq!(export.status, 200);
    assert_outside_prompt_sentinels_absent("self-contained JSONL export", &export.body);
    let records = String::from_utf8(export.body)
        .expect("export UTF-8")
        .lines()
        .map(|line| serde_json::from_str::<TraceExportRecordV1>(line).expect("export record"))
        .collect::<Vec<_>>();
    let mut exported_events = Vec::new();
    let mut exported_blobs = Vec::new();
    for record in records {
        match record {
            TraceExportRecordV1::AgentRunEventV1 { version, event } => {
                assert_eq!(version, 1);
                exported_events.push(event);
            }
            TraceExportRecordV1::BlobAttachmentV1 {
                version,
                attachment,
            } => {
                assert_eq!(version, 1);
                exported_blobs.push(attachment);
            }
            TraceExportRecordV1::OperatorTranscriptV1 { .. } => {
                panic!("durable worker export has no operator transcript")
            }
        }
    }
    assert_eq!(exported_events, events);
    assert_complete_post_idle_activity(&exported_events);
    assert_exact_prompt_snapshots(&exported_events, &exported_blobs);

    Observation {
        vocabulary: events
            .iter()
            .map(|event| event.event.event_type().to_string())
            .collect(),
        scope_shape,
        span_names,
    }
}

pub(super) fn assert_large_main_prompt_snapshot(
    events: &[AgentRunEventV1],
    attachments: &[BlobAttachmentV1],
) {
    let event = events
        .iter()
        .find(|event| {
            event.scope.kind == AgentScopeKindV1::Main
                && matches!(event.event, AgentActivityEventV1::PromptPrepared(_))
        })
        .expect("durable main prompt after lost acknowledgement");
    let AgentActivityEventV1::PromptPrepared(prompt) = &event.event else {
        unreachable!();
    };
    let CapturedContentV1::Blob { blob } = prompt.content.as_ref().expect("main prompt content")
    else {
        panic!("main full-path prompt must use blob transport");
    };
    let attachment = attachments
        .iter()
        .find(|attachment| attachment.blob.digest == blob.digest)
        .expect("recovered main prompt attachment");
    let bytes = attachment.decode().expect("decode recovered main prompt");
    assert!(bytes.len() > temper_protocol_activity::MAX_INLINE_CONTENT_BYTES);
    let snapshot: PromptSnapshotV1 =
        serde_json::from_slice(&bytes).expect("decode recovered main prompt snapshot");
    assert_eq!(snapshot, expected_main_prompt());
    assert_eq!(
        snapshot
            .to_canonical_json_bytes()
            .expect("canonical prompt"),
        bytes
    );
}

pub(super) fn assert_exact_prompt_snapshots(
    events: &[AgentRunEventV1],
    attachments: &[BlobAttachmentV1],
) {
    let attachment_bytes = attachments
        .iter()
        .map(|attachment| {
            (
                attachment.blob.digest.as_str(),
                attachment.decode().expect("valid prompt attachment"),
            )
        })
        .collect::<HashMap<_, _>>();
    let scope_labels = events
        .iter()
        .filter_map(|event| match &event.event {
            AgentActivityEventV1::ScopeStarted(started) => Some((
                event.scope.id.as_str(),
                started.display_name.as_deref().unwrap_or(""),
            )),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let main_scope = events
        .iter()
        .find(|event| {
            matches!(event.event, AgentActivityEventV1::ScopeStarted(_))
                && event.scope.kind == AgentScopeKindV1::Main
        })
        .expect("main scope")
        .scope
        .id
        .as_str();
    let mut snapshots = BTreeMap::new();
    for event in events {
        let AgentActivityEventV1::PromptPrepared(prompt) = &event.event else {
            continue;
        };
        assert_eq!(event.turn, Some(0));
        let bytes = match prompt.content.as_ref().expect("captured prompt content") {
            CapturedContentV1::Inline(inline) => inline.text.as_bytes().to_vec(),
            CapturedContentV1::Blob { blob } => attachment_bytes
                .get(blob.digest.as_str())
                .unwrap_or_else(|| panic!("missing exported attachment {}", blob.digest))
                .clone(),
        };
        assert_eq!(prompt.original_snapshot_bytes, bytes.len() as u64);
        assert_eq!(prompt.captured_bytes, bytes.len() as u64);
        let snapshot: PromptSnapshotV1 =
            serde_json::from_slice(&bytes).expect("decode canonical prompt snapshot");
        assert_eq!(
            snapshot
                .to_canonical_json_bytes()
                .expect("canonical snapshot"),
            bytes,
            "prompt bytes must not mutate across a carrier or restart"
        );
        assert!(
            snapshots.insert(event.scope.id.clone(), snapshot).is_none(),
            "one prompt boundary per scope"
        );
    }
    assert_eq!(snapshots.len(), 3);
    assert_eq!(snapshots.get(main_scope), Some(&expected_main_prompt()));
    assert!(
        snapshots[main_scope].initial_user_message.len()
            >= MAIN_USER_PREFIX.len() * LARGE_PROMPT_REPETITIONS
    );
    for (scope_id, snapshot) in &snapshots {
        if scope_id == main_scope {
            continue;
        }
        let label = scope_labels
            .get(scope_id.as_str())
            .copied()
            .expect("child scope label");
        assert!(matches!(label, "investigate" | "delegate"));
        assert_eq!(snapshot, &expected_child_prompt(label));
        let prompt_event = events
            .iter()
            .find(|event| {
                event.scope.id == *scope_id
                    && matches!(event.event, AgentActivityEventV1::PromptPrepared(_))
            })
            .expect("child prompt event");
        assert_eq!(prompt_event.scope.parent_id.as_deref(), Some(main_scope));
    }
}

pub(super) fn assert_outside_prompt_sentinels_absent(surface: &str, bytes: &[u8]) {
    let rendered = String::from_utf8_lossy(bytes);
    for sentinel in OUTSIDE_PROMPT_SENTINELS {
        assert!(
            !rendered.contains(sentinel),
            "{surface} leaked transport sentinel {sentinel}"
        );
    }
}
