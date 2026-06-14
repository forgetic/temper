//! Deterministic fake role behavior for reference-delivery runner tests.
//!
//! These fakes are behavior-only test adapters. They mutate workflow state only
//! through `RoleTools`, exactly like a real agent adapter would, and keep all
//! orchestration in runner workers/stages.

use async_trait::async_trait;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Mutex;
use temper_forge::{CreateIssue, Forge, ItemNumber, RepositoryId};
use temper_runner::{Agent, AgentError, AgentRegistry, RoleTools, WorkItem};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, ExecutionError, RoleId, TransitionId,
    WorkflowMetadata, global_child_correlation_key, parse_metadata_block, render_metadata_block,
};

use crate::pull_request_input;

#[derive(Clone, Debug, Default)]
pub struct FakeArchitect;

#[derive(Clone, Debug, Default)]
pub struct FakeEngineer;

/// The architect fake for the **basic-delivery** workflow.
///
/// basic-delivery has no design/breakdown branch: the architect's only outcome
/// is to rewrite an `untriaged` intake issue into a `code` + `ready` issue with
/// a crisp body. It runs the routed terminal transition `triage_intake_to_code`
/// directly, binding the rewritten body through the same keyed `set_body`
/// runtime seam that [`FakeArchitect`] uses for the reference workflow — there is
/// no fan-out, no `needs_design`/`needs_breakdown`, and no second transition.
#[derive(Clone, Debug, Default)]
pub struct BasicArchitect;

/// The engineer fake for the **basic-delivery** workflow.
///
/// basic-delivery fulfils the PR with a single `open_pr` transition that carries
/// a `create_pull_request` effect, rather than the reference workflow's explicit
/// `claim_code` → `open_pull_request` → `request_review` sequence. The resulting
/// PR is labelled `implementation`, which drops it straight into the `landing`
/// queue (no review gate). On `pr_ci_failed` it runs `address_ci_failure`.
#[derive(Clone, Debug, Default)]
pub struct BasicEngineer;

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
pub const ARCHITECT_PLAN_BEGIN: &str = "<!-- temper:architect-plan";
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

/// Registry for the **basic-delivery** workflow.
///
/// basic-delivery binds only the queue-subscribing roles `architect` and
/// `engineer`; `mechanical` is queue-less (serviced by the mechanical worker,
/// not a role worker) and there is no reviewer/owner/human. Registering only the
/// two role agents keeps this set aligned with the fixture's role shape.
pub fn basic_fake_registry<F>() -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
{
    basic_fake_registry_with(BasicEngineer)
}

/// Registry for the basic-delivery workflow with a caller-supplied engineer.
///
/// The Forgejo worker path swaps in a Forgejo-backed engineer (real PR head + CI
/// sentinel) while keeping the backend-neutral [`BasicArchitect`]; the
/// filesystem/memory path uses the plain [`BasicEngineer`].
pub fn basic_fake_registry_with<F, E>(engineer: E) -> AgentRegistry<F>
where
    F: Forge + ?Sized + 'static,
    E: Agent<F> + 'static,
{
    let mut registry = AgentRegistry::new();
    registry.register(RoleId::new("architect"), BasicArchitect);
    registry.register(RoleId::new("engineer"), engineer);
    registry
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

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for BasicArchitect {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        basic_architect_service(item, tools).await
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for BasicEngineer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        basic_engineer_service(item, tools, &NoPrep).await
    }
}

/// Side-effect hook the engineer state machine calls at backend-specific moments.
///
/// On the filesystem/memory backends this is a no-op ([`NoPrep`]): the fake
/// engineer opens PRs and addresses CI/merge-conflict routes with pure metadata,
/// because those backends never check that a PR head is a real git ref. On **real
/// Forgejo** a PR head must exist as a branch before `create_pull_request`, and a
/// CI fail→pass or conflict resolution needs a new head SHA — so the Forgejo
/// worker supplies hooks that create the branch + differing commit before opening
/// the PR and push fix/conflict-resolution commits before the routing
/// transitions. The engineer state machine itself ([`engineer_service`]) is
/// shared verbatim across both topologies; only this hook differs, exactly as
/// `--architect`/`--reviewer` vary the other roles.
#[async_trait]
pub(crate) trait EnginePrep<F: Forge + ?Sized>: Send + Sync {
    /// Called with the PR input immediately before `open_pull_request`.
    async fn before_open_pr(
        &self,
        _tools: &RoleTools<'_, F>,
        _input: &temper_forge::CreatePullRequest,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called on a `pr_ci_failed` item just before the CI-failure transition
    /// runs, so a backend that needs a new head SHA can push one.
    async fn before_address_ci_failure(
        &self,
        _tools: &RoleTools<'_, F>,
        _target: ArtifactSource,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called on a `pr_merge_conflict` item before requeueing landing, so a real
    /// backend can push a conflict-resolution PR head. The workflow transition
    /// itself only changes routing labels; this hook represents the external
    /// code/head update that must happen first.
    async fn before_resolve_merge_conflict(
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
/// behavior deterministic and backend-neutral.
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
        let transition = ci_failure_transition(item, tools).await?;
        return run_or_ignore_stale(tools, item.target, transition).await;
    }
    if item.queue.as_str() == "pr_merge_conflict" {
        prep.before_resolve_merge_conflict(tools, item.target)
            .await?;
        return run_or_ignore_stale(tools, item.target, "resolve_merge_conflict").await;
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

/// Correlation key for the `set_body` rewrite the basic architect binds when it
/// runs `triage_intake_to_code`.
fn basic_triage_body_key(number: ItemNumber) -> String {
    format!("triage-intake-code-{}", number.get())
}

/// The basic-delivery architect state machine.
///
/// On the `triage`/`intake` queue it runs the single routed terminal transition
/// `triage_intake_to_code`, binding a rewritten crisp body through the keyed
/// `set_body` runtime seam (effect index 0 — the transition declares exactly one
/// `set_body`). Unlike [`service_architect`] there is no fan-out and no second
/// triage branch: the one outcome marks the issue `code` + `ready`.
async fn basic_architect_service<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<bool, AgentError> {
    if item.queue.as_str() != "triage" || item.kind.as_str() != "intake" {
        return Ok(false);
    }
    let ArtifactSource::Issue { number } = item.target else {
        return Ok(false);
    };
    let Some(issue) = tools.get_issue(number).await? else {
        return Ok(false);
    };
    let body = basic_triaged_body(&issue.title, &issue.body);
    run_set_body_or_ignore_stale(
        tools,
        item.target,
        "triage_intake_to_code",
        0,
        basic_triage_body_key(number),
        body,
    )
    .await
}

/// Builds the rewritten code-spec body the basic architect authors when it
/// triages an intake issue. It is deliberately a non-empty, deterministic
/// rewrite of the intake context so the `set_body` effect has real content.
fn basic_triaged_body(title: &str, intake_body: &str) -> String {
    let context = if intake_body.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n## Intake context\n\n{}", intake_body.trim())
    };
    format!("## Code spec\n\nImplement: {title}{context}")
}

/// The basic-delivery engineer state machine, shared by [`BasicEngineer`] and the
/// Forgejo basic-engineer wrapper. `prep` injects backend-specific side effects
/// (real PR head + CI sentinel) at the two moments a real provider needs them;
/// the no-op [`NoPrep`] keeps the filesystem/memory behavior deterministic.
///
/// It differs structurally from [`engineer_service`]: ready code is fulfilled by
/// a **single** `open_pr` transition whose `create_pull_request` effect (index 0
/// — the only PR-create on the transition) is bound through the keyed runtime
/// seam, rather than an explicit `claim_code` + `open_pull_request` +
/// `request_review` sequence. `pr_ci_failed` runs `address_ci_failure`.
pub(crate) async fn basic_engineer_service<F, P>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
    prep: &P,
) -> Result<bool, AgentError>
where
    F: Forge + ?Sized,
    P: EnginePrep<F>,
{
    if item.queue.as_str() == "code_ready" && item.kind.as_str() == "code" {
        return basic_service_ready_code(item, tools, prep).await;
    }
    if item.queue.as_str() == "pr_ci_failed" {
        prep.before_address_ci_failure(tools, item.target).await?;
        return run_or_ignore_stale(tools, item.target, "address_ci_failure").await;
    }
    Ok(false)
}

async fn basic_service_ready_code<F, P>(
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

    // Reuse the reference implementation-PR shape: the `implementation` label
    // drops the opened PR straight into the `landing` queue (no review gate).
    let input = implementation_pr_input(tools, number, &issue.title);
    // Backend-specific prep (e.g. create the head branch + a differing commit on
    // real Forgejo) must run before the PR-creating transition; a no-op on the
    // filesystem/memory backends.
    prep.before_open_pr(tools, &input).await?;
    let correlation_key = format!("pr-for-code-{}", number.get());
    run_pull_request_create_or_ignore_stale(
        tools,
        item.target,
        "open_pr",
        0,
        correlation_key,
        input,
    )
    .await
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

async fn ci_failure_transition<F: Forge + ?Sized>(
    item: &WorkItem,
    tools: &RoleTools<'_, F>,
) -> Result<&'static str, AgentError> {
    let ArtifactSource::PullRequest { number } = item.target else {
        return Ok("address_ci_failure");
    };
    let Some(pull_request) = tools.get_pull_request(number).await? else {
        return Ok("address_ci_failure");
    };
    if pull_request.labels.iter().any(|label| label == "landing") {
        Ok("address_landing_ci_failure")
    } else {
        Ok("address_ci_failure")
    }
}

pub(crate) fn implementation_pr_input<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    code_number: ItemNumber,
    issue_title: &str,
) -> temper_forge::CreatePullRequest {
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

/// Runs `transition` binding `body` into the keyed `set_body` runtime seam at
/// `effect_index`, treating a stale-state failure as a quiet skip so the next
/// tick retries against fresh state (mirrors [`run_or_ignore_stale`]).
async fn run_set_body_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
    effect_index: usize,
    correlation_key: String,
    body: String,
) -> Result<bool, AgentError> {
    match tools
        .run_with_set_body_at(
            target,
            &TransitionId::new(transition),
            effect_index,
            correlation_key,
            body,
        )
        .await
    {
        Ok(_) => Ok(true),
        Err(error) if stale_execution(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Runs `transition` binding `input` into the keyed `create_pull_request`
/// runtime seam at `effect_index`, treating a stale-state failure as a quiet
/// skip so the next tick retries against fresh state (mirrors
/// [`run_or_ignore_stale`]).
async fn run_pull_request_create_or_ignore_stale<F: Forge + ?Sized>(
    tools: &RoleTools<'_, F>,
    target: ArtifactSource,
    transition: &str,
    effect_index: usize,
    correlation_key: String,
    input: temper_forge::CreatePullRequest,
) -> Result<bool, AgentError> {
    match tools
        .run_with_pull_request_create_at(
            target,
            &TransitionId::new(transition),
            effect_index,
            correlation_key,
            input,
        )
        .await
    {
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
