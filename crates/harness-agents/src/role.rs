//! Manifest-driven LLM workflow role agent.
//!
//! [`LlmRoleAgent`] is the generic replacement path for hard-coded role prompt
//! constants and role-specific decision enums: it owns a compiled
//! [`RoleManifest`], asks a decision engine for one `{ action, reason }` JSON
//! decision using the manifest's rendered prompt, validates that action against
//! the manifest's tool list, and runs only the matching workflow transition
//! through [`RoleTools`].

use std::sync::Arc;

use async_trait::async_trait;
use harness_forge::Forge;
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use harness_workflow::{RoleManifest, TransitionId};
use serde::Deserialize;

use crate::common::{build_context, run_or_ignore_stale};
use crate::decision::{DecisionError, run_decision};
use crate::provider::ProviderConfig;

const NO_ACTION: &str = "no_action";

/// Generic workflow-role decision returned by a model or injected test seam.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct RoleDecision {
    /// One manifest tool name, or [`no_action`](NO_ACTION).
    pub action: String,
    /// Short rationale for logs and operator debugging. The generic adapter does
    /// not use this to grant authority.
    #[serde(default)]
    pub reason: String,
}

impl RoleDecision {
    /// Builds a decision that deliberately makes no workflow mutation.
    pub fn no_action(reason: impl Into<String>) -> Self {
        Self {
            action: NO_ACTION.to_string(),
            reason: reason.into(),
        }
    }

    /// Builds a decision for an action name.
    pub fn action(action: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            reason: reason.into(),
        }
    }
}

/// Mockable seam for obtaining one generic role decision.
#[async_trait]
pub trait RoleDecisionEngine: Send + Sync {
    /// Decide from the system prompt and user context the adapter constructed.
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError>;
}

/// Provider-backed decision engine that runs the real `pi` SDK path.
pub struct ProviderRoleDecisionEngine {
    provider: ProviderConfig,
}

impl ProviderRoleDecisionEngine {
    /// Builds a decision engine backed by the configured LLM provider.
    pub fn new(provider: ProviderConfig) -> Self {
        Self { provider }
    }
}

#[async_trait]
impl RoleDecisionEngine for ProviderRoleDecisionEngine {
    async fn decide(
        &self,
        system_prompt: &str,
        user_context: &str,
    ) -> Result<RoleDecision, DecisionError> {
        run_decision::<RoleDecision>(&self.provider, system_prompt, user_context).await
    }
}

/// Generic LLM agent for one compiled workflow role.
pub struct LlmRoleAgent {
    manifest: RoleManifest,
    decision_engine: Arc<dyn RoleDecisionEngine>,
}

impl LlmRoleAgent {
    /// Builds a provider-backed agent for `manifest`.
    pub fn new(manifest: RoleManifest, provider: ProviderConfig) -> Self {
        Self::with_decision_engine(
            manifest,
            Arc::new(ProviderRoleDecisionEngine::new(provider)) as Arc<dyn RoleDecisionEngine>,
        )
    }

    /// Builds an agent with an injected decision engine, for hermetic tests or
    /// alternate providers.
    pub fn with_decision_engine(
        manifest: RoleManifest,
        decision_engine: Arc<dyn RoleDecisionEngine>,
    ) -> Self {
        Self {
            manifest,
            decision_engine,
        }
    }

    /// Returns the compiled role manifest this agent enforces.
    pub fn manifest(&self) -> &RoleManifest {
        &self.manifest
    }

    async fn decide(&self, item: &WorkItem, context: &str) -> Result<RoleDecision, AgentError> {
        let system_prompt = self.manifest.prompt.render();
        match self.decision_engine.decide(&system_prompt, context).await {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => Err(AgentError::message(error.to_string())),
            Err(error) => {
                eprintln!(
                    "harness-agents: LLM decision failed for role '{}' on {:?} queue '{}', treating as no-action: {error}",
                    self.manifest.id,
                    item.target,
                    item.queue.as_str()
                );
                Ok(RoleDecision::no_action("decision failed"))
            }
        }
    }

    fn transition_for_action(&self, action: &str) -> Option<&TransitionId> {
        self.manifest
            .tools
            .iter()
            .find(|tool| tool.name == action)
            .map(|tool| &tool.transition)
    }

    fn user_context(&self, work_item_context: &str) -> String {
        let work_item = serde_json::from_str::<serde_json::Value>(work_item_context)
            .unwrap_or_else(|_| serde_json::Value::String(work_item_context.to_string()));
        let allowed_actions = std::iter::once(NO_ACTION.to_string())
            .chain(self.manifest.tools.iter().map(|tool| tool.name.clone()))
            .collect::<Vec<_>>();
        let authorized_actions = self
            .manifest
            .tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "action": tool.name,
                    "transition": tool.transition.as_str(),
                    "artifact": tool.artifact.as_str(),
                    "requires_gates": tool
                        .requires_gates
                        .iter()
                        .map(|gate| gate.as_str())
                        .collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let context = serde_json::json!({
            "work_item": work_item,
            "allowed_actions": allowed_actions,
            "authorized_actions": authorized_actions,
        });
        serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string())
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmRoleAgent {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let work_item_context = build_context(item, tools).await?;
        let context = self.user_context(&work_item_context);
        let decision = self.decide(item, &context).await?;

        if decision.action == NO_ACTION {
            return Ok(false);
        }

        let Some(transition) = self.transition_for_action(&decision.action) else {
            eprintln!(
                "harness-agents: role '{}' returned unauthorized action '{}', treating as no-action",
                self.manifest.id, decision.action
            );
            return Ok(false);
        };

        run_or_ignore_stale(tools, item.target, transition.as_str()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::sync::Mutex;

    use harness_forge::{CreateIssue, CreateRepository, Forge, Issue, ItemNumber, RepositoryId};
    use harness_forge_memory::MemoryForge;
    use harness_runner::WorkItem;
    use harness_workflow::{
        ArtifactKindId, ArtifactSource, ExecutionContext, QueueId, RawWorkflowSpec, RoleId,
        ValidatedWorkflow,
    };

    use crate::prompts::{
        ARCHITECT_SYSTEM_PROMPT, ENGINEER_SYSTEM_PROMPT, HUMAN_SYSTEM_PROMPT, OWNER_SYSTEM_PROMPT,
        REVIEWER_SYSTEM_PROMPT,
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
        let workflow = workflow();
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
        let json = r#"{
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
        }"#;
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
        for checked_in_prompt in [
            ARCHITECT_SYSTEM_PROMPT,
            ENGINEER_SYSTEM_PROMPT,
            REVIEWER_SYSTEM_PROMPT,
            OWNER_SYSTEM_PROMPT,
            HUMAN_SYSTEM_PROMPT,
        ] {
            assert_ne!(calls[0].system_prompt, checked_in_prompt);
        }
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
}
