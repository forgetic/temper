//! The Forgejo-backed engineer agent.
//!
//! On the filesystem/memory backends the [`FakeEngineer`](crate::agents::FakeEngineer)
//! opens PRs and addresses CI/conflict routes with pure metadata, because those backends
//! never check that a PR head is a real git ref or run real CI. On **real
//! Forgejo** neither is true:
//!
//! - `create_pull_request` against a head branch that does not exist fails with
//!   `404` (findings-phase-0 §1), so the head branch + a differing commit must
//!   exist before the PR is opened ([`prepare_pull_request_head`]).
//! - CI is real: the committed workflow's `build` job runs `test -f ci-ok`, so a
//!   head that carries the `ci-ok` sentinel passes and one that does not fails. A
//!   commit status is keyed by SHA (findings-phase-0b), so a fail→pass needs a
//!   **new head SHA** — the engineer's fix commit adds `ci-ok`, producing a
//!   second, passing run. A merge-conflict resolution likewise pushes a small
//!   docs update before the workflow requeues landing.
//!
//! [`ForgejoEngineer`] supplies those side effects as [`EnginePrep`] hooks around
//! the shared [`engineer_service`] state machine, so the engineer's behavior
//! (claim, open PR, request review, handle review/CI/conflict feedback) is
//! **identical** across both topologies — only the backend-specific side effects differ, exactly as
//! `--architect`/`--reviewer` vary the other roles. It is constructed only on the
//! Forgejo worker path (see [`super::forgejo`]); the filesystem path keeps the
//! plain `FakeEngineer`.

use async_trait::async_trait;
use temper_forge::{CreatePullRequest, Forge};
use temper_runner::{Agent, AgentError, RoleTools, WorkItem};
use temper_workflow::ArtifactSource;

use crate::agents::{EnginePrep, basic_engineer_service, engineer_service};
use crate::forgejo_server::{
    commit_ci_sentinel, commit_conflict_resolution_update, prepare_pull_request_head,
};
use crate::worker_bin::args::CiSentinelKind;

/// The engineer role on the real Forgejo backend.
///
/// Holds the connection details and per-role token the backend-neutral
/// [`FakeEngineer`] lacks, plus the [`CiSentinelKind`] policy that decides whether
/// a freshly opened PR head already carries the `ci-ok` sentinel.
pub(crate) struct ForgejoEngineer {
    /// Clock capability of the engine task the agent runs under; REST
    /// request deadlines compute against it.
    cx: temper_io_engine::Cx,
    base_url: String,
    /// The engineer's own role token (authorized to write the repo). Never logged.
    token: String,
    sentinel: CiSentinelKind,
}

impl ForgejoEngineer {
    pub(crate) fn new(
        cx: temper_io_engine::Cx,
        base_url: String,
        token: String,
        sentinel: CiSentinelKind,
    ) -> Self {
        Self {
            cx,
            base_url,
            token,
            sentinel,
        }
    }

    async fn repo_path<F: Forge + ?Sized>(
        &self,
        tools: &RoleTools<'_, F>,
    ) -> Result<(String, String), AgentError> {
        let repository = tools
            .get_repository()
            .await?
            .ok_or_else(|| AgentError::message(format!("repository {} not found", tools.repo())))?;
        Ok((repository.owner, repository.name))
    }

    /// Resolves the PR head (source) branch for a `pr_ci_failed` target so the
    /// fix commit lands on the right branch. Returns `None` when the target is
    /// not a pull request the engineer can read (stale item).
    async fn pr_head_branch<F: Forge + ?Sized>(
        &self,
        tools: &RoleTools<'_, F>,
        target: ArtifactSource,
    ) -> Result<Option<String>, AgentError> {
        let ArtifactSource::PullRequest { number } = target else {
            return Ok(None);
        };
        let Some(pull_request) = tools.get_pull_request(number).await? else {
            return Ok(None);
        };
        Ok(Some(pull_request.source.branch))
    }
}

#[async_trait]
impl<F: Forge + ?Sized> EnginePrep<F> for ForgejoEngineer {
    async fn before_open_pr(
        &self,
        tools: &RoleTools<'_, F>,
        input: &CreatePullRequest,
    ) -> Result<(), AgentError> {
        let (owner, name) = self.repo_path(tools).await?;
        // Make the head branch real (branch + a differing commit) before the PR
        // is opened; idempotent across re-attempts.
        prepare_pull_request_head(&self.cx, &self.base_url, &self.token, &owner, &name, input)
            .await
            .map_err(|error| AgentError::message(format!("forgejo PR prep failed: {error}")))?;

        // For non-CI-fail scenarios the head should pass CI from the start, so
        // seed `ci-ok` on it now. The `Deferred` policy withholds it so the first
        // run fails and the fix commit supplies the passing head SHA.
        if matches!(self.sentinel, CiSentinelKind::Present) {
            commit_ci_sentinel(
                &self.cx,
                &self.base_url,
                &self.token,
                &owner,
                &name,
                input.source.branch.as_str(),
            )
            .await
            .map_err(|error| {
                AgentError::message(format!("forgejo CI sentinel seed failed: {error}"))
            })?;
        }
        Ok(())
    }

    async fn before_address_ci_failure(
        &self,
        tools: &RoleTools<'_, F>,
        target: ArtifactSource,
    ) -> Result<(), AgentError> {
        // The fix: push `ci-ok` to the failing PR's head branch, producing a new
        // head SHA whose CI run passes. Idempotent — a re-attempt of an already
        // fixed head is a tolerated no-op.
        let Some(branch) = self.pr_head_branch(tools, target).await? else {
            return Ok(());
        };
        let (owner, name) = self.repo_path(tools).await?;
        commit_ci_sentinel(
            &self.cx,
            &self.base_url,
            &self.token,
            &owner,
            &name,
            &branch,
        )
        .await
        .map_err(|error| AgentError::message(format!("forgejo CI fix commit failed: {error}")))
    }

    async fn before_resolve_merge_conflict(
        &self,
        tools: &RoleTools<'_, F>,
        target: ArtifactSource,
    ) -> Result<(), AgentError> {
        let Some(branch) = self.pr_head_branch(tools, target).await? else {
            return Ok(());
        };
        let (owner, name) = self.repo_path(tools).await?;
        commit_conflict_resolution_update(
            &self.cx,
            &self.base_url,
            &self.token,
            &owner,
            &name,
            &branch,
        )
        .await
        .map_err(|error| {
            AgentError::message(format!(
                "forgejo conflict-resolution commit failed: {error}"
            ))
        })
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for ForgejoEngineer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        // Reuse the `FakeEngineer` state machine verbatim (see `engineer_service`);
        // only the prep hook — this type — differs from the filesystem topology.
        engineer_service(item, tools, self).await
    }
}

/// The engineer role for the **basic-delivery** workflow on the real Forgejo
/// backend.
///
/// It is the Forgejo counterpart of the backend-neutral
/// [`BasicEngineer`](crate::agents::BasicEngineer): it drives the shared
/// [`basic_engineer_service`] state machine (single `open_pr`, then
/// `address_ci_failure`) and supplies the same Forgejo [`EnginePrep`]
/// side-effects as [`ForgejoEngineer`] — a real PR head branch + `ci-ok`
/// sentinel before the PR-creating transition, and a fresh head SHA on CI
/// failure. Only the engineer *transition* differs from the reference topology;
/// the prep is identical, so it is reused by composition.
pub(crate) struct ForgejoBasicEngineer {
    prep: ForgejoEngineer,
}

impl ForgejoBasicEngineer {
    pub(crate) fn new(
        cx: temper_io_engine::Cx,
        base_url: String,
        token: String,
        sentinel: CiSentinelKind,
    ) -> Self {
        Self {
            prep: ForgejoEngineer::new(cx, base_url, token, sentinel),
        }
    }
}

#[async_trait]
impl<F: Forge + ?Sized> Agent<F> for ForgejoBasicEngineer {
    async fn service(&self, item: &WorkItem, tools: &RoleTools<'_, F>) -> Result<bool, AgentError> {
        // Reuse the `BasicEngineer` state machine verbatim; only the prep hook —
        // the wrapped `ForgejoEngineer` — differs from the filesystem topology.
        basic_engineer_service(item, tools, &self.prep).await
    }
}
