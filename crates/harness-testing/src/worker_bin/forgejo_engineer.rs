//! The Forgejo-backed engineer agent.
//!
//! On the filesystem/memory backends the [`FakeEngineer`](crate::agents::FakeEngineer)
//! opens PRs and addresses CI failures with pure metadata, because those backends
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
//!   second, passing run.
//!
//! [`ForgejoEngineer`] supplies both as an [`EnginePrep`] hook around the shared
//! [`engineer_service`] state machine, so the engineer's behavior (claim, open
//! PR, request review, handle review/CI feedback) is **identical** across both
//! topologies — only the backend-specific side effects differ, exactly as
//! `--architect`/`--reviewer` vary the other roles. It is constructed only on the
//! Forgejo worker path (see [`super::forgejo`]); the filesystem path keeps the
//! plain `FakeEngineer`.

use async_trait::async_trait;
use harness_forge::{CreatePullRequest, Forge};
use harness_runner::{Agent, AgentError, RoleTools, WorkItem};
use harness_workflow::ArtifactSource;

use crate::agents::{engineer_service, EnginePrep};
use crate::forgejo_server::{commit_ci_sentinel, prepare_pull_request_head};
use crate::worker_bin::args::CiSentinelKind;

/// The engineer role on the real Forgejo backend.
///
/// Holds the connection details and per-role token the backend-neutral
/// [`FakeEngineer`] lacks, plus the [`CiSentinelKind`] policy that decides whether
/// a freshly opened PR head already carries the `ci-ok` sentinel.
pub(crate) struct ForgejoEngineer {
    base_url: String,
    /// The engineer's own role token (authorized to write the repo). Never logged.
    token: String,
    sentinel: CiSentinelKind,
}

impl ForgejoEngineer {
    pub(crate) fn new(base_url: String, token: String, sentinel: CiSentinelKind) -> Self {
        Self {
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
        prepare_pull_request_head(&self.base_url, &self.token, &owner, &name, input)
            .await
            .map_err(|error| AgentError::message(format!("forgejo PR prep failed: {error}")))?;

        // For non-CI-fail scenarios the head should pass CI from the start, so
        // seed `ci-ok` on it now. The `Deferred` policy withholds it so the first
        // run fails and the fix commit supplies the passing head SHA.
        if matches!(self.sentinel, CiSentinelKind::Present) {
            commit_ci_sentinel(
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
        commit_ci_sentinel(&self.base_url, &self.token, &owner, &name, &branch)
            .await
            .map_err(|error| AgentError::message(format!("forgejo CI fix commit failed: {error}")))
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

/// The Forgejo [`harness_agents::EngineerPrep`] for the **real** LLM engineer.
///
/// The real [`LlmEngineer`](harness_agents::LlmEngineer) is backend-agnostic and
/// takes an injected prep hook for the side effects a real Forge needs before a
/// PR opens / a CI failure is addressed. This adapter performs exactly the same
/// side effects as [`ForgejoEngineer`]'s `EnginePrep` impl above —
/// [`prepare_pull_request_head`] (real head branch + differing commit) and
/// [`commit_ci_sentinel`] (the `[ci-pass]` / `ci-ok` marker the committed workflow
/// gates on) — but bridges to the `harness-agents` trait instead of the testing
/// crate's, so the `--agents real` Forgejo worker drives the LLM engineer while
/// keeping the real-PR/real-CI mechanics. Idempotent across re-attempts.
///
/// `sentinel` mirrors the fake path: `Present` seeds `ci-ok` at PR-open (the PR
/// passes immediately), `Deferred` withholds it so the first run fails and the
/// `before_address_ci_failure` fix commit supplies the passing head SHA.
pub(crate) struct ForgejoLlmPrep {
    base_url: String,
    token: String,
    sentinel: CiSentinelKind,
}

impl ForgejoLlmPrep {
    pub(crate) fn new(base_url: String, token: String, sentinel: CiSentinelKind) -> Self {
        Self {
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

    /// Resolves the PR head branch for a `pr_ci_failed` target (see
    /// [`ForgejoEngineer::pr_head_branch`]).
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
impl<F: Forge + ?Sized> harness_agents::EngineerPrep<F> for ForgejoLlmPrep {
    async fn before_open_pr(
        &self,
        tools: &RoleTools<'_, F>,
        input: &CreatePullRequest,
    ) -> Result<(), AgentError> {
        let (owner, name) = self.repo_path(tools).await?;
        prepare_pull_request_head(&self.base_url, &self.token, &owner, &name, input)
            .await
            .map_err(|error| AgentError::message(format!("forgejo PR prep failed: {error}")))?;
        if matches!(self.sentinel, CiSentinelKind::Present) {
            commit_ci_sentinel(
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
        let Some(branch) = self.pr_head_branch(tools, target).await? else {
            return Ok(());
        };
        let (owner, name) = self.repo_path(tools).await?;
        commit_ci_sentinel(&self.base_url, &self.token, &owner, &name, &branch)
            .await
            .map_err(|error| AgentError::message(format!("forgejo CI fix commit failed: {error}")))
    }
}
