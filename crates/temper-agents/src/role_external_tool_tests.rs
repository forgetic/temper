use super::*;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use temper_forge::{CreateIssue, CreateRepository, Forge, Issue, PullRequestQuery, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_runner::{
    BoundExternalTool, CodingWorkspace, CodingWorkspaceError, CodingWorkspaceOutput,
    CodingWorkspaceRequest, ExternalToolExecutors, RoleTools, WorkItem,
};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionContext, ExternalToolId, QueueId, RawWorkflowSpec,
    RoleId, ValidatedWorkflow,
};

#[derive(Debug)]
enum ScriptedOutcome {
    Decision(RoleDecision),
}

#[derive(Debug, Default)]
struct CapturedCall {
    system_prompt: String,
    user_context: String,
}

#[derive(Debug)]
struct ScriptedDecisionEngine {
    outcomes: Mutex<VecDeque<ScriptedOutcome>>,
    calls: Mutex<Vec<CapturedCall>>,
}

impl ScriptedDecisionEngine {
    fn new(decision: RoleDecision) -> Self {
        Self {
            outcomes: Mutex::new(VecDeque::from([ScriptedOutcome::Decision(decision)])),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<CapturedCall> {
        self.calls
            .lock()
            .expect("call mutex is not poisoned")
            .iter()
            .map(|call| CapturedCall {
                system_prompt: call.system_prompt.clone(),
                user_context: call.user_context.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl RoleDecisionEngine for ScriptedDecisionEngine {
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError> {
        self.calls
            .lock()
            .expect("call mutex is not poisoned")
            .push(CapturedCall {
                system_prompt: system_prompt.to_string(),
                user_context: user_context.to_string(),
            });
        match self
            .outcomes
            .lock()
            .expect("outcome mutex is not poisoned")
            .pop_front()
            .expect("scripted outcome exists")
        {
            ScriptedOutcome::Decision(decision) => Ok(decision),
        }
    }
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

fn workflow_with_external_tool(required: bool) -> ValidatedWorkflow {
    let required = if required { "true" } else { "false" };
    parse_workflow(&format!(
        r#"{{
            "name": "generic-agent-test",
            "roles": [{{
                "id": "banana",
                "prompt": {{"guidance": "Prefer generic manifest actions."}},
                "external_tools": [{{
                    "id": "coding_workspace",
                    "description": "Edit and commit repository code.",
                    "required": {required},
                    "constraints": ["Only touch the checked-out repository."],
                    "guidance": "Use before opening implementation PRs."
                }}],
                "queues": ["todo"]
            }}],
            "labels": [{{"id": "task"}}, {{"id": "todo"}}, {{"id": "done"}}],
            "artifact_kinds": [{{
                "id": "task",
                "target": "issue",
                "identifying_labels": ["task"]
            }}],
            "queues": [{{"id": "todo", "artifact": "task", "labels": ["todo"]}}],
            "transitions": [{{
                "id": "advance",
                "artifact": "task",
                "roles": ["banana"],
                "effects": [
                    {{"kind": "remove_label", "label": "todo"}},
                    {{"kind": "add_label", "label": "done"}}
                ]
            }}]
        }}"#,
    ))
}

fn workflow_with_coding_workspace_pr_create() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {
                    "guidance": "Use open_pr when the coding workspace is available.",
                    "tool_guidance": "Implementation work must go through coding_workspace before opening a PR."
                },
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

fn agent_with_external_tools(
    manifest: RoleManifest,
    engine: Arc<ScriptedDecisionEngine>,
    tools: Vec<BoundExternalTool>,
) -> LlmRoleAgent {
    LlmRoleAgent::with_decision_engine_and_external_tools(
        manifest,
        engine as Arc<dyn RoleDecisionEngine>,
        tools,
    )
}

fn agent_with_external_tools_and_executors(
    manifest: RoleManifest,
    engine: Arc<ScriptedDecisionEngine>,
    tools: Vec<BoundExternalTool>,
    executors: ExternalToolExecutors,
) -> LlmRoleAgent {
    LlmRoleAgent::with_decision_engine_external_tools_and_executors(
        manifest,
        engine as Arc<dyn RoleDecisionEngine>,
        tools,
        executors,
    )
}

fn bound_coding_workspace() -> BoundExternalTool {
    BoundExternalTool {
        id: ExternalToolId::new("coding_workspace"),
        description: "Edit and commit repository code.".to_string(),
        required: true,
        constraints: vec!["Only touch the checked-out repository.".to_string()],
        guidance: Some("Use before opening implementation PRs.".to_string()),
        provider: "workspace-local".to_string(),
    }
}

fn undeclared_shell() -> BoundExternalTool {
    BoundExternalTool {
        id: ExternalToolId::new("shell"),
        description: "Run arbitrary shell commands.".to_string(),
        required: false,
        constraints: Vec::new(),
        guidance: None,
        provider: "shell-provider".to_string(),
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

#[tokio::test]
async fn optional_unbound_external_tool_is_not_available_in_runtime_prompt_or_context() {
    let fixture =
        fixture_from_workflow(&["task", "todo"], workflow_with_external_tool(false)).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(RoleDecision::no_action(
        "inspect external tools",
    )));
    let agent = LlmRoleAgent::with_decision_engine(
        fixture.manifest.clone(),
        engine.clone() as Arc<dyn RoleDecisionEngine>,
    );

    agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    let calls = engine.calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].system_prompt.contains("coding_workspace via"));
    assert!(
        calls[0]
            .system_prompt
            .contains("no external tools are bound")
    );
    let user_context: serde_json::Value =
        serde_json::from_str(&calls[0].user_context).expect("user context is json");
    assert_eq!(
        user_context["available_external_tools"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn bound_declared_external_tool_appears_in_prompt_and_context_not_actions() {
    let fixture = fixture_from_workflow(&["task", "todo"], workflow_with_external_tool(true)).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(RoleDecision::no_action(
        "inspect bound external tools",
    )));
    let agent = agent_with_external_tools(
        fixture.manifest.clone(),
        Arc::clone(&engine),
        vec![bound_coding_workspace()],
    );

    agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    let calls = engine.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .system_prompt
            .contains("coding_workspace via workspace-local")
    );
    let user_context: serde_json::Value =
        serde_json::from_str(&calls[0].user_context).expect("user context is json");
    assert_eq!(
        user_context["available_external_tools"][0]["id"],
        "coding_workspace"
    );
    assert_eq!(
        user_context["allowed_actions"],
        serde_json::json!(["no_action", "advance"])
    );
}

#[tokio::test]
async fn undeclared_bound_external_tool_is_filtered_before_prompt_context() {
    let fixture =
        fixture_from_workflow(&["task", "todo"], workflow_with_external_tool(false)).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(RoleDecision::no_action(
        "inspect undeclared",
    )));
    let agent = agent_with_external_tools(
        fixture.manifest.clone(),
        Arc::clone(&engine),
        vec![undeclared_shell()],
    );

    agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(agent.bound_external_tools().is_empty());
    let calls = engine.calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].system_prompt.contains("shell-provider"));
}

#[tokio::test]
async fn create_pr_action_uses_executable_coding_workspace_head() {
    let fixture = fixture_from_workflow(
        &["task", "todo"],
        workflow_with_coding_workspace_pr_create(),
    )
    .await;
    let engine = Arc::new(ScriptedDecisionEngine::new(RoleDecision::action(
        "open_pr",
        "workspace produced implementation",
    )));
    let workspace = Arc::new(FixtureWorkspace::default());
    let workspace_provider: Arc<dyn CodingWorkspace> = workspace.clone();
    let executors = ExternalToolExecutors::new().with_coding_workspace(
        RoleId::new("banana"),
        ExternalToolId::new("coding_workspace"),
        workspace_provider,
    );
    let agent = agent_with_external_tools_and_executors(
        fixture.manifest.clone(),
        Arc::clone(&engine),
        vec![bound_coding_workspace()],
        executors,
    );

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("workspace-backed PR create succeeds");

    assert!(changed);
    assert_eq!(labels(&fixture).await, vec!["in-progress", "task"]);
    let requests = workspace.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].branch_hint, "agent/pr-for-task-1");
    assert!(
        requests[0]
            .guidance
            .role_guidance
            .as_deref()
            .is_some_and(|guidance| guidance.contains("Use open_pr"))
    );

    let pull_requests = fixture
        .forge
        .list_pull_requests(&fixture.repo, PullRequestQuery::default())
        .await
        .expect("PR list succeeds");
    assert_eq!(pull_requests.len(), 1);
    assert_eq!(pull_requests[0].source.branch, "agent/pr-for-task-1");
    assert_eq!(
        pull_requests[0].labels,
        vec!["implementation", "needs-merge", "needs-reviewer"]
    );
    assert!(
        pull_requests[0]
            .body
            .contains("updated docs/product-change.md")
    );
    assert!(
        pull_requests[0]
            .body
            .contains("\"correlation_key\": \"pr-for-task-1\"")
    );
}
