use super::*;

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use temper_forge::{CreateIssue, CreateRepository, Forge, Issue, ItemNumber, RepositoryId};
use temper_forge_memory::MemoryForge;
use temper_runner::{RoleTools, WorkItem};
use temper_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionContext, QueueId, RawWorkflowSpec, RoleId,
    ValidatedWorkflow,
};

use crate::provider::ProviderError;

#[derive(Debug)]
enum ScriptedOutcome {
    Decision(RoleDecision),
    Error(DecisionError),
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
    fn new(outcome: ScriptedOutcome) -> Self {
        Self {
            outcomes: Mutex::new(VecDeque::from([outcome])),
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
            ScriptedOutcome::Error(error) => Err(error),
        }
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

async fn fixture(labels: &[&str]) -> Fixture {
    fixture_from_workflow(labels, workflow()).await
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

fn workflow() -> ValidatedWorkflow {
    parse_workflow(
        r#"{
            "name": "generic-agent-test",
            "roles": [{
                "id": "banana",
                "prompt": {"guidance": "Prefer generic manifest actions."},
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

fn parse_workflow(json: &str) -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(json).expect("workflow json parses");
    spec.validate().expect("workflow validates")
}

fn agent_with(manifest: RoleManifest, engine: Arc<ScriptedDecisionEngine>) -> LlmRoleAgent {
    LlmRoleAgent::with_decision_engine(manifest, engine as Arc<dyn RoleDecisionEngine>)
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
async fn authorized_transition_is_executed() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::action("advance", "ready to advance"),
    )));
    let agent = agent_with(fixture.manifest.clone(), Arc::clone(&engine));

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(changed);
    assert_eq!(labels(&fixture).await, vec!["done", "task"]);
}

#[tokio::test]
async fn no_action_makes_no_mutation() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::no_action("not enough context"),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn unknown_action_is_rejected_without_running_transition() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::action("delete_everything", "not authorized"),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn stale_precondition_errors_return_no_progress() {
    let fixture = fixture(&["task"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::action("advance", "stale but authorized"),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("stale precondition is ignored");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task"]);
}

#[tokio::test]
async fn classification_errors_return_no_progress() {
    let fixture = fixture(&["todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::action("advance", "unclassified but authorized"),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("classification error is ignored");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["todo"]);
}

#[tokio::test]
async fn target_missing_errors_return_no_progress() {
    let mut fixture = fixture(&["task", "todo"]).await;
    fixture.item.target = ArtifactSource::Issue {
        number: ItemNumber::new(999),
    };
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::action("advance", "missing but authorized"),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("missing target is ignored");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn decision_parse_failure_degrades_to_no_action() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Error(
        DecisionError::Parse {
            snippet: "not json".to_string(),
            error: "expected value".to_string(),
        },
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("parse failure is no-action");

    assert!(!changed);
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn provider_setup_failure_is_agent_error() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Error(
        DecisionError::Provider(ProviderError::Build("bad model".to_string())),
    )));
    let agent = agent_with(fixture.manifest.clone(), engine);

    let error = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect_err("provider setup failure is real error");

    assert!(matches!(error, AgentError::Message(message) if message.contains("bad model")));
    assert_eq!(labels(&fixture).await, vec!["task", "todo"]);
}

#[tokio::test]
async fn decision_engine_receives_compiled_manifest_prompt_and_authorized_actions_context() {
    let fixture = fixture(&["task", "todo"]).await;
    let engine = Arc::new(ScriptedDecisionEngine::new(ScriptedOutcome::Decision(
        RoleDecision::no_action("inspect prompt"),
    )));
    let agent = agent_with(fixture.manifest.clone(), Arc::clone(&engine));

    let changed = agent
        .service(&fixture.item, &tools(&fixture))
        .await
        .expect("service succeeds");

    assert!(!changed);
    let calls = engine.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].system_prompt, fixture.manifest.prompt.render());
    assert!(calls[0].system_prompt.contains("Role: banana"));
    assert!(
        calls[0]
            .system_prompt
            .contains("Workflow: generic-agent-test")
    );
    assert!(
        calls[0]
            .system_prompt
            .contains("Prefer generic manifest actions.")
    );
    let user_context: serde_json::Value =
        serde_json::from_str(&calls[0].user_context).expect("user context is json");
    assert_eq!(
        user_context["allowed_actions"],
        serde_json::json!(["no_action", "advance"])
    );
    assert_eq!(user_context["work_item"]["artifact"]["number"], 1);
    assert_eq!(
        user_context["authorized_actions"][0]["transition"],
        "advance"
    );
}
