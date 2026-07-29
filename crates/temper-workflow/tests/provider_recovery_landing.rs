// SPDX-License-Identifier: MPL-2.0

mod support;

use support::{TestRoot, block_on, create_pr, new_repo};
use temper_forge::{Forge, PullRequestState, UpdatePullRequest};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionError, ProviderRecovery, ProviderRecoveryDisposition,
    ProviderRecoveryFacts, RawWorkflowSpec, RoleId, TransitionId, WorkflowMetadata,
    render_metadata_block,
};

fn workflow() -> temper_workflow::ValidatedWorkflow {
    let raw: RawWorkflowSpec = serde_json::from_str(
        r#"{
          "name":"provider-recovery-landing",
          "roles":[{"id":"owner"}],
          "labels":[{"id":"implementation"}],
          "artifact_kinds":[{
            "id":"implementation_pr",
            "target":"pull_request",
            "identifying_labels":["implementation"]
          }],
          "transitions":[{
            "id":"merge_pr",
            "artifact":"implementation_pr",
            "roles":["owner"],
            "effects":[
              {"kind":"create_comment","body":"must not publish before a deferred landing"},
              {"kind":"merge_pull_request"}
            ]
          }]
        }"#,
    )
    .unwrap();
    raw.validate().unwrap()
}

#[test]
fn provider_deferred_pull_request_cannot_land_mechanically() {
    let root = TestRoot::new();
    let forge = root.forge();
    let workflow = workflow();
    let repo = new_repo(&forge);
    let executor = workflow.executor(&forge);
    let number = create_pr(&forge, &repo, &["implementation"], "implementation");
    let pull = block_on(forge.get_pull_request_by_number(&repo, number))
        .unwrap()
        .unwrap();
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        provider_recovery: Some(Box::new(ProviderRecovery {
            workstream_id: "repair-pr-1".to_string(),
            failure_epoch: 1,
            disposition: ProviderRecoveryDisposition::Unknown,
            facts: ProviderRecoveryFacts {
                provider: "fixture".to_string(),
                model: "fixture".to_string(),
                category: "redacted_unknown".to_string(),
                boundary: "sse".to_string(),
                event_kind: "stream_error".to_string(),
                status_present: false,
                code_present: false,
                http_status: None,
                provider_request_id: None,
                provider_error_code: None,
            },
            cumulative_failure_count: 2,
            deferral_count: 1,
            deferral_limit: 3,
            generation: 1,
            not_before: "2026-05-29T00:05:00Z".parse().unwrap(),
            epoch_started_at: "2026-05-29T00:00:00Z".parse().unwrap(),
            elapsed_ms: 60_000,
            slo_deadline: "2026-05-29T02:00:00Z".parse().unwrap(),
            idempotency_key: "b".repeat(64),
            source_attempt_id: "attempt-repair".to_string(),
            due_assignment_attempt_id: None,
            health_event_id: None,
        })),
        ..WorkflowMetadata::default()
    };
    block_on(forge.update_pull_request(
        &pull.id,
        UpdatePullRequest {
            body: Some(render_metadata_block(&metadata)),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();

    let error = block_on(executor.execute(
        &repo,
        ArtifactSource::PullRequest { number },
        &TransitionId::new("merge_pr"),
        &RoleId::new("owner"),
    ))
    .expect_err("provider recovery marker fences merge publication");
    assert!(matches!(error, ExecutionError::TargetStale { .. }));
    assert_eq!(
        block_on(forge.list_pull_request_comments(&pull.id))
            .unwrap()
            .len(),
        0,
        "the landing fence runs before any transition pre-effect"
    );
    assert_eq!(
        block_on(forge.get_pull_request_by_number(&repo, number))
            .unwrap()
            .unwrap()
            .state,
        PullRequestState::Open
    );
}
