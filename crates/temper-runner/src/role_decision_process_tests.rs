//! Tests for the out-of-process workflow-role decision adapter.
//!
//! Split by responsibility: pure reply/error classification, process I/O
//! behavior, and workspace-backed verdict routing each live in their own
//! submodule and reuse the shared fixtures defined here.

use super::*;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use temper_forge_model::{CreateIssue, CreateRepository, Forge, Issue, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionContext, QueueId, RawWorkflowSpec, RoleId,
    ValidatedWorkflow,
};

use crate::{BoundExternalTool, RoleTools, WorkItem};

#[path = "role_decision_process_tests/classification.rs"]
mod classification;
#[path = "role_decision_process_tests/process_io.rs"]
mod process_io;
#[path = "role_decision_process_tests/workspace_content_routing.rs"]
mod workspace_content_routing;
#[path = "role_decision_process_tests/workspace_review_routing.rs"]
mod workspace_review_routing;
#[path = "role_decision_process_tests/workspace_routing.rs"]
mod workspace_routing;

pub(super) fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "temper-runner-role-decision-{name}-{}-{nanos}",
        std::process::id()
    ))
}

pub(super) struct Fixture {
    pub(super) forge: MemoryForge,
    pub(super) repo: RepositoryId,
    pub(super) workflow: ValidatedWorkflow,
    pub(super) manifest: RoleManifest,
    pub(super) item: WorkItem,
    pub(super) issue: Issue,
}

pub(super) async fn fixture_from_workflow(labels: &[&str], workflow: ValidatedWorkflow) -> Fixture {
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
    let manifest = workflow
        .compile()
        .role(&RoleId::new("banana"))
        .expect("banana role manifest")
        .clone();
    let issue = forge
        .create_issue(
            &repo,
            CreateIssue {
                title: "generic work".to_string(),
                body: "Do the generic thing.".to_string(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created");
    let item = WorkItem {
        queue: QueueId::new("todo"),
        role: RoleId::new("banana"),
        target: ArtifactSource::Issue {
            number: issue.number,
        },
        kind: ArtifactKindId::new("task"),
    };
    Fixture {
        forge,
        repo,
        workflow,
        manifest,
        item,
        issue,
    }
}

pub(super) fn basic_workflow() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Choose advance for todo tasks."},
                "external_tools": [{
                    "id": "coding_workspace",
                    "description": "Edit and commit repository code.",
                    "required": false,
                    "constraints": ["Only touch the checked-out repository."],
                    "guidance": "Use only for PR actions."
                }],
                "queues": ["todo"]
            }],
            "labels": [{"id": "task"}, {"id": "todo"}, {"id": "done"}],
            "artifact_kinds": [{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }],
            "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
            "transitions": [{
                "id": "advance",
                "artifact": "task",
                "roles": ["banana"],
                "effects": [
                    {"kind": "remove_label", "label": "todo"},
                    {"kind": "add_label", "label": "done"}
                ]
            }]
        }"#,
    )
}

pub(super) fn pr_workflow() -> ValidatedWorkflow {
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
                    "guidance": "Produce a real product diff."
                }],
                "queues": ["todo"]
            }],
            "labels": [{"id": "task"}, {"id": "todo"}, {"id": "in-progress"}],
            "artifact_kinds": [{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }],
            "queues": [{"id": "todo", "artifact": "task", "labels": ["todo"]}],
            "transitions": [{
                "id": "open_pr",
                "artifact": "task",
                "roles": ["banana"],
                "effects": [
                    {"kind": "remove_label", "label": "todo"},
                    {"kind": "add_label", "label": "in-progress"},
                    {"kind": "create_pull_request"}
                ]
            }]
        }"#,
    )
}

pub(super) fn parse_workflow(json: &str) -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
    spec.validate().expect("workflow validates")
}

pub(super) fn bound_coding_workspace() -> BoundExternalTool {
    BoundExternalTool {
        id: "coding_workspace".to_string(),
        description: "Edit and commit repository code.".to_string(),
        required: true,
        constraints: vec!["Only touch the checked-out repository.".to_string()],
        guidance: Some("Use before opening implementation PRs.".to_string()),
        provider: "workspace-local".to_string(),
    }
}

pub(super) fn tools(fixture: &Fixture) -> RoleTools<'_, MemoryForge> {
    RoleTools::new(
        &fixture.workflow,
        &fixture.forge,
        &fixture.repo,
        RoleId::new("banana"),
        ExecutionContext::new(),
    )
}

pub(super) async fn labels(fixture: &Fixture) -> Vec<String> {
    let mut labels = fixture
        .forge
        .get_issue_by_number(&fixture.repo, fixture.issue.number)
        .await
        .expect("issue lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

pub(super) fn script_config(script: &str, args: Vec<String>) -> WorkflowRoleDecisionProcessConfig {
    let script_path = temp_path("responder.sh");
    fs::write(&script_path, script).expect("script writes");
    WorkflowRoleDecisionProcessConfig::new("/bin/sh")
        .with_args(std::iter::once(script_path.to_string_lossy().into_owned()).chain(args))
        .with_timeout(Duration::from_secs(2))
}

pub(super) fn inline_config(command: &str) -> WorkflowRoleDecisionProcessConfig {
    WorkflowRoleDecisionProcessConfig::new("/bin/sh")
        .with_args(["-c".to_string(), format!("cat >/dev/null; {command}")])
        .with_timeout(Duration::from_secs(2))
}

pub(super) fn agent(
    cx: temper_engine_io::Cx,
    manifest: RoleManifest,
    config: WorkflowRoleDecisionProcessConfig,
) -> WorkflowRoleDecisionProcessAgent {
    WorkflowRoleDecisionProcessAgent::new(cx, "generic-agent-test", manifest, config)
        .expect("process config validates")
}
