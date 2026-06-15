//! Workspace-backed verdict routing: PR heads, escalations, content rewrites,
//! issue breakdowns, undeclared verdicts, and native reviews.

use super::*;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use temper_forge::{Forge, PullRequestQuery};

use crate::{
    Agent, CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    ExternalToolExecutors,
};
use temper_workflow::ExternalToolId;

async fn issue_body(fixture: &Fixture) -> String {
    fixture
        .forge
        .get_issue_by_number(&fixture.repo, fixture.issue.number)
        .await
        .expect("issue lookup succeeds")
        .expect("issue exists")
        .body
}

#[derive(Default)]
struct FixtureWorkspace {
    requests: Mutex<Vec<CodingWorkspaceRequest>>,
}

impl FixtureWorkspace {
    fn requests(&self) -> Vec<CodingWorkspaceRequest> {
        self.requests
            .lock()
            .expect("workspace request mutex is not poisoned")
            .clone()
    }
}

#[async_trait]
impl CodingWorkspace for FixtureWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        self.requests
            .lock()
            .expect("workspace request mutex is not poisoned")
            .push(request.clone());
        Ok(CodingWorkspaceOutput::new(
            request.branch_hint,
            request.base_branch,
            "updated docs/product-change.md",
            vec!["docs/product-change.md".to_string()],
            vec![
                "implementation".to_string(),
                "needs-reviewer".to_string(),
                "needs-merge".to_string(),
            ],
        ))
    }
}

#[test]
fn process_agent_uses_coding_workspace_for_pr_actions() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], pr_workflow()).await;
        let workspace = Arc::new(FixtureWorkspace::default());
        let workspace_provider: Arc<dyn CodingWorkspace> = workspace.clone();
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("coding_workspace"),
            workspace_provider,
        );
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"open_pr","reason":"workspace ready"}'"#,
        ),
        vec![bound_coding_workspace()],
        executors,
    )
    .expect("process config validates");

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("workspace-backed PR create succeeds");

        assert!(changed);
        assert_eq!(labels(&fixture).await, vec!["in-progress", "task"]);
        let requests = workspace.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].branch_hint, "agent/pr-for-task-1");
        assert!(requests[0].work_item.context_json.contains("generic work"));
        let pull_requests = fixture
            .forge
            .list_pull_requests(&fixture.repo, PullRequestQuery::default())
            .await
            .expect("PR list succeeds");
        assert_eq!(pull_requests.len(), 1);
        assert_eq!(pull_requests[0].source.branch, "agent/pr-for-task-1");
        assert!(
            pull_requests[0]
                .body
                .contains("updated docs/product-change.md")
        );
    })
}

/// An `open_pr` action that declares a `needs_architect` verdict routing to an
/// escalation transition on the same artifact/role.
fn pr_workflow_with_outcomes() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Use open_pr when coding_workspace is available."},
                "external_tools": [{
                    "id": "coding_workspace",
                    "description": "Edit and commit repository code.",
                    "required": true,
                    "constraints": ["Only touch the checked-out repository."],
                    "guidance": "Produce a real product diff or escalate."
                }],
                "queues": ["todo"]
            }],
            "labels": [{"id": "task"}, {"id": "todo"}, {"id": "in-progress"}, {"id": "needs-architect"}],
            "artifact_kinds": [{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }],
            "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
            "transitions": [
                {
                    "id": "open_pr",
                    "artifact": "task",
                    "roles": ["banana"],
                    "outcomes": {"needs_architect": "escalate"},
                    "effects": [
                        {"kind": "remove_label", "label": "todo"},
                        {"kind": "add_label", "label": "in-progress"},
                        {"kind": "create_pull_request"}
                    ]
                },
                {
                    "id": "escalate",
                    "artifact": "task",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "remove_label", "label": "todo"},
                        {"kind": "add_label", "label": "needs-architect"}
                    ]
                }
            ]
        }"#,
    )
}

/// A workspace that returns a verdict (and no usable diff) instead of a head.
struct VerdictWorkspace {
    verdict: temper_workflow::VerdictId,
    changed_files: Vec<String>,
}

#[async_trait]
impl CodingWorkspace for VerdictWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        Ok(CodingWorkspaceOutput::new(
            request.branch_hint,
            request.base_branch,
            "no implementable diff; escalating",
            self.changed_files.clone(),
            Vec::new(),
        )
        .with_verdict(self.verdict.clone()))
    }
}

#[test]
fn workspace_verdict_routes_open_pr_to_escalation_without_pr_create() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], pr_workflow_with_outcomes()).await;
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(VerdictWorkspace {
            verdict: temper_workflow::VerdictId::new("needs_architect"),
            // Empty diff is the escalation signal, not an error.
            changed_files: Vec::new(),
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"open_pr","reason":"workspace ready"}'"#,
        ),
        vec![bound_coding_workspace()],
        executors,
    )
    .expect("process config validates");

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("verdict routing succeeds");

        assert!(changed);
        // The escalation transition ran (needs-architect added, todo removed),
        // NOT open_pr (no in-progress label).
        assert_eq!(labels(&fixture).await, vec!["needs-architect", "task"]);
        let pull_requests = fixture
            .forge
            .list_pull_requests(&fixture.repo, PullRequestQuery::default())
            .await
            .expect("PR list succeeds");
        assert!(
            pull_requests.is_empty(),
            "escalation must not open a pull request"
        );
    })
}

/// An `open_pr`-style action whose `ready_code` verdict routes to a content
/// transition that rewrites the artifact body via `set_body`.
fn set_body_workflow_with_outcomes() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Triage the intake."},
                "external_tools": [{
                    "id": "coding_workspace",
                    "description": "Analyze and author content.",
                    "required": true,
                    "constraints": ["Only touch the checked-out repository."],
                    "guidance": "Rewrite the intake into a crisp spec or escalate."
                }],
                "queues": ["todo"]
            }],
            "labels": [{"id": "task"}, {"id": "todo"}, {"id": "in-progress"}, {"id": "ready"}],
            "artifact_kinds": [{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }],
            "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
            "transitions": [
                {
                    "id": "triage_intake",
                    "artifact": "task",
                    "roles": ["banana"],
                    "outcomes": {"ready_code": "rewrite_body"},
                    "effects": [
                        {"kind": "remove_label", "label": "todo"}
                    ]
                },
                {
                    "id": "rewrite_body",
                    "artifact": "task",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "set_body"},
                        {"kind": "remove_label", "label": "todo"},
                        {"kind": "add_label", "label": "ready"}
                    ]
                }
            ]
        }"#,
    )
}

/// A workspace that returns a verdict plus an authored body (the `set_body`
/// work product), with no implementable diff.
struct BodyWorkspace {
    verdict: temper_workflow::VerdictId,
    body: String,
}

#[async_trait]
impl CodingWorkspace for BodyWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        Ok(CodingWorkspaceOutput::new(
            request.branch_hint,
            request.base_branch,
            "rewrote intake into a crisp spec",
            Vec::new(),
            Vec::new(),
        )
        .with_verdict(self.verdict.clone())
        .with_body(self.body.clone()))
    }
}

#[test]
fn workspace_verdict_routes_to_set_body_and_writes_the_authored_body() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture =
            fixture_from_workflow(&["task", "todo"], set_body_workflow_with_outcomes()).await;
        let authored = "# Crisp spec\n\nImplementable, authored by the architect workspace.";
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(BodyWorkspace {
            verdict: temper_workflow::VerdictId::new("ready_code"),
            body: authored.to_string(),
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"triage_intake","reason":"intake ready"}'"#,
        ),
        vec![bound_coding_workspace()],
        executors,
    )
    .expect("process config validates");

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("verdict routing to set_body succeeds");

        assert!(changed);
        // The routed `rewrite_body` transition ran: the authored body was written
        // and the `ready` label added. The triage action declares no
        // `create_pull_request` effect (it never opens a PR) yet still dispatched its
        // workspace, because it is workspace-backed by its `outcomes` declaration.
        assert_eq!(issue_body(&fixture).await, authored);
        assert_eq!(labels(&fixture).await, vec!["ready", "task"]);
        let pull_requests = fixture
            .forge
            .list_pull_requests(&fixture.repo, PullRequestQuery::default())
            .await
            .expect("PR list succeeds");
        assert!(
            pull_requests.is_empty(),
            "a content rewrite must not open a pull request"
        );
    })
}

/// A triage action whose `needs_breakdown` verdict routes to a transition that
/// fans the intake out into dependent children via `create_issues`.
fn create_issues_workflow_with_outcomes() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Triage the intake."},
                "external_tools": [{
                    "id": "coding_workspace",
                    "description": "Analyze and author a breakdown.",
                    "required": true,
                    "constraints": ["Only touch the checked-out repository."],
                    "guidance": "Rewrite the intake or break it into children."
                }],
                "queues": ["todo"]
            }],
            "labels": [{"id": "task"}, {"id": "todo"}, {"id": "planned"}, {"id": "code"}, {"id": "ready"}],
            "artifact_kinds": [{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }],
            "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
            "transitions": [
                {
                    "id": "triage_intake",
                    "artifact": "task",
                    "roles": ["banana"],
                    "outcomes": {"needs_breakdown": "triage_intake_breakdown"},
                    "effects": [
                        {"kind": "remove_label", "label": "todo"}
                    ]
                },
                {
                    "id": "triage_intake_breakdown",
                    "artifact": "task",
                    "roles": ["banana"],
                    "effects": [
                        {"kind": "create_issues"},
                        {"kind": "remove_label", "label": "todo"},
                        {"kind": "add_label", "label": "planned"}
                    ]
                }
            ]
        }"#,
    )
}

/// A workspace that returns a verdict plus authored dependent children (the
/// `create_issues` work product), with no implementable diff.
struct ChildrenWorkspace {
    verdict: temper_workflow::VerdictId,
    children: Vec<temper_workflow::CreateIssuesChild>,
}

#[async_trait]
impl CodingWorkspace for ChildrenWorkspace {
    async fn produce_head(
        &self,
        request: CodingWorkspaceRequest,
    ) -> Result<CodingWorkspaceOutput, CodingWorkspaceError> {
        Ok(CodingWorkspaceOutput::new(
            request.branch_hint,
            request.base_branch,
            "broke the intake into dependent children",
            Vec::new(),
            Vec::new(),
        )
        .with_verdict(self.verdict.clone())
        .with_children(self.children.clone()))
    }
}

#[test]
fn workspace_verdict_routes_to_create_issues_and_fans_out_children() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture =
            fixture_from_workflow(&["task", "todo"], create_issues_workflow_with_outcomes()).await;
        let children = vec![
            temper_workflow::CreateIssuesChild::new("api", "Add the API", "Author the API.")
                .with_labels(["code", "ready"]),
            temper_workflow::CreateIssuesChild::new("ui", "Add the UI", "Consume the API.")
                .with_labels(["code", "ready"])
                .with_dependencies(["api"]),
        ];
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(ChildrenWorkspace {
            verdict: temper_workflow::VerdictId::new("needs_breakdown"),
            children,
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"triage_intake","reason":"needs breakdown"}'"#,
        ),
        vec![bound_coding_workspace()],
        executors,
    )
    .expect("process config validates");

        let changed = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect("verdict routing to create_issues succeeds");

        assert!(changed);

        // The routed `triage_intake_breakdown` transition ran: the authored children
        // fanned out under the intake as parent, with the sibling dependency
        // recorded, and the parent's co-declared `planned` label flip applied.
        let parent_ref = temper_workflow::ArtifactRef::same_repo(fixture.issue.number);
        let mut created: Vec<_> = fixture
            .forge
            .list_issues(&fixture.repo, temper_forge::IssueQuery::default())
            .await
            .expect("issues list")
            .into_iter()
            .filter(|issue| {
                temper_workflow::parse_metadata_block(&issue.body)
                    .ok()
                    .flatten()
                    .is_some_and(|metadata| metadata.parents.contains(&parent_ref))
            })
            .collect();
        created.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(created.len(), 2, "two children fanned out under the intake");
        assert_eq!(created[0].title, "Add the API");
        assert_eq!(created[1].title, "Add the UI");

        // The UI child depends on the API child's number.
        let api_number = created[0].number;
        let ui_meta = temper_workflow::parse_metadata_block(&created[1].body)
            .expect("metadata parses")
            .expect("metadata exists");
        assert!(
            ui_meta
                .dependencies
                .contains(&temper_workflow::ArtifactRef::same_repo(api_number))
        );

        assert_eq!(labels(&fixture).await, vec!["planned", "task"]);
        let pull_requests = fixture
            .forge
            .list_pull_requests(&fixture.repo, PullRequestQuery::default())
            .await
            .expect("PR list succeeds");
        assert!(
            pull_requests.is_empty(),
            "a breakdown must not open a pull request"
        );
    })
}

#[test]
fn workspace_undeclared_verdict_is_an_error() {
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        let fixture = fixture_from_workflow(&["task", "todo"], pr_workflow_with_outcomes()).await;
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(VerdictWorkspace {
            verdict: temper_workflow::VerdictId::new("unknown_verdict"),
            changed_files: vec!["docs/change.md".to_string()],
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("banana"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );
        let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
        cx.clone(),
        "generic-agent-test",
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"open_pr","reason":"workspace ready"}'"#,
        ),
        vec![bound_coding_workspace()],
        executors,
    )
    .expect("process config validates");

        let error = agent
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect_err("an undeclared verdict is an error");
        assert!(
            error.to_string().contains("undeclared verdict"),
            "expected undeclared-verdict error, got `{error}`"
        );
        // No transition applied.
        assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
    })
}

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
