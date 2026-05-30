//! Deterministic fake role behavior for reference-delivery runner tests.
//!
//! These fakes are behavior-only test adapters. They mutate workflow state only
//! through `RoleTools`, exactly like a real agent adapter would, and keep all
//! orchestration in runner workers/stages.

use async_trait::async_trait;
use harness_forge::{Forge, ItemNumber};
use harness_runner::{Agent, AgentError, AgentRegistry, RoleTools, WorkItem};
use harness_workflow::{
    parse_metadata_block, render_metadata_block, ArtifactKindId, ArtifactSource, ExecutionError,
    RoleId, TransitionId, WorkflowMetadata,
};
use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::pull_request_input;

#[derive(Clone, Debug, Default)]
pub struct FakeArchitect;

#[derive(Clone, Debug, Default)]
pub struct FakeEngineer;

#[derive(Clone, Debug, Default)]
pub struct ClosingArchitect;

#[derive(Clone, Debug, Default)]
pub struct FakeReviewer;

#[derive(Debug, Default)]
pub struct RequestChangesThenApproveReviewer {
    visits: Mutex<BTreeMap<ItemNumber, u64>>,
}

impl RequestChangesThenApproveReviewer {
    pub fn new() -> Self {
        Self::default()
    }

    fn next_transition(&self, number: ItemNumber) -> &'static str {
        let mut visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        let visit = visits.entry(number).or_insert(0);
        *visit = visit.saturating_add(1);
        if *visit == 1 {
            "request_changes"
        } else {
            "approve_review"
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeOwner;

#[derive(Clone, Debug, Default)]
pub struct FakeHuman;

pub fn fake_registry<F>() -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    fake_registry_with(FakeArchitect, FakeReviewer)
}

pub fn fake_registry_with<F, A, R>(architect: A, reviewer: R) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
    A: Agent<F> + 'static,
    R: Agent<F> + 'static,
{
    let mut registry = AgentRegistry::new();
    registry.register(RoleId::new("architect"), architect);
    registry.register(RoleId::new("engineer"), FakeEngineer);
    registry.register(RoleId::new("reviewer"), reviewer);
    registry.register(RoleId::new("owner"), FakeOwner);
    registry.register(RoleId::new("human"), FakeHuman);
    registry
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        service_architect(item, tools, false).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for ClosingArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        service_architect(item, tools, true).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeEngineer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() == "code_ready" && item.kind.as_str() == "code" {
            return service_ready_code(item, tools).await;
        }
        if item.queue.as_str() == "pr_changes_requested" {
            return run_or_ignore_stale(tools, item.target, "address_review_changes").await;
        }
        if item.queue.as_str() == "pr_ci_failed" {
            return run_or_ignore_stale(tools, item.target, "address_ci_failure").await;
        }
        Ok(false)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeReviewer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() == "pr_needs_review" && item.kind.as_str() == "implementation_pr" {
            return run_or_ignore_stale(tools, item.target, "approve_review").await;
        }
        Ok(false)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for RequestChangesThenApproveReviewer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() != "pr_needs_review" || item.kind.as_str() != "implementation_pr" {
            return Ok(false);
        }
        let ArtifactSource::PullRequest { number } = item.target else {
            return Ok(false);
        };
        run_or_ignore_stale(tools, item.target, self.next_transition(number)).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeOwner {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.kind.as_str() == "implementation_pr" && item.queue.as_str() == "owner_alignment" {
            return run_or_ignore_stale(tools, item.target, "review_alignment").await;
        }
        if item.kind.as_str() == "implementation_pr" {
            return run_or_ignore_stale(tools, item.target, "approve_merge").await;
        }
        if item.queue.as_str() == "needs_owner" && item.kind.as_str() == "design" {
            return run_or_ignore_stale(tools, item.target, "request_human_input").await;
        }
        Ok(false)
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeHuman {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        if item.queue.as_str() == "needs_human" && item.kind.as_str() == "design" {
            return run_or_ignore_stale(tools, item.target, "clear_human_flag").await;
        }
        Ok(false)
    }
}

async fn service_architect<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    close_parent_issues: bool,
) -> Result<bool, AgentError> {
    if item.queue.as_str() == "design_triage" && item.kind.as_str() == "intake" {
        return run_or_ignore_stale(tools, item.target, "triage_to_code").await;
    }
    if item.queue.as_str() == "landed_inbox" && item.kind.as_str() == "implementation_pr" {
        let reconciled = run_or_ignore_stale(tools, item.target, "reconcile_landed").await?;
        if reconciled && close_parent_issues {
            close_produced_parent_issues(item, tools).await?;
        }
        return Ok(reconciled);
    }
    Ok(false)
}

async fn close_produced_parent_issues<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<bool, AgentError> {
    let ArtifactSource::PullRequest { number } = item.target else {
        return Ok(false);
    };
    let Some(pull_request) = tools.get_pull_request(number).await? else {
        return Ok(false);
    };
    let Some(metadata) = parse_metadata_block(&pull_request.body)
        .map_err(|error| AgentError::message(format!("invalid PR workflow metadata: {error}")))?
    else {
        return Ok(false);
    };

    let mut closed = false;
    for parent in metadata.parents {
        closed |= tools.close_issue(parent).await?;
    }
    Ok(closed)
}

async fn service_ready_code<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<bool, AgentError> {
    let ArtifactSource::Issue { number } = item.target else {
        return Ok(false);
    };
    let Some(issue) = tools.get_issue(number).await? else {
        return Ok(false);
    };
    if !issue.labels.iter().any(|label| label == "ready") {
        return Ok(false);
    }

    let claimed = run_or_ignore_stale(tools, item.target, "claim_code").await?;
    if !claimed {
        return Ok(false);
    }

    let correlation_key = format!("pr-for-code-{}", number.get());
    let pull_request = tools
        .open_pull_request(
            &correlation_key,
            implementation_pr_input(tools, number, &issue.title),
        )
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

fn implementation_pr_input<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    code_number: ItemNumber,
    issue_title: &str,
) -> harness_forge::CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![code_number],
        ..WorkflowMetadata::default()
    };
    let body = format!(
        "Fake implementation for code issue #{code_number}.\n\n{}",
        render_metadata_block(&metadata)
    );
    pull_request_input(
        tools.repo(),
        format!("Implement #{code_number}: {issue_title}"),
        body,
        format!("fake/pr-for-code-{}", code_number.get()),
        vec!["implementation".to_string()],
    )
}

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
