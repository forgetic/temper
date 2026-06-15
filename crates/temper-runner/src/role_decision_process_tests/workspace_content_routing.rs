//! Workspace verdicts that route to content-bearing transitions: body rewrites
//! (`set_body`) and issue breakdowns (`create_issues`).

use super::*;

use std::sync::Arc;

use async_trait::async_trait;
use temper_forge::PullRequestQuery;

use crate::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
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
