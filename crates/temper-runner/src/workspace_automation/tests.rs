use super::*;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use temper_forge_model::{CreateIssue, CreateRepository, Forge, IssueQuery};
use temper_forge_memory::MemoryForge;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, CreateIssuesChild, ExternalToolId, QueueId,
    RawWorkflowSpec, RoleId, TransitionId, ValidatedWorkflow, VerdictId, parse_metadata_block,
};

use crate::scan::AutomatedWorkItem;
use crate::{
    CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput, CodingWorkspaceRequest,
    ExternalToolExecutors,
};

/// A queue automation whose `needs_breakdown` verdict routes to a transition
/// that fans the intake out into dependent children via `create_issues`.
const BREAKDOWN_WORKFLOW: &str = r#"{
    "name": "architect-automation",
    "roles": [{
        "id": "architect",
        "external_tools": [{
            "id": "coding_workspace",
            "description": "Analyze and author a breakdown.",
            "required": true,
            "constraints": ["Only touch the checked-out repository."],
            "guidance": "Break the intake into children."
        }]
    }],
    "labels": [{"id": "intake"}, {"id": "triaging"}, {"id": "planned"}, {"id": "code"}, {"id": "ready"}],
    "artifact_kinds": [{"id": "epic", "target": "issue", "identifying_labels": ["intake"]}],
    "queues": [{"id": "intake_triage", "artifact": "epic", "labels": ["triaging"]}],
    "transitions": [
        {
            "id": "triage_intake",
            "artifact": "epic",
            "roles": ["architect"],
            "outcomes": {"needs_breakdown": "triage_intake_breakdown"},
            "effects": [{"kind": "remove_label", "label": "triaging"}]
        },
        {
            "id": "triage_intake_breakdown",
            "artifact": "epic",
            "roles": ["architect"],
            "effects": [
                {"kind": "create_issues"},
                {"kind": "remove_label", "label": "triaging"},
                {"kind": "add_label", "label": "planned"}
            ]
        }
    ]
}"#;

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(BREAKDOWN_WORKFLOW).expect("json parses");
    spec.validate().expect("workflow validates")
}

/// A workspace that returns a verdict plus authored dependent children, with
/// no diff — the architect breakdown shape on the automation path.
struct ChildrenWorkspace {
    verdict: VerdictId,
    children: Vec<CreateIssuesChild>,
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
fn automation_verdict_routes_to_create_issues_and_fans_out_children() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repo = forge
            .create_repository(CreateRepository {
                owner: "acme".to_string(),
                name: "service".to_string(),
                default_branch: "main".to_string(),
                description: None,
            })
            .await
            .expect("repo created")
            .id;
        let epic = forge
            .create_issue(
                &repo,
                CreateIssue {
                    title: "Epic".to_string(),
                    body: "raw human epic".to_string(),
                    labels: vec!["intake".to_string(), "triaging".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("epic created")
            .number;

        let workflow = workflow();
        let compiled = workflow.compile();
        let workspace: Arc<dyn CodingWorkspace> = Arc::new(ChildrenWorkspace {
            verdict: VerdictId::new("needs_breakdown"),
            children: vec![
                CreateIssuesChild::new("api", "Add the API", "Author the API.")
                    .with_labels(["code", "ready"]),
                CreateIssuesChild::new("ui", "Add the UI", "Consume the API.")
                    .with_labels(["code", "ready"])
                    .with_dependencies(["api"]),
            ],
        });
        let executors = ExternalToolExecutors::new().with_workspace(
            RoleId::new("architect"),
            ExternalToolId::new("coding_workspace"),
            workspace,
        );

        let item = AutomatedWorkItem {
            queue: QueueId::new("intake_triage"),
            actor: RoleId::new("architect"),
            transition: TransitionId::new("triage_intake"),
            executor: Some(ExternalToolId::new("coding_workspace")),
            outcomes: BTreeMap::from([(
                VerdictId::new("needs_breakdown"),
                TransitionId::new("triage_intake_breakdown"),
            )]),
            target: ArtifactSource::Issue { number: epic },
            kind: ArtifactKindId::new("epic"),
        };

        let outcome =
            execute_workspace_automation(&workflow, &compiled, &executors, &forge, &repo, &item)
                .await
                .expect("workspace automation routes to the breakdown");

        assert!(matches!(
            outcome,
            WorkspaceAutomationOutcome::Applied { routed }
                if routed == TransitionId::new("triage_intake_breakdown")
        ));

        // The children fanned out under the intake as parent, the sibling
        // dependency was recorded, and the parent's `planned` label flip applied.
        let parent_ref = ArtifactRef::same_repo(epic);
        let mut created: Vec<_> = forge
            .list_issues(&repo, IssueQuery::default())
            .await
            .expect("issues list")
            .into_iter()
            .filter(|issue| {
                parse_metadata_block(&issue.body)
                    .ok()
                    .flatten()
                    .is_some_and(|metadata| metadata.parents.contains(&parent_ref))
            })
            .collect();
        created.sort_by(|a, b| a.title.cmp(&b.title));
        assert_eq!(created.len(), 2, "two children fanned out under the intake");
        assert_eq!(created[0].title, "Add the API");
        assert_eq!(created[1].title, "Add the UI");

        let api_number = created[0].number;
        let ui_meta = parse_metadata_block(&created[1].body)
            .expect("metadata parses")
            .expect("metadata exists");
        assert!(
            ui_meta
                .dependencies
                .contains(&ArtifactRef::same_repo(api_number))
        );

        let parent = forge
            .get_issue_by_number(&repo, epic)
            .await
            .expect("lookup")
            .expect("epic exists");
        assert!(parent.labels.iter().any(|label| label == "planned"));
    })
}
