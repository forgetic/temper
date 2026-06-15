//! Worker construction and the per-process run loop.
//!
//! Each invocation builds one Forge handle, resolves the repository by its
//! owner/name path, constructs the matching runner worker, and drives it with a
//! [`PollLoop`] until a stop signal fires. The worker set and labels are derived
//! from the resolved workflow and its [`RunnerConfig`] (the bundled
//! reference-delivery default, or the `--workflow <path>` /
//! `TEMPER_WORKFLOW_FILE` selection — see [`resolve_workflow_and_config`]);
//! nothing here hardcodes role, queue, or label names.
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
//! - `--backend forgejo` builds a [`ForgejoForge`](temper_forge_forgejo::ForgejoForge)
//!   whose identity is the per-role token (no `as_user`), and drives the
//!   network-IO futures on a Tokio runtime (see [`super::forgejo`]). CI is
//!   produced by the real `forgejo-runner` and read via the Phase 3b path, so
//!   `--kind ci` is rejected for this backend at parse time.

use chrono::{DateTime, Duration, Utc};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;
use temper_forge_filesystem::FilesystemForge;
use temper_forge_model::{
    CreateRepository, Forge, ForgeError, RepositoryId, RepositoryPath, UpsertLabel, User,
};
use temper_runner::{
    ManualClock, MechanicalWorker, MultiRepoMechanicalWorker, MultiRepoRoleWorker, PollLoop,
    RepositoryJournal, RepositorySet, RepositoryTarget, RoleWorker, RunReport, RunnerConfig,
};
use temper_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, RoleId, ValidatedWorkflow};

use crate::agents::{
    ClosingArchitect, FakeArchitect, FakeReviewer, RequestChangesThenApproveReviewer,
    basic_fake_registry, fake_registry_with,
};
use crate::worker_bin::args::{
    ArchitectKind, Backend, CiPolicyKind, ClockKind, ProfileKind, ReviewerKind, RoleBehavior,
    WorkerArgs, WorkerKind,
};
use crate::worker_bin::multi_ci::MultiRepoCiWorker;
use crate::{WorkflowLoadError, block_on, resolve_workflow, runner_config_for_workflow};
use temper_runner::AgentRegistry;

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
    /// The runtime-selected `--workflow` file could not be read, parsed, or
    /// validated. The message names the file and the reason; it never contains a
    /// secret (workflow files are not secret, and the loader echoes only the
    /// path plus the read/parse/validation detail).
    Workflow(WorkflowLoadError),
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
            RunError::Workflow(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Forge(error) => Some(error),
            RunError::Drive(error) => Some(error.as_ref()),
            RunError::Workflow(error) => Some(error),
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

impl From<WorkflowLoadError> for RunError {
    fn from(error: WorkflowLoadError) -> Self {
        Self::Workflow(error)
    }
}

/// Resolves the worker's workflow + derived runner config from `args`.
///
/// `--workflow <path>` (or `TEMPER_WORKFLOW_FILE`) selects the file; with neither
/// set this returns the bundled reference-delivery default, byte-for-byte
/// identical to the historic `crate::workflow()` / `crate::runner_config()`. The
/// runner config derives its role→user bindings from the workflow's
/// queue-subscribing roles, so a runtime-selected spec binds its own roles.
pub(super) fn resolve_workflow_and_config(
    args: &WorkerArgs,
) -> Result<(ValidatedWorkflow, RunnerConfig), RunError> {
    let workflow = resolve_workflow(args.workflow_file.as_ref())?;
    let config = runner_config_for_workflow(&workflow);
    Ok((workflow, config))
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
    let (workflow, config) = resolve_workflow_and_config(args)?;
    let compiled = workflow.compile();
    for path in &args.repositories {
        let repo = ensure_repository(&forge, path, &config).await?;
        upsert_labels(&forge, &repo, &compiled).await?;
    }
    Ok(())
}

async fn ensure_repository(
    forge: &FilesystemForge,
    path: &RepositoryPath,
    config: &RunnerConfig,
) -> Result<RepositoryId, RunError> {
    if let Some(repo) = forge.get_repository_by_path(path).await? {
        return Ok(repo.id);
    }
    let repo = forge
        .create_repository(CreateRepository {
            owner: path.owner.clone(),
            name: path.name.clone(),
            default_branch: config.repository.default_branch.clone(),
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
    let (workflow, config) = resolve_workflow_and_config(args)?;
    let compiled = workflow.compile();
    let role_id = RoleId::new(role);

    let registry = registry_for(behavior);
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
/// scenario wiring in `temper-runner/tests/end_to_end.rs`, so the same
/// scenarios converge across both topologies.
pub(super) fn registry_for(behavior: RoleBehavior) -> AgentRegistry<dyn Forge> {
    match behavior.profile {
        // basic-delivery binds only architect + engineer (no reviewer/owner/human);
        // its architect/reviewer knobs do not apply, so they are ignored here.
        ProfileKind::Basic => basic_fake_registry(),
        ProfileKind::Reference => match (behavior.architect, behavior.reviewer) {
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
        },
    }
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
    let (workflow, config) = resolve_workflow_and_config(args)?;
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
fn drive<W: temper_runner::Worker>(args: &WorkerArgs, worker: &W) -> Result<RunReport, RunError> {
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
        if let Some(path) = &self.stop_file
            && sentinel_exists(path)
        {
            return true;
        }
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            return true;
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
