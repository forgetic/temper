//! Deterministic fake role behavior for reference-delivery runner tests.
//!
//! These fakes are behavior-only test adapters. They mutate workflow state only
//! through `RoleTools`, exactly like a real agent adapter would, and keep all
//! orchestration in runner workers/stages.

use async_trait::async_trait;
use harness_forge::{CreateIssue, Forge, ItemNumber, RepositoryId};
use harness_runner::{Agent, AgentError, AgentRegistry, RoleTools, WorkItem};
use harness_workflow::{
    global_child_correlation_key, parse_metadata_block, render_metadata_block, ArtifactKindId,
    ArtifactRef, ArtifactSource, ExecutionError, RoleId, TransitionId, WorkflowMetadata,
};
use serde::Deserialize;
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

    /// The transition this reviewer should attempt next for `number`, based on
    /// how many of its reviews have **landed** so far: `request_changes` first,
    /// then `approve_review`.
    fn pending_transition(&self, number: ItemNumber) -> &'static str {
        let visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        if visits.get(&number).copied().unwrap_or(0) == 0 {
            "request_changes"
        } else {
            "approve_review"
        }
    }

    /// Records that a review **succeeded** for `number`, advancing the counter.
    ///
    /// Crucially this is called only after the transition actually lands, not on
    /// every visit: a stale/skipped first attempt must not "consume" the
    /// request-changes step and let the next visit jump straight to approval. On
    /// a real backend the first scan can race ahead of the PR being review-ready,
    /// so advancing only on success keeps "request changes, then approve" intact.
    fn record_success(&self, number: ItemNumber) {
        let mut visits = self
            .visits
            .lock()
            .expect("reviewer visit mutex is poisoned");
        let visit = visits.entry(number).or_insert(0);
        *visit = visit.saturating_add(1);
    }
}

#[derive(Clone, Debug, Default)]
pub struct FakeOwner;

#[derive(Clone, Debug, Default)]
pub struct FakeHuman;

/// Marker used by deterministic tests to tell the fake architect which child
/// issues to fan out before applying the normal triage transition.
pub const ARCHITECT_PLAN_BEGIN: &str = "<!-- harness:architect-plan";
const ARCHITECT_PLAN_END: &str = "-->";

#[derive(Clone, Debug, Default, Deserialize)]
struct ArchitectPlan {
    #[serde(default)]
    children: Vec<ArchitectPlanChild>,
}

#[derive(Clone, Debug, Deserialize)]
struct ArchitectPlanChild {
    slug: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default, alias = "target_repository")]
    target_repo: Option<RepositoryId>,
}

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
        engineer_service(item, tools, &NoPrep).await
    }
}

/// Side-effect hook the engineer state machine calls at backend-specific moments.
///
/// On the filesystem/memory backends this is a no-op ([`NoPrep`]): the fake
/// engineer opens PRs and addresses CI failures with pure metadata, because those
/// backends never check that a PR head is a real git ref. On **real Forgejo** a
/// PR head must exist as a branch before `create_pull_request`, and a CI fail→pass
/// needs a new head SHA — so the Forgejo worker supplies a hook that creates the
/// branch + a differing commit before opening the PR and pushes a `ci-ok` fix
/// commit before the `address_ci_failure` transition. The engineer state machine
/// itself ([`engineer_service`]) is shared verbatim across both topologies; only
/// this hook differs, exactly as `--architect`/`--reviewer` vary the other roles.
#[async_trait]
pub(crate) trait EnginePrep<F: Forge + ?Sized>: Send + Sync {
    /// Called with the PR input immediately before `open_pull_request`.
    async fn before_open_pr(
        &self,
        _tools: &RoleTools<'_, F>,
        _input: &harness_forge::CreatePullRequest,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called on a `pr_ci_failed` item just before the `address_ci_failure`
    /// transition runs, so a backend that needs a new head SHA can push one.
    async fn before_address_ci_failure(
        &self,
        _tools: &RoleTools<'_, F>,
        _target: ArtifactSource,
    ) -> Result<(), AgentError> {
        Ok(())
    }
}

/// The no-op [`EnginePrep`] used by the backend-neutral [`FakeEngineer`].
pub(crate) struct NoPrep;

impl<F: Forge + ?Sized> EnginePrep<F> for NoPrep {}

/// The engineer role state machine, shared by [`FakeEngineer`] and the Forgejo
/// engineer wrapper. `prep` injects backend-specific side effects at the two
/// moments a real provider needs them; the no-op [`NoPrep`] keeps the filesystem
/// behavior byte-identical.
pub(crate) async fn engineer_service<F, P>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    prep: &P,
) -> Result<bool, AgentError>
where
    F: Forge + ?Sized,
    P: EnginePrep<F>,
{
    if item.queue.as_str() == "code_ready" && item.kind.as_str() == "code" {
        return service_ready_code(item, tools, prep).await;
    }
    if item.queue.as_str() == "pr_changes_requested" {
        return run_or_ignore_stale(tools, item.target, "address_review_changes").await;
    }
    if item.queue.as_str() == "pr_ci_failed" {
        prep.before_address_ci_failure(tools, item.target).await?;
        return run_or_ignore_stale(tools, item.target, "address_ci_failure").await;
    }
    Ok(false)
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
        let transition = self.pending_transition(number);
        let changed = run_or_ignore_stale(tools, item.target, transition).await?;
        // Advance only when the review actually landed, so a stale first attempt
        // does not skip the request-changes step.
        if changed && transition == "request_changes" {
            self.record_success(number);
        }
        Ok(changed)
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
        let fanout = fan_out_architect_children(item, tools).await?;
        let transition = if fanout.has_children {
            "triage_to_blocked_code"
        } else {
            "triage_to_code"
        };
        let triaged = run_or_ignore_stale(tools, item.target, transition).await?;
        return Ok(fanout.changed || triaged);
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

struct FanOutResult {
    changed: bool,
    has_children: bool,
}

async fn fan_out_architect_children<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<FanOutResult, AgentError> {
    let ArtifactSource::Issue { number: parent } = item.target else {
        return Ok(FanOutResult {
            changed: false,
            has_children: false,
        });
    };
    let Some(issue) = tools.get_issue(parent).await? else {
        return Ok(FanOutResult {
            changed: false,
            has_children: false,
        });
    };
    let plan = parse_architect_plan(&issue.body)?;
    let has_children = !plan.children.is_empty();
    let mut changed = false;
    for child in plan.children {
        validate_architect_child(&child)?;
        let target_repo = child
            .target_repo
            .clone()
            .unwrap_or_else(|| tools.repo().clone());
        let key = global_child_correlation_key(tools.repo(), parent, &child.slug);
        let outcome = tools
            .ensure_issue_in_repo(
                &target_repo,
                &key,
                ArtifactRef::same_repo(parent),
                architect_child_input(&child),
            )
            .await?;
        let child_number = outcome.artifact().number;
        changed |= outcome.was_created();
        changed |= tools
            .add_issue_dependency_metadata(
                parent,
                ArtifactRef::in_repo(target_repo.clone(), child_number),
            )
            .await?;
    }
    Ok(FanOutResult {
        changed,
        has_children,
    })
}

fn parse_architect_plan(body: &str) -> Result<ArchitectPlan, AgentError> {
    let Some(start) = body.find(ARCHITECT_PLAN_BEGIN) else {
        return Ok(ArchitectPlan::default());
    };
    let after = &body[start + ARCHITECT_PLAN_BEGIN.len()..];
    let Some(end) = after.find(ARCHITECT_PLAN_END) else {
        return Err(AgentError::message(
            "architect plan block was not terminated with `-->`",
        ));
    };
    serde_json::from_str(after[..end].trim()).map_err(|error| {
        AgentError::message(format!(
            "architect plan block contained invalid JSON: {error}"
        ))
    })
}

fn validate_architect_child(child: &ArchitectPlanChild) -> Result<(), AgentError> {
    if child.slug.trim().is_empty() {
        return Err(AgentError::message(
            "architect child slug must not be empty",
        ));
    }
    if child.title.trim().is_empty() {
        return Err(AgentError::message(format!(
            "architect child `{}` title must not be empty",
            child.slug
        )));
    }
    Ok(())
}

fn architect_child_input(child: &ArchitectPlanChild) -> CreateIssue {
    CreateIssue {
        title: child.title.clone(),
        body: child_body(&child.body),
        labels: vec!["code".to_string(), "ready".to_string()],
        assignees: Vec::new(),
    }
}

fn child_body(body: &str) -> String {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        ..WorkflowMetadata::default()
    };
    if body.trim().is_empty() {
        render_metadata_block(&metadata)
    } else {
        format!("{body}\n\n{}", render_metadata_block(&metadata))
    }
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
        if parent.is_same_repo() {
            closed |= tools.close_issue(parent.number).await?;
        }
    }
    Ok(closed)
}

async fn service_ready_code<F, P>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    prep: &P,
) -> Result<bool, AgentError>
where
    F: Forge + ?Sized,
    P: EnginePrep<F>,
{
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
    let input = implementation_pr_input(tools, number, &issue.title);
    // Backend-specific prep (e.g. create the head branch + a differing commit on
    // real Forgejo) must run before the PR is opened; a no-op on filesystem.
    prep.before_open_pr(tools, &input).await?;
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

pub(crate) fn implementation_pr_input<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    code_number: ItemNumber,
    issue_title: &str,
) -> harness_forge::CreatePullRequest {
    let metadata = WorkflowMetadata {
        kind: Some(ArtifactKindId::new("implementation_pr")),
        parents: vec![ArtifactRef::same_repo(code_number)],
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
