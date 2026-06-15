//! Workspace verdicts that route a `review_pr` action to a native review
//! (`attach_review`) without ever producing a pull-request head.

use super::*;

use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    ExternalToolExecutors,
};
use temper_workflow::ExternalToolId;

/// A `review_pr`-style action: it targets a **pull request**, declares no
/// `create_pull_request` effect (a reviewer never opens a PR), and is
/// workspace-backed purely by its `outcomes` declaration. Its `changes` verdict
/// routes to a transition that attaches a native review carrying the authored
/// review body.
fn review_pr_workflow_with_outcomes() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Use review_pr to review the PR."},
                "external_tools": [{
                    "id": "review_workspace",
                    "description": "Read the diff and CI, then return a verdict.",
                    "required": true,
                    "constraints": ["Read-only checkout."],
                    "guidance": "Decide from the diff and CI, not the PR summary."
                }],
                "queues": ["review"]
            }],
            "labels": [{"id": "pr"}, {"id": "needs-reviewer"}, {"id": "changes"}],
            "artifact_kinds": [{
                "id": "implementation_pr",
                "target": "pull_request",
                "identifying_labels": ["pr"]
            }],
            "queues": [{"id": "review", "artifact": "implementation_pr", "labels": ["needs-reviewer"]}],
            "transitions": [
                {
                    "id": "review_pr",
                    "artifact": "implementation_pr",
                    "roles": ["banana"],
                    "outcomes": {
                        "approve": "approve_review",
                        "changes": "request_changes_with_review",
                        "escalate": "escalate_review"
                    },
                    "effects": [
                        {"kind": "remove_label", "label": "needs-reviewer"}
                    ]
                },
                {
                    "id": "approve_review",
                    "artifact": "implementation_pr",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "remove_label", "label": "needs-reviewer"},
                        {"kind": "submit_review", "decision": "approved"}
                    ]
                },
                {
                    "id": "request_changes_with_review",
                    "artifact": "implementation_pr",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "remove_label", "label": "needs-reviewer"},
                        {"kind": "attach_review", "decision": "changes_requested"},
                        {"kind": "add_label", "label": "changes"}
                    ]
                },
                {
                    "id": "escalate_review",
                    "artifact": "implementation_pr",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "remove_label", "label": "needs-reviewer"}
                    ]
                }
            ]
        }"#,
    )
}

/// A review workspace that returns a `changes` verdict plus an authored review
/// body (the `attach_review` work product). It never produces a diff.
struct ReviewWorkspace {
    verdict: temper_workflow::VerdictId,
    review_body: String,
}

#[async_trait]
impl CodingWorkspace for ReviewWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        Ok(CodingWorkspaceOutput::new(
            request.branch_hint,
            request.base_branch,
            "reviewed the diff and requested changes",
            Vec::new(),
            Vec::new(),
        )
        .with_verdict(self.verdict.clone())
        .with_review_body(self.review_body.clone()))
    }
}

#[test]
fn workspace_verdict_routes_review_pr_to_native_review_without_pr_create() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        use temper_forge::{BranchRef, CreatePullRequest, CreateRepository, PullRequestQuery};

        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repo is created")
            .id;
        let workflow = review_pr_workflow_with_outcomes();
        let manifest = workflow
            .compile()
            .role(&RoleId::new("banana"))
            .expect("banana role manifest")
            .clone();
        let pull_request = forge
            .create_pull_request(
                &repo,
                CreatePullRequest {
                    title: "Implement the thing".to_string(),
                    body: "A real diff under review.".to_string(),
                    source: BranchRef {
                        repository_id: repo.clone(),
                        branch: "agent/work".to_string(),
                    },
                    target: BranchRef {
                        repository_id: repo.clone(),
                        branch: "main".to_string(),
                    },
                    labels: vec!["pr".to_string(), "needs-reviewer".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("PR is created");
        let item = WorkItem {
            queue: QueueId::new("review"),
            role: RoleId::new("banana"),
            target: ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            kind: ArtifactKindId::new("implementation_pr"),
        };

        let authored = "These changes need a test for the empty-input case.";
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(ReviewWorkspace {
            verdict: temper_workflow::VerdictId::new("changes"),
            review_body: authored.to_string(),
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("review_workspace"),
            workspace,
        );
        let bound_review_workspace = BoundExternalTool {
            id: "review_workspace".to_string(),
            description: "Read the diff and CI, then return a verdict.".to_string(),
            required: true,
            constraints: vec!["Read-only checkout.".to_string()],
            guidance: Some("Decide from the diff and CI, not the PR summary.".to_string()),
            provider: "workspace-local".to_string(),
        };
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        manifest,
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"review_pr","reason":"workspace ready"}'"#,
        ),
        vec![bound_review_workspace],
        executors,
    )
    .expect("process config validates");

        let role_tools = RoleTools::new(
            &workflow,
            &forge,
            &repo,
            RoleId::new("banana"),
            ExecutionContext::new(),
        );
        let changed = agent
            .service(&item, &role_tools)
            .await
            .expect("review_pr dispatches its workspace and routes the verdict");

        assert!(changed);

        // The routed `request_changes_with_review` transition ran: the
        // `needs-reviewer` label was removed, `changes` added, and a native review
        // carrying the authored body was attached. The `review_pr` action declares
        // no `create_pull_request` effect, yet it still dispatched its workspace
        // because it is workspace-backed by its `outcomes` declaration.
        let reviewed = forge
            .get_pull_request_by_number(&repo, pull_request.number)
            .await
            .expect("PR lookup succeeds")
            .expect("PR exists");
        let mut pr_labels = reviewed.labels.clone();
        pr_labels.sort();
        assert_eq!(pr_labels, vec!["changes".to_string(), "pr".to_string()]);

        let reviews = forge
            .list_pull_request_reviews(&reviewed.id)
            .await
            .expect("review list succeeds");
        assert_eq!(reviews.len(), 1, "exactly one native review was attached");
        assert_eq!(
            reviews[0].decision,
            temper_forge::ReviewDecision::ChangesRequested
        );
        // The native review carries the workspace-authored body. `attach_review`
        // appends an idempotency marker comment, so assert the authored text leads.
        let review_body = reviews[0].body.as_deref().expect("review carries a body");
        assert!(
            review_body.starts_with(authored),
            "review body should carry the authored text, got `{review_body}`"
        );

        // No pull request was opened: the reviewer routes a verdict, it never
        // produces a head.
        let pull_requests = forge
            .list_pull_requests(&repo, PullRequestQuery::default())
            .await
            .expect("PR list succeeds");
        assert_eq!(
            pull_requests.len(),
            1,
            "review must not open a new pull request"
        );
    })
}
