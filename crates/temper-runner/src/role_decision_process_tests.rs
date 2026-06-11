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

#[test]
 fn process_agent_sends_request_filters_environment_and_executes_action() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn process_agent_treats_unauthorized_action_as_no_action() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn process_agent_reports_timeout_exit_and_malformed_replies() {
    temper_io_engine::block_on(async move {
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
    })
}

#[test]
 fn process_agent_redacts_secret_like_stderr() {
    temper_io_engine::block_on(async move {
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
    })
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
    temper_io_engine::block_on(async move {
    let fixture = fixture_from_workflow(&["task", "todo"], pr_workflow()).await;
    let workspace = Arc::new(FixtureWorkspace::default());
    let workspace_provider: Arc<dyn CodingWorkspace> = workspace.clone();
    let executors = ExternalToolExecutors::new().with_workspace(
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
    temper_io_engine::block_on(async move {
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

async fn issue_body(fixture: &Fixture) -> String {
    fixture
        .forge
        .get_issue_by_number(&fixture.repo, fixture.issue.number)
        .await
        .expect("issue lookup succeeds")
        .expect("issue exists")
        .body
}

#[test]
 fn workspace_verdict_routes_to_set_body_and_writes_the_authored_body() {
    temper_io_engine::block_on(async move {
    let fixture = fixture_from_workflow(&["task", "todo"], set_body_workflow_with_outcomes()).await;
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
    temper_io_engine::block_on(async move {
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
    assert!(ui_meta
        .dependencies
        .contains(&temper_workflow::ArtifactRef::same_repo(api_number)));

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
    temper_io_engine::block_on(async move {
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
    temper_io_engine::block_on(async move {
    use temper_forge::{BranchRef, CreatePullRequest, PullRequestQuery};

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
