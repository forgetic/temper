// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::sync::Arc;

use secrecy::SecretString;
use temper_engine::{
    AgentTraceJournal, AuthenticatedWorkerBinding, Daemon, TraceEventPage, TraceJournalConfig,
    TraceRunPage, TraceRunSummary,
};
use temper_engine_io::http::{HttpCall, HttpResponseData, build_http_client, http_call};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, BlobAttachmentV1, BlobMediaTypeV1, CaptureModeV1, CapturedContentV1,
    DroppedEventKindV1, FailureCodeV1, FailureInfoV1, PromptCaptureDispositionV1, PromptPreparedV1,
    PromptSnapshotV1, RunFailedV1, RunFinishedV1, RunStartedV1, RunStatusV1, StopReasonV1,
    TraceExportRecordV1, TraceGapV1, UsageV1,
};

const TOKEN: &str = "trace-read-super-secret";
const STARTED_AT: &str = "2099-01-01T00:00:00Z";

fn policy() -> AgentActivityCapturePolicyV1 {
    AgentActivityCapturePolicyV1 {
        capture: CaptureModeV1::Transcript,
        ..Default::default()
    }
}

fn binding(
    policy: &AgentActivityCapturePolicyV1,
    run_id: &str,
    role: &str,
    session: Option<&str>,
) -> AuthenticatedWorkerBinding {
    AuthenticatedWorkerBinding {
        worker_id: "worker-a".to_string(),
        assignment_id: format!("assignment-{run_id}"),
        assignment: AgentAssignmentIdentityV1 {
            trace_context: None,
            job_id: format!("job-{run_id}"),
            repository: "ai/temper".to_string(),
            artifact_ref: "ai/temper#311".to_string(),
            role: role.to_string(),
            action: "open_pr".to_string(),
            correlation_key: "pr-for-code-311".to_string(),
        },
        agent_session_id: session.map(str::to_string),
        capture_policy: policy.clone(),
    }
}

fn event(
    run_id: &str,
    binding: &AuthenticatedWorkerBinding,
    seq: u64,
    event: AgentActivityEventV1,
) -> AgentRunEventV1 {
    AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        seq,
        occurred_at: if seq == 1 {
            STARTED_AT.to_string()
        } else {
            format!("2099-01-01T00:00:{:02}Z", seq - 1)
        },
        elapsed_ms: seq.saturating_sub(1) * 100,
        assignment: binding.assignment.clone(),
        agent_session_id: binding.agent_session_id.clone(),
        scope: AgentScopeV1 {
            id: "main".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event,
    }
}

fn start(run_id: &str, binding: &AuthenticatedWorkerBinding) -> AgentRunEventV1 {
    event(
        run_id,
        binding,
        1,
        AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: binding.capture_policy.capture,
        }),
    )
}

fn finish(run_id: &str, binding: &AuthenticatedWorkerBinding, seq: u64) -> AgentRunEventV1 {
    event(
        run_id,
        binding,
        seq,
        AgentActivityEventV1::RunFinished(RunFinishedV1 {
            status: RunStatusV1::Succeeded,
            duration_ms: seq * 100,
            stop_reason: Some(StopReasonV1::EndTurn),
        }),
    )
}

fn blob_prompt(
    run_id: &str,
    binding: &AuthenticatedWorkerBinding,
    seq: u64,
    attachment: &BlobAttachmentV1,
    snapshot: &PromptSnapshotV1,
) -> AgentRunEventV1 {
    let canonical = snapshot.to_canonical_json_bytes().expect("snapshot JSON");
    let tools = snapshot
        .tools_to_canonical_json_bytes()
        .expect("tool manifest JSON");
    let mut event = event(
        run_id,
        binding,
        seq,
        AgentActivityEventV1::PromptPrepared(PromptPreparedV1 {
            system_prompt_present: snapshot.system_prompt.is_some(),
            system_prompt_bytes: snapshot
                .system_prompt
                .as_ref()
                .map_or(0, |prompt| prompt.len() as u64),
            initial_user_message_bytes: snapshot.initial_user_message.len() as u64,
            tool_manifest_bytes: tools.len() as u64,
            tool_count: snapshot.tools.len() as u32,
            original_snapshot_bytes: canonical.len() as u64,
            captured_bytes: canonical.len() as u64,
            disposition: PromptCaptureDispositionV1::Captured,
            content: Some(CapturedContentV1::Blob {
                blob: attachment.blob.clone(),
            }),
        }),
    );
    event.turn = Some(0);
    event
}

fn batch(run_id: &str, events: Vec<AgentRunEventV1>) -> AgentActivityBatch {
    AgentActivityBatch {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        first_seq: events.first().expect("non-empty batch").seq,
        events,
        blobs: Vec::new(),
    }
}

fn seed_journal(root: &std::path::Path) -> (AgentTraceJournal, AuthenticatedWorkerBinding) {
    let policy = policy();
    let journal = AgentTraceJournal::open(TraceJournalConfig {
        root: root.to_path_buf(),
        policy: policy.clone(),
    })
    .expect("journal opens");

    let run_a = binding(&policy, "run-a", "engineer", Some("session-1"));
    journal
        .ingest(
            &run_a,
            &batch(
                "run-a",
                vec![start("run-a", &run_a), finish("run-a", &run_a, 2)],
            ),
        )
        .expect("run-a ingests");

    let run_b = binding(&policy, "run-b", "reviewer", Some("session-2"));
    journal
        .ingest(
            &run_b,
            &batch(
                "run-b",
                vec![
                    start("run-b", &run_b),
                    event(
                        "run-b",
                        &run_b,
                        2,
                        AgentActivityEventV1::RunFailed(RunFailedV1 {
                            failure: FailureInfoV1 {
                                code: FailureCodeV1::Internal,
                                message: "bounded failure".to_string(),
                                retryable: false,
                            },
                        }),
                    ),
                ],
            ),
        )
        .expect("run-b ingests");

    let run_c = binding(&policy, "run-c", "engineer", Some("session-1"));
    journal
        .ingest(
            &run_c,
            &batch(
                "run-c",
                vec![
                    start("run-c", &run_c),
                    event(
                        "run-c",
                        &run_c,
                        2,
                        AgentActivityEventV1::TraceGap(TraceGapV1 {
                            dropped_events: 2,
                            dropped_bytes: 20,
                            kinds: vec![DroppedEventKindV1::TextDelta],
                        }),
                    ),
                ],
            ),
        )
        .expect("partial run-c ingests");

    (journal, run_c)
}

async fn spawn(
    handle: &skein::runtime::RuntimeHandle,
    journal: Option<AgentTraceJournal>,
) -> (Daemon, String) {
    let daemon = Daemon::new(Arc::new(handle.clone()));
    let daemon = journal.map_or(daemon.clone(), |journal| {
        daemon.with_agent_trace_query(journal, SecretString::from(TOKEN))
    });
    let server = temper_engine::serve(
        handle,
        &daemon,
        "127.0.0.1:0".parse().expect("loopback address"),
    )
    .await
    .expect("server binds");
    (daemon, format!("http://{}", server.local_addr()))
}

async fn get(base: &str, path: &str, authorization: Option<&str>) -> HttpResponseData {
    let mut headers = Vec::new();
    if let Some(value) = authorization {
        headers.push(("Authorization".to_string(), value.to_string()));
    }
    http_call(
        &build_http_client(),
        HttpCall {
            method: "GET".to_string(),
            url: format!("{base}{path}"),
            headers,
            body: Vec::new(),
        },
    )
    .await
    .expect("GET succeeds")
}

fn bearer() -> String {
    format!("Bearer {TOKEN}")
}

fn json<T: serde::de::DeserializeOwned>(response: &HttpResponseData) -> T {
    serde_json::from_slice(&response.body).expect("response is typed JSON")
}

#[test]
fn trace_export_protocol_type_retains_engine_compatibility_reexport() {
    let policy = policy();
    let binding = binding(&policy, "compatibility", "engineer", None);
    let protocol_record = TraceExportRecordV1::event(start("compatibility", &binding));
    let engine_record: temper_engine::TraceExportRecordV1 = protocol_record.clone();

    assert_eq!(engine_record, protocol_record);
}

#[test]
fn trace_routes_are_disabled_without_a_token_and_state_stays_public() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_daemon, base) = spawn(&handle, None).await;
        let unavailable = get(&base, "/v1/agent-runs/run-a/events", Some("Bearer any")).await;
        assert_eq!(unavailable.status, 404);
        assert!(!String::from_utf8_lossy(&unavailable.body).contains("run-a"));

        let state = get(&base, "/v1/state", None).await;
        assert_eq!(state.status, 200);
        assert!(!String::from_utf8_lossy(&state.body).contains("agent-runs"));
    });
}

#[test]
fn trace_authorization_distinguishes_missing_and_wrong_without_leaking_secrets() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (journal, _) = seed_journal(&temporary.path().join("journal"));
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_daemon, base) = spawn(&handle, Some(journal)).await;
        let missing = get(&base, "/v1/agent-runs/run-does-not-exist", None).await;
        assert_eq!(missing.status, 401);
        assert!(
            missing
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("www-authenticate"))
        );

        for credential in ["Bearer wrong-secret", "token trace-read-super-secret"] {
            let wrong = get(&base, "/v1/agent-runs/run-does-not-exist", Some(credential)).await;
            assert_eq!(wrong.status, 403);
            let body = String::from_utf8_lossy(&wrong.body);
            assert!(!body.contains(TOKEN));
            assert!(!body.contains("wrong-secret"));
            assert!(!body.contains("run-does-not-exist"));
        }

        let missing_run = get(&base, "/v1/agent-runs/run-does-not-exist", Some(&bearer())).await;
        assert_eq!(missing_run.status, 404);
        assert!(!String::from_utf8_lossy(&missing_run.body).contains("run-does-not-exist"));
    });
}

#[test]
fn run_pages_use_stable_equal_timestamp_order_and_composable_filters_after_restart() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let root = temporary.path().join("journal");
    let (journal, _) = seed_journal(&root);
    drop(journal);
    let reopened = AgentTraceJournal::open(TraceJournalConfig {
        root,
        policy: policy(),
    })
    .expect("journal restarts");

    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_daemon, base) = spawn(&handle, Some(reopened)).await;
        let first = get(&base, "/v1/agent-runs?limit=1", Some(&bearer())).await;
        assert_eq!(first.status, 200);
        let first: TraceRunPage = json(&first);
        assert_eq!(first.runs.len(), 1);
        assert_eq!(first.runs[0].run_id, "run-a");
        let cursor = first.next_cursor.expect("another equal-time run remains");

        let second = get(
            &base,
            &format!("/v1/agent-runs?limit=1&cursor={cursor}"),
            Some(&bearer()),
        )
        .await;
        let second: TraceRunPage = json(&second);
        assert_eq!(second.runs[0].run_id, "run-b");

        let filtered = get(
            &base,
            "/v1/agent-runs?artifact_ref=ai%2Ftemper%23311&role=engineer&correlation_key=pr-for-code-311&agent_session_id=session-1&status=succeeded&run_id=run-a",
            Some(&bearer()),
        )
        .await;
        assert_eq!(filtered.status, 200);
        let filtered: TraceRunPage = json(&filtered);
        assert_eq!(filtered.runs.len(), 1);
        let summary = &filtered.runs[0];
        assert_eq!(summary.run_id, "run-a");
        assert_eq!(summary.identity.role, "engineer");
        assert_eq!(summary.first_seq, Some(1));
        assert_eq!(summary.last_seq, 2);
        assert_eq!(summary.capture_mode, CaptureModeV1::Transcript);

        for path in [
            "/v1/agent-runs?limit=0",
            "/v1/agent-runs?limit=201",
            "/v1/agent-runs?cursor=not-base64",
            "/v1/agent-runs?role=engineer&role=reviewer",
            "/v1/agent-runs?unknown=value",
            &format!("/v1/agent-runs?role=reviewer&cursor={cursor}"),
        ] {
            assert_eq!(
                get(&base, path, Some(&bearer())).await.status,
                400,
                "{path}"
            );
        }
    });
}

#[test]
fn event_pages_follow_appends_and_jsonl_export_preserves_canonical_order() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let (journal, run_c) = seed_journal(&temporary.path().join("journal"));
    let query_journal = journal.clone();

    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_daemon, base) = spawn(&handle, Some(query_journal)).await;
        let first = get(
            &base,
            "/v1/agent-runs/run-c/events?after_seq=0&limit=1",
            Some(&bearer()),
        )
        .await;
        let first: TraceEventPage = json(&first);
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(first.next_after_seq, 1);
        assert!(first.has_more);

        journal
            .ingest(
                &run_c,
                &batch(
                    "run-c",
                    vec![
                        event(
                            "run-c",
                            &run_c,
                            3,
                            AgentActivityEventV1::Usage(UsageV1 {
                                input_tokens: 10,
                                output_tokens: 5,
                                cache_read_tokens: 3,
                                cache_write_tokens: 1,
                            }),
                        ),
                        finish("run-c", &run_c, 4),
                    ],
                ),
            )
            .expect("new events append");

        let rest = get(
            &base,
            "/v1/agent-runs/run-c/events?after_seq=1&limit=3",
            Some(&bearer()),
        )
        .await;
        assert_eq!(rest.status, 200);
        let rest: TraceEventPage = json(&rest);
        assert_eq!(
            rest.events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(rest.next_after_seq, 4);
        assert!(!rest.has_more);

        let summary = get(&base, "/v1/agent-runs/run-c", Some(&bearer())).await;
        let summary: TraceRunSummary = json(&summary);
        assert!(summary.has_trace_gaps);
        assert_eq!(summary.dropped_events, 2);
        assert_eq!(summary.usage.input_tokens, 10);
        assert_eq!(summary.last_seq, 4);

        let export = get(&base, "/v1/agent-runs/run-c/export", Some(&bearer())).await;
        assert_eq!(export.status, 200);
        assert!(export.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-type") && value == "application/x-ndjson"
        }));
        assert!(export.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("cache-control") && value == "no-store"
        }));
        assert!(export.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("x-content-type-options") && value == "nosniff"
        }));
        let exported = String::from_utf8(export.body).expect("JSONL is UTF-8");
        let sequences = exported
            .lines()
            .map(|line| {
                match serde_json::from_str::<TraceExportRecordV1>(line)
                    .expect("versioned export record")
                {
                    TraceExportRecordV1::AgentRunEventV1 { version, event } => {
                        assert_eq!(version, 1);
                        event.seq
                    }
                    TraceExportRecordV1::BlobAttachmentV1 { .. } => {
                        panic!("run-c has no blob attachments")
                    }
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(sequences, [1, 2, 3, 4]);

        for path in [
            "/v1/agent-runs/run-c/events?limit=0",
            "/v1/agent-runs/run-c/events?limit=1001",
            "/v1/agent-runs/run-c/events?after_seq=invalid",
            "/v1/agent-runs/run-c/export?after_seq=1",
        ] {
            assert_eq!(
                get(&base, path, Some(&bearer())).await.status,
                400,
                "{path}"
            );
        }
    });
}

#[test]
fn prompt_blob_export_is_self_contained_deterministic_and_revalidated() {
    let temporary = tempfile::tempdir().expect("tempdir");
    let root = temporary.path().join("journal");
    let policy = policy();
    let journal = AgentTraceJournal::open(TraceJournalConfig {
        root,
        policy: policy.clone(),
    })
    .expect("journal opens");
    let run_id = "run-prompt-export";
    let binding = binding(&policy, run_id, "engineer", Some("session-prompt"));
    let snapshot = PromptSnapshotV1 {
        system_prompt: Some("exact exported system prompt".to_string()),
        initial_user_message: "large user context ".repeat(2_000),
        tools: Vec::new(),
    };
    let canonical = snapshot.to_canonical_json_bytes().expect("snapshot JSON");
    let attachment = BlobAttachmentV1::from_bytes(BlobMediaTypeV1::ApplicationJson, &canonical);
    let first_prompt = blob_prompt(run_id, &binding, 2, &attachment, &snapshot);
    let mut second_prompt = blob_prompt(run_id, &binding, 3, &attachment, &snapshot);
    second_prompt.scope = AgentScopeV1 {
        id: "sub-agent".to_string(),
        kind: AgentScopeKindV1::SubAgent,
        parent_id: Some("main".to_string()),
    };
    journal
        .ingest(
            &binding,
            &AgentActivityBatch {
                version: ACTIVITY_PROTOCOL_VERSION,
                run_id: run_id.to_string(),
                first_seq: 1,
                events: vec![
                    start(run_id, &binding),
                    first_prompt,
                    second_prompt,
                    finish(run_id, &binding, 4),
                ],
                blobs: vec![attachment.clone()],
            },
        )
        .expect("prompt run ingests");
    let digest = attachment.blob.digest.strip_prefix("sha256:").unwrap();
    let blob_path = journal.run_directory(run_id).join("blobs").join(digest);

    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let (_daemon, base) = spawn(&handle, Some(journal)).await;
        assert_eq!(
            get(&base, "/v1/agent-runs/run-prompt-export/export", None,)
                .await
                .status,
            401
        );

        let events = get(
            &base,
            "/v1/agent-runs/run-prompt-export/events",
            Some(&bearer()),
        )
        .await;
        assert_eq!(events.status, 200);
        let events: TraceEventPage = json(&events);
        assert_eq!(
            events
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );

        let export = get(
            &base,
            "/v1/agent-runs/run-prompt-export/export",
            Some(&bearer()),
        )
        .await;
        assert_eq!(export.status, 200);
        assert!(export.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("cache-control") && value == "no-store"
        }));
        let records = String::from_utf8(export.body)
            .expect("export UTF-8")
            .lines()
            .map(|line| serde_json::from_str::<TraceExportRecordV1>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 5, "one shared attachment is emitted once");
        let event_sequences = records
            .iter()
            .filter_map(|record| match record {
                TraceExportRecordV1::AgentRunEventV1 { version, event } => {
                    assert_eq!(*version, 1);
                    Some(event.seq)
                }
                TraceExportRecordV1::BlobAttachmentV1 { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(event_sequences, [1, 2, 3, 4]);
        assert!(matches!(
            records[1],
            TraceExportRecordV1::AgentRunEventV1 { ref event, .. } if event.seq == 2
        ));
        let TraceExportRecordV1::BlobAttachmentV1 {
            version,
            attachment: exported_attachment,
        } = &records[2]
        else {
            panic!("attachment must immediately follow its first referencing event");
        };
        assert_eq!(*version, 1);
        assert_eq!(exported_attachment, &attachment);
        assert_eq!(exported_attachment.decode().unwrap(), canonical);

        fs::write(&blob_path, b"corrupt after first export").expect("corrupt blob fixture");
        let corrupted = get(
            &base,
            "/v1/agent-runs/run-prompt-export/export",
            Some(&bearer()),
        )
        .await;
        assert_eq!(corrupted.status, 500);
        assert!(corrupted.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("cache-control") && value == "no-store"
        }));
    });
}
