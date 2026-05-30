//! Worker construction and the per-process run loop.
//!
//! Each invocation builds one [`FilesystemForge`] handle over the shared store,
//! resolves the repository by its owner/name path, constructs the matching
//! runner worker, and drives it with a [`PollLoop`] until a stop signal fires.
//! The worker set and labels are derived from the compiled reference workflow
//! and [`RunnerConfig`]; nothing here hardcodes role, queue, or label names.

use chrono::{DateTime, Duration, Utc};
use harness_forge::{Forge, ForgeError, RepositoryId, RepositoryPath, UpsertLabel, User};
use harness_forge_filesystem::FilesystemForge;
use harness_runner::{
    CiWorker, ManualClock, MechanicalWorker, PollLoop, RoleWorker, RunReport, RunnerConfig,
};
use harness_workflow::{CompiledWorkflow, InMemoryJournal, LeasePolicy, RoleId};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::agents::{
    fake_registry_with, ClosingArchitect, FakeArchitect, FakeReviewer,
    RequestChangesThenApproveReviewer,
};
use crate::ci::{FailThenPassCiPolicy, FilesystemCiSink, FixedCiPolicy};
use crate::worker_bin::args::{
    ArchitectKind, CiPolicyKind, ClockKind, ReviewerKind, RoleBehavior, WorkerArgs, WorkerKind,
};
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
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Forge(error) => Some(error),
            RunError::Drive(error) => Some(error.as_ref()),
            RunError::RepositoryMissing { .. } | RunError::UnknownRole { .. } => None,
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
/// `provision` is one-shot; every other kind drives a [`PollLoop`] until its
/// stop signal fires (sentinel file present or `--run-secs` deadline passed).
pub fn run(args: &WorkerArgs) -> Result<RunReport, RunError> {
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
    let repo = ensure_repository(&forge, &args.owner, &args.name).await?;
    upsert_labels(&forge, &repo, &compiled).await?;
    Ok(())
}

async fn ensure_repository(
    forge: &FilesystemForge,
    owner: &str,
    name: &str,
) -> Result<RepositoryId, RunError> {
    let path = RepositoryPath::new(owner, name);
    if let Some(repo) = forge.get_repository_by_path(&path).await? {
        return Ok(repo.id);
    }
    let repo = forge.create_repository(runner_config().repository).await?;
    Ok(repo.id)
}

async fn upsert_labels(
    forge: &FilesystemForge,
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
    let repo = block_on(resolve_repository(&forge, &args.owner, &args.name))?;

    let worker = RoleWorker::new(
        &workflow,
        &compiled,
        &forge,
        &repo,
        role_id.clone(),
        agent,
        config.execution_context(&role_id),
    );
    drive(args, &worker)
}

/// Builds the fake registry whose architect and reviewer match `behavior`.
///
/// `fake_registry_with` is generic over the two variant types, so the runtime
/// choice resolves into one of the four concrete combinations here. This mirrors
/// the in-process scenario wiring in `harness-runner/tests/end_to_end.rs`, so the
/// same scenarios converge across both topologies.
fn registry_for(behavior: RoleBehavior) -> AgentRegistry<FilesystemForge> {
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

fn resolve_role_user(config: &RunnerConfig, role: &RoleId, fallback_handle: &str) -> User {
    config
        .role_binding(role)
        .map(|binding| binding.user.clone())
        .unwrap_or_else(|| crate::actor_user(fallback_handle))
}

fn run_mechanical(args: &WorkerArgs) -> Result<RunReport, RunError> {
    let workflow = workflow();
    let config = runner_config();
    let forge = FilesystemForge::new(&args.root);
    let repo = block_on(resolve_repository(&forge, &args.owner, &args.name))?;

    // The journal is per-process fast-recovery state, not durable coordination:
    // leases live in Forge metadata (ADR 0013/0018), so the mechanical worker
    // re-derives everything it needs from Forge each tick. A fresh in-memory
    // journal per process is therefore correct across a restart.
    let journal = InMemoryJournal::new();
    let worker = MechanicalWorker::new(
        &workflow,
        &forge,
        &repo,
        &journal,
        LeasePolicy::new(config.lease_ttl),
    );
    drive(args, &worker)
}

fn run_ci(args: &WorkerArgs, policy: CiPolicyKind) -> Result<RunReport, RunError> {
    let forge = FilesystemForge::new(&args.root);
    let repo = block_on(resolve_repository(&forge, &args.owner, &args.name))?;
    let sink = FilesystemCiSink::new(forge.clone());
    match policy {
        CiPolicyKind::Pass => {
            let worker = CiWorker::new(&forge, &repo, sink);
            drive(args, &worker)
        }
        CiPolicyKind::FailThenPass => {
            let worker = CiWorker::with_policy(&forge, &repo, sink, FailThenPassCiPolicy);
            drive(args, &worker)
        }
        CiPolicyKind::FixedFail => {
            let worker = CiWorker::with_policy(&forge, &repo, sink, FixedCiPolicy::fail());
            drive(args, &worker)
        }
    }
}

async fn resolve_repository(
    forge: &FilesystemForge,
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

/// Drives `worker` with a poll loop until the stop signal fires.
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

fn epoch() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("Unix epoch is valid")
}

/// Stop predicate: fire when the sentinel file exists or the deadline passes.
struct StopSignal {
    stop_file: Option<PathBuf>,
    deadline: Option<Instant>,
}

impl StopSignal {
    fn new(stop_file: Option<PathBuf>, run_secs: Option<u64>) -> Self {
        Self {
            stop_file,
            deadline: run_secs.map(|secs| Instant::now() + std::time::Duration::from_secs(secs)),
        }
    }

    fn should_stop(&self) -> bool {
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
mod tests {
    use super::*;
    use crate::worker_bin::args::WorkerArgs;
    use harness_runner::Worker;

    fn temp_root(suite: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "harness-testing-worker-{suite}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn base_args(root: PathBuf, kind: WorkerKind) -> WorkerArgs {
        WorkerArgs {
            kind,
            root,
            owner: "acme".into(),
            name: "service".into(),
            poll_interval: Duration::milliseconds(1),
            stop_file: None,
            run_secs: Some(0),
            clock: ClockKind::Deterministic,
        }
    }

    #[test]
    fn each_kind_wires_up_against_a_temp_root() {
        let root = temp_root("wireup");

        // Provision first so the repository and labels exist for the others.
        run(&base_args(root.clone(), WorkerKind::Provision)).expect("provision succeeds");

        // No-sleep bounded ticks per kind prove the construction path wires up
        // against the shared store without spawning processes or sleeping.
        let workflow = workflow();
        let compiled = workflow.compile();
        let config = runner_config();

        // Role worker for every role that has a fake agent.
        let registry = registry_for(RoleBehavior::default());
        for role in compiled.roles() {
            if registry.get(&role.id).is_none() {
                continue;
            }
            let user = resolve_role_user(&config, &role.id, role.id.as_str());
            let forge = FilesystemForge::new(&root).as_user(user);
            let repo = block_on(resolve_repository(&forge, "acme", "service")).expect("repo");
            let agent = registry.get(&role.id).expect("agent").clone();
            let worker = RoleWorker::new(
                &workflow,
                &compiled,
                &forge,
                &repo,
                role.id.clone(),
                agent,
                config.execution_context(&role.id),
            );
            let clock = ManualClock::with_tick_step(epoch(), Duration::seconds(1));
            let poll = PollLoop::with_clock(&worker, Duration::milliseconds(1), clock);
            block_on(poll.run_bounded(2)).expect("role worker ticks");
        }

        // Mechanical worker.
        {
            let forge = FilesystemForge::new(&root);
            let repo = block_on(resolve_repository(&forge, "acme", "service")).expect("repo");
            let journal = InMemoryJournal::new();
            let worker = MechanicalWorker::new(
                &workflow,
                &forge,
                &repo,
                &journal,
                LeasePolicy::new(config.lease_ttl),
            );
            assert_eq!(worker.name(), "mechanical");
            let clock = ManualClock::with_tick_step(epoch(), Duration::seconds(1));
            let poll = PollLoop::with_clock(&worker, Duration::milliseconds(1), clock);
            block_on(poll.run_bounded(2)).expect("mechanical worker ticks");
        }

        // CI worker, each policy.
        for policy in [
            CiPolicyKind::Pass,
            CiPolicyKind::FailThenPass,
            CiPolicyKind::FixedFail,
        ] {
            let report =
                run(&base_args(root.clone(), WorkerKind::Ci { policy })).expect("ci worker runs");
            assert_eq!(report.workers.len(), 1);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn run_role_reports_unknown_role() {
        let root = temp_root("unknown-role");
        run(&base_args(root.clone(), WorkerKind::Provision)).expect("provision");
        let error = run(&base_args(
            root.clone(),
            WorkerKind::Role {
                role: "ghost".into(),
                user: "ghost".into(),
                behavior: RoleBehavior::default(),
            },
        ))
        .unwrap_err();
        assert!(matches!(error, RunError::UnknownRole { .. }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_repository_reports_missing() {
        let root = temp_root("missing-repo");
        let forge = FilesystemForge::new(&root);
        let error = block_on(resolve_repository(&forge, "acme", "service")).unwrap_err();
        assert!(matches!(error, RunError::RepositoryMissing { .. }));
        let _ = std::fs::remove_dir_all(&root);
    }
}
