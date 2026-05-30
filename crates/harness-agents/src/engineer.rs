//! The real, LLM-driven engineer role agent.
//!
//! [`LlmEngineer`] is a [`harness_runner::Agent`] whose **decision** comes from a
//! DeepSeek model (via the in-process `pi` SDK) but whose **mutations** go only
//! through [`RoleTools`] — the same authority boundary the deterministic fakes
//! use. A confused model therefore fails as a workflow validation error, never a
//! bypass: it cannot touch git, the filesystem, or the Forge API directly.
//!
//! ## Why a structured decision rather than SDK tools
//!
//! The Phase B contract offers two shapes: expose workflow actions to the model
//! as SDK [`Tool`](pi::sdk::Tool)s that call `RoleTools`, or have the model emit
//! a structured decision the adapter maps to a `RoleTools` call. We take the
//! **structured-decision** shape for B1 because `RoleTools<'a, F>` borrows the
//! Forge handle and executor for a single `service` call; an SDK `Tool` must be
//! `'static + Send + Sync` (it is stored in a `ToolRegistry` and called back by
//! the agent loop), so closing a tool over the borrowed `RoleTools` does not
//! typecheck without `unsafe` lifetime laundering. Emitting a decision and
//! invoking `RoleTools` from the adapter keeps the boundary explicit and the
//! lifetimes honest, and is the contract's "simplest to get working first"
//! option. B2 can revisit the tool shape if a role needs multi-step tool use.
//!
//! ## Backend side effects
//!
//! Opening a pull request on a real Forge needs a real head branch with a
//! non-empty diff (and, for CI, a passing head). That is backend-specific and
//! lives outside this LLM-agnostic crate, so [`LlmEngineer`] takes an injected
//! [`EngineerPrep`] hook, mirroring how the testing crate's `ForgejoEngineer`
//! wraps the shared fake state machine. On the in-memory/filesystem backends the
//! [`NoPrep`] default is a no-op.

use std::sync::Arc;

use async_trait::async_trait;
use harness_forge::{CreatePullRequest, Forge, ItemNumber};
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use harness_workflow::{
    ArtifactKindId, ArtifactSource, ExecutionError, TransitionId, WorkflowMetadata,
    render_metadata_block,
};
use serde::Deserialize;

use crate::decision::{DecisionError, run_decision};
use crate::prompts::ENGINEER_SYSTEM_PROMPT;
use crate::provider::ProviderConfig;

/// Backend side effects the engineer must perform before opening a pull request.
///
/// The filesystem/memory backends never check that a PR head is a real git ref,
/// so the [`NoPrep`] default suffices. A real Forge (Forgejo) needs the head
/// branch + a differing commit to exist first; the test wires an implementation
/// that calls the harness's `prepare_pull_request_head`/`commit_ci_sentinel`.
#[async_trait]
pub trait EngineerPrep<F: Forge + ?Sized>: Send + Sync {
    /// Called with the PR input immediately before `open_pull_request`.
    async fn before_open_pr(
        &self,
        _tools: &RoleTools<'_, F>,
        _input: &CreatePullRequest,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called on a `pr_ci_failed` item just before `address_ci_failure`, so a
    /// backend that needs a new head SHA can push one.
    async fn before_address_ci_failure(
        &self,
        _tools: &RoleTools<'_, F>,
        _target: ArtifactSource,
    ) -> Result<(), AgentError> {
        Ok(())
    }
}

/// The no-op [`EngineerPrep`] for backends that need no git side effects.
pub struct NoPrep;

impl<F: Forge + ?Sized> EngineerPrep<F> for NoPrep {}

/// The action the LLM chose for a work item.
///
/// Deserialized from the model's single JSON object. The closed set mirrors the
/// engineer's authorized transitions in the reference-delivery workflow; an
/// unparseable or out-of-set response is treated as [`Self::NoAction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum EngineerDecision {
    /// Claim a ready code issue and open its implementation PR.
    ClaimAndOpenPr,
    /// Re-request review after a reviewer asked for changes.
    AddressReviewChanges,
    /// Push a fix and re-request review after CI failed.
    AddressCiFailure,
    /// Do nothing (stale, already handled, or not applicable).
    NoAction,
}

/// A real engineer agent: decide with the LLM, act through [`RoleTools`].
pub struct LlmEngineer<F: Forge + ?Sized> {
    provider: ProviderConfig,
    prep: Arc<dyn EngineerPrep<F>>,
}

impl<F: Forge + ?Sized> LlmEngineer<F> {
    /// Builds an engineer with no backend prep (memory/filesystem backends).
    pub fn new(provider: ProviderConfig) -> Self {
        Self {
            provider,
            prep: Arc::new(NoPrep),
        }
    }

    /// Builds an engineer with a backend-specific [`EngineerPrep`] hook.
    pub fn with_prep(provider: ProviderConfig, prep: Arc<dyn EngineerPrep<F>>) -> Self {
        Self { provider, prep }
    }

    /// Asks the model for a decision on `item`, defaulting to [`NoAction`] on any
    /// model/parse error so a flaky LLM never blocks the worker (it just makes no
    /// progress this tick and is retried).
    ///
    /// [`NoAction`]: EngineerDecision::NoAction
    async fn decide(&self, item: &WorkItem, context: &str) -> Result<EngineerDecision, AgentError> {
        match run_decision::<EngineerDecision>(&self.provider, ENGINEER_SYSTEM_PROMPT, context)
            .await
        {
            Ok(decision) => Ok(decision),
            Err(DecisionError::Provider(error)) => {
                // A missing key / bad provider config is a real setup error.
                Err(AgentError::message(error.to_string()))
            }
            Err(error) => {
                // Transient model or parse failure: make no progress, retry next
                // tick. The message is safe (no secrets) and aids debugging.
                eprintln!(
                    "harness-agents: engineer LLM decision failed for {:?} on queue '{}', \
                     treating as no-action: {error}",
                    item.target,
                    item.queue.as_str()
                );
                Ok(EngineerDecision::NoAction)
            }
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for LlmEngineer<F> {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        let context = build_context(item, tools).await?;
        let decision = self.decide(item, &context).await?;

        match decision {
            EngineerDecision::ClaimAndOpenPr => self.claim_and_open_pr(item, tools).await,
            EngineerDecision::AddressReviewChanges => {
                run_or_ignore_stale(tools, item.target, "address_review_changes").await
            }
            EngineerDecision::AddressCiFailure => {
                self.prep
                    .before_address_ci_failure(tools, item.target)
                    .await?;
                run_or_ignore_stale(tools, item.target, "address_ci_failure").await
            }
            EngineerDecision::NoAction => Ok(false),
        }
    }
}

impl<F: Forge + ?Sized> LlmEngineer<F> {
    /// Executes the claim → open-PR → request-review sequence through
    /// [`RoleTools`], running the backend prep hook before the PR is opened.
    ///
    /// This is the engineer's load-bearing action and mirrors the fake
    /// engineer's `service_ready_code` exactly, so the same workflow policy and
    /// idempotent creation seam apply.
    async fn claim_and_open_pr(
        &self,
        item: &WorkItem,
        tools: &RoleTools<'_, F>,
    ) -> Result<bool, AgentError> {
        let ArtifactSource::Issue { number } = item.target else {
            return Ok(false);
        };
        let Some(issue) = tools.get_issue(number).await? else {
            return Ok(false);
        };
        // The workflow still enforces this; we check it so a model that picks
        // `claim_and_open_pr` for a not-yet-ready issue makes no progress instead
        // of erroring.
        if !issue.labels.iter().any(|label| label == "ready") {
            return Ok(false);
        }

        let claimed = run_or_ignore_stale(tools, item.target, "claim_code").await?;
        if !claimed {
            return Ok(false);
        }

        let correlation_key = format!("pr-for-code-{}", number.get());
        let input = implementation_pr_input(tools.repo(), number, &issue.title);
        self.prep.before_open_pr(tools, &input).await?;
        let pull_request = tools
            .open_pull_request(&correlation_key, input)
            .await?
            .into_artifact();
        let requested = run_or_ignore_stale(
            tools,
            ArtifactSource::PullRequest {
                number: pull_request.number,
            },
            "request_review",
        )
        .await?;
        Ok(claimed || requested)
    }
}

/// Serializes the work item plus the issue/PR it points at into the JSON the
/// model reasons over. Reads go through [`RoleTools`]; a read failure aborts the
/// tick (it is not the model's fault).
async fn build_context<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<String, AgentError> {
    let artifact = match item.target {
        ArtifactSource::Issue { number } => tools.get_issue(number).await?.map(|issue| {
            serde_json::json!({
                "type": "issue",
                "number": number.get(),
                "title": issue.title,
                "body": issue.body,
                "labels": issue.labels,
                "state": format!("{:?}", issue.state),
            })
        }),
        ArtifactSource::PullRequest { number } => tools.get_pull_request(number).await?.map(|pr| {
            serde_json::json!({
                "type": "pull_request",
                "number": number.get(),
                "title": pr.title,
                "body": pr.body,
                "labels": pr.labels,
                "state": format!("{:?}", pr.state),
            })
        }),
    };

    let context = serde_json::json!({
        "queue": item.queue.as_str(),
        "kind": item.kind.as_str(),
        "artifact": artifact,
    });
    Ok(serde_json::to_string_pretty(&context).unwrap_or_else(|_| context.to_string()))
}

/// Builds the implementation-PR input (title, metadata-tagged body, head branch).
///
/// The head branch follows the same `<prefix>/pr-for-code-N` convention the fake
/// uses, so the backend prep hook can derive it deterministically. The metadata
/// block records the parent code issue so reconciliation can find it.
fn implementation_pr_input(
    repo: &harness_forge::RepositoryId,
    code_number: ItemNumber,
    issue_title: &str,
) -> CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![code_number],
        ..WorkflowMetadata::default()
    };
    let body = format!(
        "LLM-authored implementation for code issue #{code_number}.\n\n{}",
        render_metadata_block(&metadata)
    );
    CreatePullRequest {
        title: format!("Implement #{code_number}: {issue_title}"),
        body,
        source: harness_forge::BranchRef {
            repository_id: repo.clone(),
            branch: format!("agent/pr-for-code-{}", code_number.get()),
        },
        target: harness_forge::BranchRef {
            repository_id: repo.clone(),
            branch: "main".to_string(),
        },
        labels: vec!["implementation".to_string()],
        assignees: Vec::new(),
    }
}

/// Runs a transition, treating stale/precondition/classification failures as "no
/// progress" (return `false`) exactly as the fakes do, so a model acting on a
/// stale item degrades gracefully.
async fn run_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
) -> Result<bool, AgentError> {
    match tools.run(target, &TransitionId::new(transition)).await {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn stale_execution(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::Precondition { .. }
            | ExecutionError::TargetMissing { .. }
            | ExecutionError::Classification(_)
    )
}
