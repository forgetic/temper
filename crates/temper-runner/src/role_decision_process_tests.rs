use super::*;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use temper_forge::{CreateIssue, CreateRepository, Forge, Issue, PullRequestQuery, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionContext, ExternalToolId, QueueId, RawWorkflowSpec,
    RoleId, ValidatedWorkflow,
};

use crate::{
    Agent, BoundExternalTool, CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput,
    CodingWorkspaceRequest, ExternalToolExecutors, RoleTools, WorkItem, REDACTED,
    WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION,
};

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "temper-runner-role-decision-{name}-{}-{nanos}",
        std::process::id()
    ))
}

struct Fixture {
    forge: MemoryForge,
    repo: RepositoryId,
    workflow: ValidatedWorkflow,
    manifest: RoleManifest,
    item: WorkItem,
    issue: Issue,
}

async fn fixture_from_workflow(labels: &[&str], workflow: ValidatedWorkflow) -> Fixture {
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

fn basic_workflow() -> ValidatedWorkflow {
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

fn pr_workflow() -> ValidatedWorkflow {
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

fn parse_workflow(json: &str) -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
    spec.validate().expect("workflow validates")
}

fn bound_coding_workspace() -> BoundExternalTool {
    BoundExternalTool {
        id: "coding_workspace".to_string(),
        description: "Edit and commit repository code.".to_string(),
        required: true,
        constraints: vec!["Only touch the checked-out repository.".to_string()],
        guidance: Some("Use before opening implementation PRs.".to_string()),
        provider: "workspace-local".to_string(),
    }
}

fn tools<'a>(fixture: &'a Fixture) -> RoleTools<'a, MemoryForge> {
    RoleTools::new(
        &fixture.workflow,
        &fixture.forge,
        &fixture.repo,
        RoleId::new("banana"),
        ExecutionContext::new(),
    )
}

async fn labels(fixture: &Fixture) -> Vec<String> {
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

fn script_config(script: &str, args: Vec<String>) -> WorkflowRoleDecisionProcessConfig {
    let script_path = temp_path("responder.sh");
    fs::write(&script_path, script).expect("script writes");
    WorkflowRoleDecisionProcessConfig::new("/bin/sh")
        .with_args(std::iter::once(script_path.to_string_lossy().into_owned()).chain(args))
        .with_timeout(Duration::from_secs(2))
}

fn inline_config(command: &str) -> WorkflowRoleDecisionProcessConfig {
    WorkflowRoleDecisionProcessConfig::new("/bin/sh")
        .with_args(["-c".to_string(), format!("cat >/dev/null; {command}")])
        .with_timeout(Duration::from_secs(2))
}

fn agent(
    manifest: RoleManifest,
    config: WorkflowRoleDecisionProcessConfig,
) -> WorkflowRoleDecisionProcessAgent {
    WorkflowRoleDecisionProcessAgent::new("generic-agent-test", manifest, config)
        .expect("process config validates")
}

#[test]
fn decision_reply_classification_distinguishes_adapter_branches() {
    let request: WorkflowRoleDecisionRequest = serde_json::from_str(include_str!(
        "../fixtures/workflow-role-decision-request.json"
    ))
    .expect("request fixture parses");

    let authorized = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::action("advance", "authorized"),
    );
    assert_eq!(authorized.validation_outcome, "valid");
    assert_eq!(authorized.action_kind, "authorized_action");
    assert_eq!(authorized.disposition, DecisionDisposition::ExecuteAction);

    let no_action = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::no_action("nothing useful to do"),
    );
    assert_eq!(no_action.validation_outcome, "valid");
    assert_eq!(no_action.action_kind, "no_action");
    assert_eq!(no_action.disposition, DecisionDisposition::NoAction);

    let unauthorized = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply::action("delete_everything", "bad idea"),
    );
    assert_eq!(
        unauthorized.validation_outcome,
        "unauthorized_downgraded_to_no_action"
    );
    assert_eq!(unauthorized.action_kind, "no_action");
    assert_eq!(unauthorized.disposition, DecisionDisposition::NoAction);

    let mismatch = classify_decision_reply(
        &request,
        &WorkflowRoleDecisionReply {
            protocol_version: 999,
            action: "advance".to_string(),
            reason: "old responder".to_string(),
        },
    );
    assert_eq!(mismatch.validation_outcome, "protocol_mismatch");
    assert_eq!(mismatch.action_kind, "invalid_reply");
    assert_eq!(mismatch.disposition, DecisionDisposition::Error);
    assert!(mismatch
        .error
        .expect("mismatch keeps protocol error")
        .contains("version mismatch"));
}

#[test]
fn process_error_classification_distinguishes_failure_branches() {
    let malformed_json = serde_json::from_str::<WorkflowRoleDecisionReply>("not-json")
        .expect_err("bad reply is malformed");
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::MalformedJson {
            source: malformed_json,
        }),
        ("malformed_json", "invalid_reply")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Timeout {
            timeout: Duration::from_millis(10),
        }),
        ("timeout", "process_unavailable")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Protocol(
            WorkflowRoleDecisionProtocolError::VersionMismatch {
                expected: 1,
                actual: 999,
            },
        )),
        ("protocol_mismatch", "invalid_reply")
    );
    assert_eq!(
        classify_process_error(&WorkflowRoleDecisionProcessError::Exit {
            status: "exit status: 7".to_string(),
            stderr: "stderr preview".to_string(),
        }),
        ("process_failure", "process_unavailable")
    );
}

#[tokio::test]
async fn process_agent_sends_request_filters_environment_and_executes_action() {
    let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
    let request_path = temp_path("request.json");
    let config = script_config(
        r#"cat > "$1"
if [ "${TEMPER_RUNNER_ROLE_DECISION_ALLOWED:-}" = "allowed-value" ] && [ -z "${TEMPER_RUNNER_ROLE_DECISION_BLOCKED:-}" ]; then
  printf '%s\n' '{"protocol_version":1,"action":"advance","reason":"env-ok"}'
else
  printf '%s\n' '{"protocol_version":1,"action":"no_action","reason":"env-leaked"}'
fi
"#,
        vec![request_path.to_string_lossy().into_owned()],
    )
    .with_env_allowlist(["TEMPER_RUNNER_ROLE_DECISION_ALLOWED"]);
    std::env::set_var("TEMPER_RUNNER_ROLE_DECISION_ALLOWED", "allowed-value");
    std::env::set_var("TEMPER_RUNNER_ROLE_DECISION_BLOCKED", "blocked-value");
    let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools(
        "generic-agent-test",
        fixture.manifest.clone(),
        config,
        vec![bound_coding_workspace()],
    )
    .expect("process config validates");

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(changed);
    assert_eq!(labels(&fixture).await, vec!["done", "task"]);
    let captured: WorkflowRoleDecisionRequest =
        serde_json::from_str(&fs::read_to_string(&request_path).expect("request captured"))
            .expect("captured request parses");
    assert_eq!(
        captured.protocol_version,
        WORKFLOW_ROLE_DECISION_PROTOCOL_VERSION
    );
    assert_eq!(captured.workflow_id, "generic-agent-test");
    assert_eq!(captured.work_item_context["role"], "banana");
    assert_eq!(
        captured.work_item_context["artifact"]["title"],
        "generic work"
    );
    let observability = captured.work_item_context["observability"]
        .as_object()
        .expect("observability context is an object");
    assert_eq!(observability["repo"], fixture.repo.to_string());
    assert_eq!(observability["role"], "banana");
    assert_eq!(observability["queue"], "todo");
    assert_eq!(observability["artifact_type"], "issue");
    assert_eq!(observability["artifact_number"], fixture.issue.number.get());
    assert_eq!(observability["artifact_kind"], "task");
    assert!(observability["work_item_id"]
        .as_str()
        .expect("work item id is a string")
        .contains("artifact:issue:1"));
    assert!(observability["decision_id"]
        .as_str()
        .expect("decision id is a string")
        .starts_with("decision/work-item/"));
    assert!(observability.get("tick_id").is_none());
    assert_eq!(captured.authorized_actions[0].action, "advance");
    assert_eq!(
        captured.available_external_tools[0].provider,
        "workspace-local"
    );
    std::env::remove_var("TEMPER_RUNNER_ROLE_DECISION_ALLOWED");
    std::env::remove_var("TEMPER_RUNNER_ROLE_DECISION_BLOCKED");
}

#[tokio::test]
async fn process_agent_treats_unauthorized_action_as_no_action() {
    let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
    let agent = agent(
        fixture.manifest.clone(),
        inline_config(
            r#"printf '%s' '{"protocol_version":1,"action":"delete_everything","reason":"bad"}'"#,
        ),
    );

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("unauthorized action degrades to no-action");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn process_agent_reports_timeout_exit_and_malformed_replies() {
    let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
    let cases = [
        (
            WorkflowRoleDecisionProcessConfig::new("/bin/sh")
                .with_args(["-c".to_string(), "cat >/dev/null; sleep 1".to_string()])
                .with_timeout(Duration::from_millis(20)),
            "timed out",
        ),
        (
            inline_config("printf 'bad news' >&2; exit 7"),
            "exited unsuccessfully",
        ),
        (
            inline_config(
                r#"printf '%s' '{"protocol_version":1,"action":"advance"}{"protocol_version":1,"action":"advance"}'"#,
            ),
            "malformed JSON",
        ),
        (
            inline_config(r#"printf '%s' '{"protocol_version":1,"action":"advance","extra":1}'"#),
            "unknown field",
        ),
        (
            inline_config(
                r#"printf '%s' '{"protocol_version":1,"action":"advance","action":"advance"}'"#,
            ),
            "duplicate field",
        ),
        (
            inline_config(r#"printf '%s' '{"protocol_version":999,"action":"advance"}'"#),
            "version mismatch",
        ),
    ];

    for (config, expected) in cases {
        let error = agent(fixture.manifest.clone(), config)
            .service(&fixture.item, &tools(&fixture))
            .await
            .expect_err("process failure is an agent error");
        assert!(
            error.to_string().contains(expected),
            "expected `{expected}` in `{error}`"
        );
    }
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn process_agent_redacts_secret_like_stderr() {
    let fixture = fixture_from_workflow(&["task", "todo"], basic_workflow()).await;
    let error = agent(
        fixture.manifest.clone(),
        inline_config("printf 'token=super-secret' >&2; exit 7"),
    )
    .service(&fixture.item, &tools(&fixture))
    .await
    .expect_err("process failure is an agent error");
    let rendered = error.to_string();

    assert!(rendered.contains(REDACTED));
    assert!(!rendered.contains("super-secret"));
    assert!(!rendered.contains("token=super-secret"));
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

#[tokio::test]
async fn process_agent_uses_coding_workspace_for_pr_actions() {
    let fixture = fixture_from_workflow(&["task", "todo"], pr_workflow()).await;
    let workspace = Arc::new(FixtureWorkspace::default());
    let workspace_provider: Arc<dyn CodingWorkspace> = workspace.clone();
    let executors = ExternalToolExecutors::new().with_coding_workspace(
        RoleId::new("banana"),
        ExternalToolId::new("coding_workspace"),
        workspace_provider,
    );
    let agent = WorkflowRoleDecisionProcessAgent::with_bound_external_tools_and_executors(
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
    assert!(pull_requests[0]
        .body
        .contains("updated docs/product-change.md"));
}
