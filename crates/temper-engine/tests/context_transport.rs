// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use serde_json::json;
use temper_engine_io::http::{HttpCall, build_http_client, http_call};
use temper_forge::{CreateIssue, CreateRepository, Forge, RepositoryPath, UserId};
use temper_forge_memory::{FaultOp, MemoryForge};
use temper_protocol_worker::{
    Artifact, Capability, Capacity, ContextOutcome, FetchContext, ForgeContextErrorCode,
    ForgeContextOperation, ForgeGetItemOperation, ForgeListRelatedOperation, ForgeRelationType,
    JobResult, Poll, Register, ResultStatus, WORKER_AUTHORIZATION_HEADER, WORKER_PROTOCOL_VERSION,
    WorkerAuth, WorkerProtocolMessage,
};
use temper_runner::RepositoryTarget;
use temper_workflow::{RawWorkflowSpec, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let raw: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    raw.validate().expect("workflow validates")
}

fn register(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        worker_pool: Some("builders".to_string()),
        labels: None,
    })
}

fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(1),
    })
}

fn fetch(worker_id: &str, job_id: &str, operation: ForgeContextOperation) -> WorkerProtocolMessage {
    WorkerProtocolMessage::FetchContext(FetchContext::new(worker_id, job_id, operation))
}

fn get_item(repo: &str, number: u64) -> ForgeContextOperation {
    ForgeContextOperation::ForgeGetItem(ForgeGetItemOperation {
        repo: repo.to_string(),
        number,
        artifact_type: None,
        include_comments: false,
    })
}

fn assert_context_error(response: WorkerProtocolMessage, expected: ForgeContextErrorCode) {
    let WorkerProtocolMessage::ContextResponse(response) = response else {
        panic!("expected context response, got {response:?}");
    };
    assert_eq!(response.outcome, ContextOutcome::Error { code: expected });
}

async fn post(
    url: &str,
    message: &WorkerProtocolMessage,
    token: Option<&str>,
) -> temper_engine_io::http::HttpResponseData {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(token) = token {
        headers.push((
            WORKER_AUTHORIZATION_HEADER.to_string(),
            WorkerAuth::bearer(token).authorization_header_value(),
        ));
    }
    http_call(
        &build_http_client(),
        HttpCall {
            method: "POST".to_string(),
            url: url.to_string(),
            headers,
            body: serde_json::to_vec(message).expect("serializes"),
        },
    )
    .await
    .expect("HTTP request succeeds")
}

fn decode(response: temper_engine_io::http::HttpResponseData) -> WorkerProtocolMessage {
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).expect("protocol response")
}

#[test]
fn context_reads_are_assignment_scoped_bounded_and_available_over_both_carriers() {
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repository = forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repository created");
        let issue = forge
            .create_issue(
                &repository.id,
                CreateIssue {
                    title: "context title".to_string(),
                    body: "sensitive body only belongs in the bounded response".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .expect("issue created");
        let catalog = temper_engine::ConfiguredRepositoryCatalog::new(
            [RepositoryTarget::new(
                repository.id.clone(),
                RepositoryPath::new("acme", "service"),
            )],
            "https://forge.invalid",
        )
        .expect("catalog");
        let mut auth = temper_engine::WorkerPoolAuthConfig::new();
        auth.insert_pool("builders", Some(WorkerAuth::bearer("pool-secret")));
        let daemon = temper_engine::Daemon::with_applier_and_worker_pools(
            Arc::new(handle.clone()),
            Arc::new(temper_engine::NoopApplier),
            vec![temper_engine::WorkerPoolPolicy::new(
                "builders",
                vec!["engineer".to_string()],
                vec!["acme/service".to_string()],
                Some(2),
            )],
        )
        .with_worker_pool_auth(auth)
        .with_artifact_context_catalog(catalog)
        .with_forge_context_reader(forge.clone(), Arc::new(workflow()));
        let server =
            temper_engine::serve(&handle, &daemon, "127.0.0.1:0".parse().expect("address"))
                .await
                .expect("server starts");
        let url = format!("http://{}/v1/message", server.local_addr());
        let good_auth = Some(WorkerAuth::bearer("pool-secret"));
        for worker in ["worker-a", "worker-b"] {
            assert!(
                daemon
                    .deliver_protocol_message_with_auth(register(worker), good_auth.clone())
                    .await
                    .expect("register")
                    .is_none()
            );
        }
        daemon
            .enqueue_job(
                "job-1",
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(issue.number.get()),
                    kind: "issue".to_string(),
                },
                json!({}),
            )
            .await;
        let assignment = match daemon
            .deliver_protocol_message_with_auth(poll("worker-a"), good_auth.clone())
            .await
            .expect("poll")
        {
            Some(WorkerProtocolMessage::Assign(assign)) => assign,
            other => panic!("expected assignment, got {other:?}"),
        };
        daemon
            .enqueue_job(
                "job-pending",
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(999),
                    kind: "issue".to_string(),
                },
                json!({}),
            )
            .await;

        forge.fail_next(FaultOp::GetIssueByNumber, "backend secret must not escape");
        assert_context_error(
            decode(
                post(
                    &url,
                    &fetch(
                        "worker-a",
                        "job-1",
                        get_item("acme/service", issue.number.get()),
                    ),
                    Some("wrong-token"),
                )
                .await,
            ),
            ForgeContextErrorCode::NotAuthorized,
        );
        assert_context_error(
            decode(
                post(
                    &url,
                    &fetch(
                        "worker-b",
                        "job-1",
                        get_item("acme/service", issue.number.get()),
                    ),
                    Some("pool-secret"),
                )
                .await,
            ),
            ForgeContextErrorCode::NotAuthorized,
        );
        assert_context_error(
            decode(
                post(
                    &url,
                    &fetch(
                        "worker-a",
                        "job-pending",
                        get_item("acme/service", issue.number.get()),
                    ),
                    Some("pool-secret"),
                )
                .await,
            ),
            ForgeContextErrorCode::NotAuthorized,
        );
        assert_context_error(
            decode(
                post(
                    &url,
                    &fetch(
                        "worker-a",
                        "job-1",
                        get_item("other/repo", issue.number.get()),
                    ),
                    Some("pool-secret"),
                )
                .await,
            ),
            ForgeContextErrorCode::NotAuthorized,
        );
        let excessive = ForgeContextOperation::ForgeListRelated(ForgeListRelatedOperation {
            repo: "acme/service".to_string(),
            number: issue.number.get(),
            artifact_type: None,
            relations: vec![ForgeRelationType::Parent],
            depth: Some(usize::MAX),
            limit: Some(1),
        });
        assert_context_error(
            decode(
                post(
                    &url,
                    &fetch("worker-a", "job-1", excessive),
                    Some("pool-secret"),
                )
                .await,
            ),
            ForgeContextErrorCode::LimitExceeded,
        );
        let malformed = http_call(
            &build_http_client(),
            HttpCall {
                method: "POST".to_string(),
                url: url.clone(),
                headers: vec![
                    ("content-type".to_string(), "application/json".to_string()),
                    (
                        WORKER_AUTHORIZATION_HEADER.to_string(),
                        WorkerAuth::bearer("pool-secret").authorization_header_value(),
                    ),
                ],
                body: serde_json::to_vec(&json!({
                    "type": "fetch-context",
                    "protocol_version": WORKER_PROTOCOL_VERSION,
                    "worker_id": "worker-a",
                    "job_id": "job-1",
                    "operation": {"operation": "forge_update_item", "repo": "acme/service", "number": 1}
                }))
                .expect("serializes"),
            },
        )
        .await
        .expect("malformed request returns response");
        assert_context_error(decode(malformed), ForgeContextErrorCode::InvalidRequest);

        // The armed fault survives every denial above, proving none reached Forge.
        let unavailable = decode(
            post(
                &url,
                &fetch(
                    "worker-a",
                    "job-1",
                    get_item("acme/service", issue.number.get()),
                ),
                Some("pool-secret"),
            )
            .await,
        );
        assert_context_error(unavailable.clone(), ForgeContextErrorCode::ForgeUnavailable);
        assert!(
            !serde_json::to_string(&unavailable)
                .expect("response serializes")
                .contains("backend secret")
        );
        let response = decode(
            post(
                &url,
                &fetch(
                    "worker-a",
                    "job-1",
                    get_item("acme/service", issue.number.get()),
                ),
                Some("pool-secret"),
            )
            .await,
        );
        assert!(
            serde_json::to_vec(&response)
                .expect("response serializes")
                .len()
                <= temper_engine::MAX_FORGE_RESPONSE_BYTES
        );
        assert!(matches!(
            response,
            WorkerProtocolMessage::ContextResponse(temper_protocol_worker::ContextResponse {
                outcome: ContextOutcome::Success { .. },
                ..
            })
        ));

        let in_process = daemon
            .deliver_protocol_message_with_auth(
                fetch(
                    "worker-a",
                    "job-1",
                    get_item("acme/service", issue.number.get()),
                ),
                good_auth,
            )
            .await
            .expect("in-process request")
            .expect("context response");
        assert!(matches!(
            in_process,
            WorkerProtocolMessage::ContextResponse(temper_protocol_worker::ContextResponse {
                outcome: ContextOutcome::Success { .. },
                ..
            })
        ));

        let release = daemon
            .deliver_protocol_message_with_auth(
                WorkerProtocolMessage::Result(JobResult {
                    protocol_version: WORKER_PROTOCOL_VERSION,
                    worker_id: "worker-a".to_string(),
                    job_id: "job-1".to_string(),
                    attempt_id: assignment.attempt_id.clone(),
                    status: ResultStatus::Success,
                    repos: Vec::new(),
                    verdict: None,
                    title: None,
                    body: None,
                    children: Vec::new(),
                    failure: None,
                    summary: None,
                    details: None,
                }),
                Some(WorkerAuth::bearer("pool-secret")),
            )
            .await
            .expect("result accepted")
            .expect("release response");
        assert!(matches!(release, WorkerProtocolMessage::Release(_)));
        forge.fail_next(
            FaultOp::GetIssueByNumber,
            "completed assignment must not touch Forge",
        );
        assert_context_error(
            daemon
                .deliver_protocol_message_with_auth(
                    fetch(
                        "worker-a",
                        "job-1",
                        get_item("acme/service", issue.number.get()),
                    ),
                    Some(WorkerAuth::bearer("pool-secret")),
                )
                .await
                .expect("completed read response")
                .expect("context response"),
            ForgeContextErrorCode::NotAuthorized,
        );
        let error = forge
            .get_issue_by_number(&repository.id, issue.number)
            .await
            .expect_err("denied completed read leaves the armed fault untouched");
        assert!(error.to_string().contains("completed assignment"));
    });
}
