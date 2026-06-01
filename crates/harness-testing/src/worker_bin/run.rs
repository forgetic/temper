//! Worker construction and the per-process run loop.
//!
//! Each invocation builds one Forge handle, resolves the repository by its
//! owner/name path, constructs the matching runner worker, and drives it with a
//! [`PollLoop`] until a stop signal fires. The worker set and labels are derived
//! from the compiled reference workflow and [`RunnerConfig`]; nothing here
//! hardcodes role, queue, or label names.
//!
//! # Backend seam
//!
//! The backend is selected by `--backend` (see [`Backend`]). Everything above
//! `dyn Forge` is identical for both backends: the same registry, workers, and
//! poll loop run against either handle. Only the *handle construction* and the
//! *executor that drives the futures* differ, and both are isolated here:
//!
//! - `--backend filesystem` builds a [`FilesystemForge`], derives identity with
//!   [`FilesystemForge::as_user`], and drives the (synchronous) futures with the
//!   crate's no-op [`block_on`](crate::block_on). This is the default, so the
//!   existing multiprocess test is untouched, and it keeps the fake `--kind ci`
//!   producer.
//! - `--backend forgejo` builds a [`ForgejoForge`](harness_forge_forgejo::ForgejoForge)
//!   whose identity is the per-role token (no `as_user`), and drives the
//!   network-IO futures on a Tokio runtime (see [`super::forgejo`]). CI is
//!   produced by the real `forgejo-runner` and read via the Phase 3b path, so
//!   `--kind ci` is rejected for this backend at parse time.

use chrono::{DateTime, Duration, Utc};
use harness_forge::{
    CreateRepository, Forge, ForgeError, RepositoryId, RepositoryPath, UpsertLabel, User,
};
use harness_forge_filesystem::FilesystemForge;
use harness_runner::{
    ManualClock, MechanicalWorker, MultiRepoMechanicalWorker, MultiRepoRoleWorker, PollLoop,
    RepositoryJournal, RepositorySet, RepositoryTarget, RoleWorker, RunReport, RunnerConfig,
};
use harness_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, RoleId};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::agents::{
    fake_registry_with, ClosingArchitect, FakeArchitect, FakeReviewer,
    RequestChangesThenApproveReviewer,
};
use crate::worker_bin::args::{
    AgentsAuthKind, AgentsKind, ArchitectKind, Backend, CiPolicyKind, ClockKind, ReviewerKind,
    RoleBehavior, WorkerArgs, WorkerKind,
};
use crate::worker_bin::multi_ci::MultiRepoCiWorker;
use crate::{block_on, runner_config, workflow};
use harness_runner::AgentRegistry;

/// Errors that abort a worker invocation with a non-zero exit.
#[derive(Debug)]
pub enum RunError {
    /// A Forge operation failed.
    Forge(ForgeError),
    /// The configured repository path does not exist in the store.
    RepositoryMissing { owner: String, name: String },
    /// The requested role has no agent in the fake registry.
    UnknownRole { role: String },
    /// The poll loop or a worker tick failed.
    Drive(Box<dyn Error + Send + Sync + 'static>),
    /// A backend-construction step failed (e.g. invalid Forgejo configuration).
    ///
    /// The message must never contain a token or password.
    Backend(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Forge(error) => write!(formatter, "forge operation failed: {error}"),
            RunError::RepositoryMissing { owner, name } => {
                write!(formatter, "repository {owner}/{name} not found in store")
            }
            RunError::UnknownRole { role } => {
                write!(formatter, "no fake agent registered for role '{role}'")
            }
            RunError::Drive(error) => write!(formatter, "worker run failed: {error}"),
            RunError::Backend(message) => write!(formatter, "backend setup failed: {message}"),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Forge(error) => Some(error),
            RunError::Drive(error) => Some(error.as_ref()),
            RunError::RepositoryMissing { .. }
            | RunError::UnknownRole { .. }
            | RunError::Backend(_) => None,
        }
    }
}

impl From<ForgeError> for RunError {
    fn from(error: ForgeError) -> Self {
        Self::Forge(error)
    }
}

/// Runs the worker described by `args` to completion, blocking on async work.
///
/// Dispatches on the selected [`Backend`]: the filesystem path drives the
/// synchronous reference backend with [`block_on`], while the Forgejo path
/// drives real network IO on a Tokio runtime (see [`super::forgejo`]).
pub fn run(args: &WorkerArgs) -> Result<RunReport, RunError> {
    match &args.backend {
        Backend::Filesystem => run_filesystem(args),
        Backend::Forgejo(forgejo) => super::forgejo::run(args, forgejo),
    }
}

/// Drives the worker against the filesystem reference backend.
fn run_filesystem(args: &WorkerArgs) -> Result<RunReport, RunError> {
    match &args.kind {
        WorkerKind::Provision => {
            block_on(provision(args))?;
            Ok(RunReport::default())
        }
        WorkerKind::Role {
            role,
            user,
            behavior,
        } => run_role(args, role, user, *behavior),
        WorkerKind::Mechanical => run_mechanical(args),
        WorkerKind::Ci { policy } => run_ci(args, *policy),
    }
}

/// Creates the repository and upserts every label the workflow declares.
async fn provision(args: &WorkerArgs) -> Result<(), RunError> {
    let forge = FilesystemForge::new(&args.root);
    let workflow = workflow();
    let compiled = workflow.compile();
    for path in &args.repositories {
        let repo = ensure_repository(&forge, path).await?;
        upsert_labels(&forge, &repo, &compiled).await?;
    }
    Ok(())
}

async fn ensure_repository(
    forge: &FilesystemForge,
    path: &RepositoryPath,
) -> Result<RepositoryId, RunError> {
    if let Some(repo) = forge.get_repository_by_path(path).await? {
        return Ok(repo.id);
    }
    let repo = forge
        .create_repository(CreateRepository {
            owner: path.owner.clone(),
            name: path.name.clone(),
            default_branch: runner_config().repository.default_branch,
            description: None,
        })
        .await?;
    Ok(repo.id)
}

/// Upserts every workflow label against `repo`.
///
/// Generic over the Forge so both backends share one label-provisioning path.
pub(super) async fn upsert_labels<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    compiled: &CompiledWorkflow,
) -> Result<(), RunError> {
    for label in compiled.labels().labels() {
        forge
            .upsert_label(
                repo,
                UpsertLabel {
                    name: label.id.to_string(),
                    color: None,
                    description: None,
                },
            )
            .await?;
    }
    Ok(())
}

fn run_role(
    args: &WorkerArgs,
    role: &str,
    user_handle: &str,
    behavior: RoleBehavior,
) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let config = runner_config();
    let role_id = RoleId::new(role);

    // The filesystem backend needs no engineer prep (no real git refs / CI), so
    // `--agents real` here uses the LLM agents with `NoPrep`. `--agents fake`
    // (the default) is unchanged.
    let registry = match args.agents {
        AgentsKind::Fake => registry_for(behavior),
        AgentsKind::Real => real_registry_for(args, behavior, Arc::new(harness_agents::NoPrep))?,
    };
    let agent = registry
        .get(&role_id)
        .ok_or_else(|| RunError::UnknownRole { role: role.into() })?
        .clone();

    // Resolve the identity the role acts as from the runner config binding, so
    // the handle's `current_user` matches the role-to-user map the workflow
    // executor uses. `--user` is the human-readable cross-check.
    let user = resolve_role_user(&config, &role_id, user_handle);
    let base = FilesystemForge::new(&args.root);
    let forge = base.as_user(user);
    if args.repositories.len() == 1 {
        let repo = block_on(resolve_repository(&forge, &args.owner, &args.name))?;
        let worker = RoleWorker::new(
            &workflow,
            &compiled,
            &forge as &dyn Forge,
            &repo,
            role_id.clone(),
            agent,
            config.execution_context(&role_id),
        );
        drive(args, &worker)
    } else {
        let repositories = block_on(resolve_repository_set(&forge, &args.repositories))?;
        let worker = MultiRepoRoleWorker::new(
            &workflow,
            &compiled,
            &forge as &dyn Forge,
            repositories,
            role_id.clone(),
            agent,
            config.execution_context(&role_id),
        );
        drive(args, &worker)
    }
}

/// Builds the fake registry whose architect and reviewer match `behavior`.
///
/// The registry is over `dyn Forge` so the same agents drive either backend
/// behind the object-safe Forge trait; the fakes already implement
/// `Agent<F>` for every `F: Forge + ?Sized`. This mirrors the in-process
/// scenario wiring in `harness-runner/tests/end_to_end.rs`, so the same
/// scenarios converge across both topologies.
pub(super) fn registry_for(behavior: RoleBehavior) -> AgentRegistry<dyn Forge> {
    match (behavior.architect, behavior.reviewer) {
        (ArchitectKind::Default, ReviewerKind::Default) => {
            fake_registry_with(FakeArchitect, FakeReviewer)
        }
        (ArchitectKind::Default, ReviewerKind::RequestChangesThenApprove) => {
            fake_registry_with(FakeArchitect, RequestChangesThenApproveReviewer::new())
        }
        (ArchitectKind::Closing, ReviewerKind::Default) => {
            fake_registry_with(ClosingArchitect, FakeReviewer)
        }
        (ArchitectKind::Closing, ReviewerKind::RequestChangesThenApprove) => {
            fake_registry_with(ClosingArchitect, RequestChangesThenApproveReviewer::new())
        }
    }
}

/// Builds the **real** (LLM-backed) registry whose architect/reviewer variants
/// match `behavior` and whose engineer carries `engineer_prep`.
///
/// This is the `--agents real` counterpart of [`registry_for`]: it constructs the
/// shared [`ProviderConfig`](harness_agents::ProviderConfig) once for the
/// selected auth mode (DeepSeek key or ChatGPT OAuth login, resolved at runtime
/// and never logged) and maps every role to its LLM agent through
/// [`harness_agents::real_registry_with`]. `engineer_prep` carries the
/// backend-specific PR-head/CI side effects: `NoPrep` on filesystem, the Forgejo
/// prep on the real backend (see [`super::forgejo`]).
///
/// The provider is built with [`provider_for`], which resolves the auth mode,
/// codex model, and auth-file path from `args` (precedence CLI > env > default)
/// and runs an **eager credential preflight** so a missing key or login fails
/// here — before any worker tick.
pub(super) fn real_registry_for(
    args: &WorkerArgs,
    behavior: RoleBehavior,
    engineer_prep: Arc<dyn harness_agents::EngineerPrep<dyn Forge>>,
) -> Result<AgentRegistry<dyn Forge>, RunError> {
    let provider = provider_for(args)?;
    let config = harness_agents::RealRegistryConfig {
        architect_closing: matches!(behavior.architect, ArchitectKind::Closing),
        reviewer_request_changes_then_approve: matches!(
            behavior.reviewer,
            ReviewerKind::RequestChangesThenApprove
        ),
        engineer_prep,
    };
    Ok(harness_agents::real_registry_with(provider, config))
}

/// Builds the shared [`ProviderConfig`](harness_agents::ProviderConfig) for the
/// auth mode selected on `args`.
///
/// Maps the worker's [`AgentsAuthKind`] onto [`harness_agents::AuthChoice`] and
/// forwards the `--codex-model` / `--auth-file` overrides (each `None` falls back
/// to its env var then the built-in default inside `harness-agents`). Anthropic
/// model selection is env-only (`HARNESS_AGENTS_ANTHROPIC_MODEL`). The eager
/// preflight inside `from_auth` surfaces a missing DeepSeek key, ChatGPT login,
/// or Anthropic login as a [`RunError::Backend`] setup error, before any worker
/// tick; OAuth errors point the operator at the matching `pi /login ...` command.
fn provider_for(args: &WorkerArgs) -> Result<harness_agents::ProviderConfig, RunError> {
    let choice = match args.auth {
        AgentsAuthKind::ChatGptOAuth => harness_agents::AuthChoice::ChatGptOAuth,
        AgentsAuthKind::DeepSeek => harness_agents::AuthChoice::DeepSeek,
        AgentsAuthKind::AnthropicOAuth => harness_agents::AuthChoice::AnthropicOAuth,
    };
    harness_agents::ProviderConfig::from_auth(
        choice,
        args.codex_model.clone(),
        args.auth_file.clone(),
    )
    .map_err(|error| RunError::Backend(error.to_string()))
}

pub(super) fn resolve_role_user(
    config: &RunnerConfig,
    role: &RoleId,
    fallback_handle: &str,
) -> User {
    config
        .role_binding(role)
        .map(|binding| binding.user.clone())
        .unwrap_or_else(|| crate::actor_user(fallback_handle))
}

fn run_mechanical(args: &WorkerArgs) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let config = runner_config();
    let forge = FilesystemForge::new(&args.root);
    if args.repositories.len() == 1 {
        let repo = block_on(resolve_repository(&forge, &args.owner, &args.name))?;

        // The journal is per-process fast-recovery state, not durable coordination:
        // leases live in Forge metadata (ADR 0013/0018), so the mechanical worker
        // re-derives everything it needs from Forge each tick. A fresh in-memory
        // journal per process is therefore correct across a restart.
        let journal = InMemoryJournal::new();
        let worker = MechanicalWorker::new(
            &workflow,
            &forge as &dyn Forge,
            &repo,
            &journal,
            LeasePolicy::new(config.lease_ttl),
        );
        drive(args, &worker)
    } else {
        let repositories = block_on(resolve_repository_set(&forge, &args.repositories))?;
        let journals: Vec<InMemoryJournal> = repositories
            .repositories()
            .iter()
            .map(|_| InMemoryJournal::new())
            .collect();
        let bindings: Vec<RepositoryJournal<'_, InMemoryJournal>> = repositories
            .repositories()
            .iter()
            .zip(journals.iter())
            .map(|(repository, journal)| RepositoryJournal {
                repository: &repository.id,
                journal,
            })
            .collect();
        let worker = MultiRepoMechanicalWorker::new(
            &workflow,
            &forge as &dyn Forge,
            repositories.clone(),
            bindings,
            LeasePolicy::new(config.lease_ttl),
        )
        .map_err(|error| RunError::Backend(error.to_string()))?;
        drive(args, &worker)
    }
}

fn run_ci(args: &WorkerArgs, policy: CiPolicyKind) -> Result<RunReport, RunError> {
    let forge = FilesystemForge::new(&args.root);
    let repos = block_on(resolve_repository_ids(&forge, &args.repositories))?;
    let worker = MultiRepoCiWorker::new(&forge, repos, policy);
    drive(args, &worker)
}

/// Resolves the repository id by owner/name, generic over the Forge backend.
pub(super) async fn resolve_repository<F: Forge + ?Sized>(
    forge: &F,
    owner: &str,
    name: &str,
) -> Result<RepositoryId, RunError> {
    forge
        .get_repository_by_path(&RepositoryPath::new(owner, name))
        .await?
        .map(|repo| repo.id)
        .ok_or_else(|| RunError::RepositoryMissing {
            owner: owner.into(),
            name: name.into(),
        })
}

pub(super) async fn resolve_repository_set<F: Forge + ?Sized>(
    forge: &F,
    paths: &[RepositoryPath],
) -> Result<RepositorySet, RunError> {
    let mut targets = Vec::new();
    for path in paths {
        let repo = forge.get_repository_by_path(path).await?.ok_or_else(|| {
            RunError::RepositoryMissing {
                owner: path.owner.clone(),
                name: path.name.clone(),
            }
        })?;
        targets.push(RepositoryTarget::new(
            repo.id,
            RepositoryPath::new(repo.owner, repo.name),
        ));
    }
    Ok(RepositorySet::new(targets))
}

async fn resolve_repository_ids<F: Forge + ?Sized>(
    forge: &F,
    paths: &[RepositoryPath],
) -> Result<Vec<RepositoryId>, RunError> {
    let mut repos = Vec::new();
    for path in paths {
        repos.push(resolve_repository(forge, &path.owner, &path.name).await?);
    }
    Ok(repos)
}

/// Drives `worker` with a poll loop until the stop signal fires, on [`block_on`].
///
/// This is the *filesystem* executor: the reference backend's futures resolve
/// synchronously, so the crate's no-op waker is sufficient. The Forgejo path
/// uses [`super::forgejo::drive_async`] on a real reactor instead.
fn drive<W: harness_runner::Worker>(args: &WorkerArgs, worker: &W) -> Result<RunReport, RunError> {
    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let report = match args.clock {
        ClockKind::Deterministic => {
            // Seed near the filesystem backend's logical-clock origin (Unix
            // epoch) and advance one second per poll. This matches how the
            // in-process scenarios assert: `owner_alignment`'s `max_age` is
            // evaluated against epoch-based logical timestamps, so a wall-clock
            // `now` would make a small fresh cohort look stale and mis-fire.
            //
            // CLOCK-FIDELITY SEAM (Phase 5 "swap to real"): a real provider
            // backend writes wall-clock timestamps, so production passes
            // `--clock wall` and this deterministic branch goes away.
            let clock = ManualClock::with_tick_step(epoch(), Duration::seconds(1));
            let poll = PollLoop::with_clock(worker, args.poll_interval, clock);
            block_on(poll.run_until(|| stop.should_stop()))
        }
        ClockKind::Wall => {
            let poll = PollLoop::new(worker, args.poll_interval);
            block_on(poll.run_until(|| stop.should_stop()))
        }
    };
    report.map_err(|error| RunError::Drive(Box::new(error)))
}

pub(super) fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is valid")
}

/// Stop predicate: fire when the sentinel file exists or the deadline passes.
pub(super) struct StopSignal {
    stop_file: Option<PathBuf>,
    deadline: Option<Instant>,
}

impl StopSignal {
    pub(super) fn new(stop_file: Option<PathBuf>, run_secs: Option<u64>) -> Self {
        Self {
            stop_file,
            deadline: run_secs.map(|secs| Instant::now() + std::time::Duration::from_secs(secs)),
        }
    }

    pub(super) fn should_stop(&self) -> bool {
        if let Some(path) = &self.stop_file {
            if sentinel_exists(path) {
                return true;
            }
        }
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return true;
            }
        }
        false
    }
}

fn sentinel_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;
