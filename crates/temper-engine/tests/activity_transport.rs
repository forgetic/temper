// SPDX-License-Identifier: MPL-2.0

use temper_engine_io::http::{HttpCall, HttpResponseData, build_http_client, http_call};
use temper_protocol_activity::{
    ACTIVITY_PROTOCOL_VERSION, AgentActivityBatch, AgentActivityCapturePolicyV1,
    AgentActivityEventV1, AgentAssignmentIdentityV1, AgentRunEventV1, AgentScopeKindV1,
    AgentScopeV1, ModelCallFinishedV1, ModelCallStatusV1, ModelFailureCategoryV1, ModelFailureV1,
    REDACTED_MODEL_FAILURE_MESSAGE, RunStartedV1, StopReasonV1, UNKNOWN_MODEL_FAILURE_IDENTITY,
};
use temper_protocol_worker::{
    Capability, Capacity, Register, WORKER_AUTHORIZATION_HEADER, WORKER_PROTOCOL_VERSION,
    WorkerActivityBatch, WorkerAuth, WorkerProtocolMessage,
};

fn register(worker_id: &str, role: &str, repo: &str, pool: Option<&str>) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        worker_pool: pool.map(str::to_string),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    })
}

async fn post_bearer(
    url: &str,
    message: &WorkerProtocolMessage,
    token: Option<&str>,
) -> HttpResponseData {
    let client = build_http_client();
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if let Some(token) = token {
        headers.push((
            WORKER_AUTHORIZATION_HEADER.to_string(),
            WorkerAuth::bearer(token).authorization_header_value(),
        ));
    }
    http_call(
        &client,
        HttpCall {
            method: "POST".to_string(),
            url: url.to_string(),
            headers,
            body: serde_json::to_vec(message).expect("message serializes"),
        },
    )
    .await
    .expect("post message")
}

fn activity_message(worker_id: &str, run_id: &str) -> WorkerProtocolMessage {
    let policy = AgentActivityCapturePolicyV1::default();
    let assignment = AgentAssignmentIdentityV1 {
        trace_context: None,
        job_id: "job-trace-1".to_string(),
        repository: "ai/temper".to_string(),
        artifact_ref: "ai/temper#310".to_string(),
        role: "engineer".to_string(),
        action: "open_pr".to_string(),
        correlation_key: "pr-for-code-310".to_string(),
    };
    let event = AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        seq: 1,
        occurred_at: "2026-07-14T11:09:03.000Z".to_string(),
        elapsed_ms: 0,
        assignment,
        agent_session_id: None,
        scope: AgentScopeV1 {
            id: "main-trace-1".to_string(),
            kind: AgentScopeKindV1::Main,
            parent_id: None,
        },
        turn: None,
        event: AgentActivityEventV1::RunStarted(RunStartedV1 {
            capture: policy.capture,
        }),
    };
    WorkerProtocolMessage::ActivityBatch(WorkerActivityBatch {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        assignment_id: "job-trace-1".to_string(),
        capture_policy: policy,
        batch: AgentActivityBatch {
            version: ACTIVITY_PROTOCOL_VERSION,
            run_id: run_id.to_string(),
            first_seq: 1,
            events: vec![event],
            blobs: Vec::new(),
        },
    })
}

fn unsafe_model_activity_message(worker_id: &str, run_id: &str) -> WorkerProtocolMessage {
    let WorkerProtocolMessage::ActivityBatch(mut request) = activity_message(worker_id, run_id)
    else {
        unreachable!();
    };
    let start = request.batch.events[0].clone();
    request.batch.events.push(AgentRunEventV1 {
        version: ACTIVITY_PROTOCOL_VERSION,
        run_id: run_id.to_string(),
        seq: 2,
        occurred_at: "2026-07-14T11:09:04.000Z".to_string(),
        elapsed_ms: 10,
        assignment: start.assignment,
        agent_session_id: start.agent_session_id,
        scope: start.scope,
        turn: Some(1),
        event: AgentActivityEventV1::ModelCallFinished(ModelCallFinishedV1 {
            call_id: "transport-model-failure-531".to_string(),
            attempt: 1,
            status: ModelCallStatusV1::Succeeded,
            duration_ms: 10,
            time_to_first_token_ms: None,
            stop_reason: Some(StopReasonV1::Error),
            failure: Some(ModelFailureV1 {
                provider: "openai-codex".to_string(),
                model: "gpt-5.6-sol".to_string(),
                category: ModelFailureCategoryV1::Provider,
                retryable: true,
                http_status: Some(502),
                provider_request_id: Some("request ENVIRONMENT-TRANSPORT-SENTINEL-531".to_string()),
                provider_error_code: Some("raw/body/PROVIDER-RESPONSE-TRANSPORT-SENTINEL-531".to_string()),
                message: "Authorization: Bearer CREDENTIAL-TRANSPORT-SENTINEL-531 PROMPT-TRANSPORT-SENTINEL-531 RAW-BODY-TRANSPORT-SENTINEL-531".to_string(),
                detail_redacted: false,
            }),
        }),
    });
    WorkerProtocolMessage::ActivityBatch(request)
}

#[test]
fn activity_transport_sanitizes_model_failures_before_validation_and_dispatch() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let temp = tempfile::tempdir().expect("trace tempdir");
        let policy = AgentActivityCapturePolicyV1::default();
        let journal = temper_engine::AgentTraceJournal::open(temper_engine::TraceJournalConfig {
            root: temp.path().join("journal"),
            policy,
        })
        .expect("open trace journal");
        let daemon = temper_engine::Daemon::new(std::sync::Arc::new(handle.clone()))
            .with_trace_journal(journal.clone());
        daemon
            .deliver_protocol_message(register(
                "model-failure-worker",
                "engineer",
                "ai/temper",
                None,
            ))
            .await
            .expect("trusted worker registers");

        let message = unsafe_model_activity_message(
            "model-failure-worker",
            "run-transport-model-failure-531",
        );
        let WorkerProtocolMessage::ActivityBatch(raw) = &message else {
            unreachable!();
        };
        assert!(raw.batch.validate().is_err());
        assert!(matches!(
            daemon
                .deliver_protocol_message(message)
                .await
                .expect("trusted activity delivery"),
            Some(WorkerProtocolMessage::ActivityAck(_))
        ));

        let events = journal
            .events("run-transport-model-failure-531")
            .expect("read sanitized activity");
        let AgentActivityEventV1::ModelCallFinished(finished) = &events[1].event else {
            panic!("model finish is durable");
        };
        assert_eq!(finished.status, ModelCallStatusV1::Failed);
        let failure = finished.failure.as_ref().expect("explicit diagnostic");
        assert_eq!(failure.category, ModelFailureCategoryV1::RedactedUnknown);
        assert_eq!(failure.provider, UNKNOWN_MODEL_FAILURE_IDENTITY);
        assert_eq!(failure.model, UNKNOWN_MODEL_FAILURE_IDENTITY);
        assert_eq!(failure.message, REDACTED_MODEL_FAILURE_MESSAGE);
        assert_eq!(failure.provider_request_id, None);
        assert_eq!(failure.provider_error_code, None);
        let durable = std::fs::read(
            journal
                .run_directory("run-transport-model-failure-531")
                .join("events.jsonl"),
        )
        .expect("read durable activity bytes");
        let durable = String::from_utf8_lossy(&durable);
        for sentinel in [
            "CREDENTIAL-TRANSPORT-SENTINEL-531",
            "PROMPT-TRANSPORT-SENTINEL-531",
            "RAW-BODY-TRANSPORT-SENTINEL-531",
            "ENVIRONMENT-TRANSPORT-SENTINEL-531",
            "PROVIDER-RESPONSE-TRANSPORT-SENTINEL-531",
        ] {
            assert!(!durable.contains(sentinel), "journal leaked {sentinel}");
        }
    })
}

#[test]
fn http_and_in_process_carriers_share_durable_ack_and_deduplication() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let temp = tempfile::tempdir().expect("trace tempdir");
        let policy = AgentActivityCapturePolicyV1::default();
        let journal = temper_engine::AgentTraceJournal::open(temper_engine::TraceJournalConfig {
            root: temp.path().join("journal"),
            policy: policy.clone(),
        })
        .expect("open trace journal");
        let mut auth = temper_engine::WorkerPoolAuthConfig::new();
        auth.insert_pool("builders", Some(WorkerAuth::bearer("builders-secret")));
        let daemon = temper_engine::Daemon::with_applier_and_worker_pools(
            std::sync::Arc::new(handle.clone()),
            std::sync::Arc::new(temper_engine::NoopApplier),
            vec![temper_engine::WorkerPoolPolicy::new(
                "builders",
                vec!["engineer".to_string()],
                vec!["ai/temper".to_string()],
                Some(1),
            )],
        )
        .with_worker_pool_auth(auth)
        .with_trace_journal(journal.clone());
        let server = temper_engine::serve(
            &handle,
            &daemon,
            "127.0.0.1:0".parse().expect("loopback addr"),
        )
        .await
        .expect("bind trace server");
        let url = format!("http://{}/v1/message", server.local_addr());

        assert_eq!(
            post_bearer(
                &url,
                &register("trace-worker", "engineer", "ai/temper", Some("builders")),
                Some("builders-secret"),
            )
            .await
            .status,
            204
        );
        let message = activity_message("trace-worker", "run-carrier-parity");
        let http = post_bearer(&url, &message, Some("builders-secret")).await;
        assert_eq!(http.status, 200);
        let http_ack: WorkerProtocolMessage =
            serde_json::from_slice(&http.body).expect("HTTP activity acknowledgement");

        // Model a lost reply by retransmitting through the other carrier.
        let in_process_ack = daemon
            .deliver_protocol_message(message.clone())
            .await
            .expect("trusted in-process delivery")
            .expect("activity acknowledgement");
        assert_eq!(in_process_ack, http_ack);
        assert_eq!(
            journal
                .events("run-carrier-parity")
                .expect("read journal")
                .len(),
            1
        );
        assert_eq!(
            post_bearer(&url, &message, Some("wrong-secret"))
                .await
                .status,
            401
        );
    })
}

#[test]
fn authenticated_activity_forwarding_remains_available_after_shutdown_fence() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let temp = tempfile::tempdir().expect("trace tempdir");
        let journal = temper_engine::AgentTraceJournal::open(temper_engine::TraceJournalConfig {
            root: temp.path().join("journal"),
            policy: AgentActivityCapturePolicyV1::default(),
        })
        .expect("open trace journal");
        let daemon = temper_engine::Daemon::new(std::sync::Arc::new(handle.clone()))
            .with_trace_journal(journal.clone());
        daemon
            .deliver_protocol_message(register(
                "shutdown-trace-worker",
                "engineer",
                "ai/temper",
                None,
            ))
            .await
            .expect("trusted worker registers");

        let shutdown = daemon.begin_shutdown().await;
        assert!(shutdown.report().pending_applications.is_empty());
        assert!(matches!(
            daemon
                .deliver_protocol_message(activity_message(
                    "shutdown-trace-worker",
                    "run-forwarded-during-shutdown",
                ))
                .await
                .expect("post-fence activity delivery"),
            Some(WorkerProtocolMessage::ActivityAck(_))
        ));
        assert_eq!(
            journal
                .events("run-forwarded-during-shutdown")
                .expect("durable shutdown activity")
                .len(),
            1
        );
        assert!(shutdown.wait_for_join().await);
    })
}

#[test]
fn distributed_delivery_requires_configured_auth_but_trusted_carrier_does_not() {
    temper_engine_io::block_on_with(|_cx, handle| async move {
        let temp = tempfile::tempdir().expect("trace tempdir");
        let journal = temper_engine::AgentTraceJournal::open(temper_engine::TraceJournalConfig {
            root: temp.path().join("journal"),
            policy: AgentActivityCapturePolicyV1::default(),
        })
        .expect("open trace journal");
        let daemon = temper_engine::Daemon::new(std::sync::Arc::new(handle.clone()))
            .with_trace_journal(journal.clone());
        let server = temper_engine::serve(
            &handle,
            &daemon,
            "127.0.0.1:0".parse().expect("loopback addr"),
        )
        .await
        .expect("bind trace server");
        let url = format!("http://{}/v1/message", server.local_addr());
        assert_eq!(
            post_bearer(
                &url,
                &register("trusted-worker", "engineer", "ai/temper", None),
                None,
            )
            .await
            .status,
            204
        );

        let message = activity_message("trusted-worker", "run-trusted-only");
        assert_eq!(post_bearer(&url, &message, None).await.status, 401);
        assert!(matches!(
            daemon
                .deliver_protocol_message(message)
                .await
                .expect("trusted delivery"),
            Some(WorkerProtocolMessage::ActivityAck(_))
        ));
        assert_eq!(
            journal
                .events("run-trusted-only")
                .expect("read trusted journal")
                .len(),
            1
        );
    })
}
