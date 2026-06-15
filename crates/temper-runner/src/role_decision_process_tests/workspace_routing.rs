//! Workspace-backed verdict routing for PR heads, escalations, and undeclared
//! verdicts.

use super::*;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use temper_forge::{Forge, PullRequestQuery};

use crate::{
    Agent, CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    ExternalToolExecutors,
};
use temper_workflow::ExternalToolId;

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
