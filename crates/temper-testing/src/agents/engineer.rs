use async_trait::async_trait;
use temper_forge_model::{Forge, ItemNumber};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::{
    ArtifactKindId, ArtifactRef, ArtifactSource, WorkflowMetadata, render_metadata_block,
};

use super::support::{run_or_ignore_stale, run_pull_request_create_or_ignore_stale};
use crate::pull_request_input;

#[derive(Clone, Debug, Default)]
pub struct FakeEngineer;

/// The engineer fake for the **basic-delivery** workflow.
///
/// basic-delivery fulfils the PR with a single `open_pr` transition that carries
/// a `create_pull_request` effect, rather than the reference workflow's explicit
/// `claim_code` -> `open_pull_request` -> `request_review` sequence. The
/// resulting PR is labelled `implementation` + `landing`, which drops it
/// straight into the `landing` queue (no review gate). On `pr_ci_failed` it runs
/// `address_ci_failure`.
#[derive(Clone, Debug, Default)]
pub struct BasicEngineer;

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for FakeEngineer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        engineer_service(item, tools, &NoPrep).await
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
/// because those backends never check that a PR head is a real git ref. On real
/// Forgejo a PR head must exist as a branch before `create_pull_request`, and a
/// CI fail->pass or conflict resolution needs a new head SHA - so the Forgejo
/// worker supplies hooks that create the branch + differing commit before
/// opening the PR and push fix/conflict-resolution commits before the routing
/// transitions. The engineer state machine itself ([`engineer_service`]) is
/// shared verbatim across both topologies; only this hook differs, exactly as
/// `--architect`/`--reviewer` vary the other roles.
#[async_trait]
pub(crate) trait EnginePrep<F: Forge + ?Sized>: Send + Sync {
    /// Called with the PR input immediately before `open_pull_request`.
    async fn before_open_pr(
        &self,
        _tools: &RoleTools<'_, F>,
        _input: &temper_forge_model::CreatePullRequest,
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

    /// Called on a `pr_merge_conflict` item before clearing the conflict
    /// blocker, so a real backend can push a conflict-resolution PR head. The
    /// workflow transition itself only changes routing labels; this hook
    /// represents the external code/head update that must happen first.
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

/// The basic-delivery engineer state machine, shared by [`BasicEngineer`] and
/// the Forgejo basic-engineer wrapper. `prep` injects backend-specific side
/// effects (real PR head + CI sentinel) at the two moments a real provider needs
/// them; the no-op [`NoPrep`] keeps the filesystem/memory behavior deterministic.
///
/// It differs structurally from [`engineer_service`]: ready code is fulfilled by
/// a single `open_pr` transition whose `create_pull_request` effect (index 0 -
/// the only PR-create on the transition) is bound through the keyed runtime
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
    // identifies the artifact, while `landing` drops the opened PR straight into
    // the CI-gated landing queue (no review gate).
    let mut input = implementation_pr_input(tools, number, &issue.title);
    input.labels.push("landing".to_string());
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
) -> temper_forge_model::CreatePullRequest {
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
