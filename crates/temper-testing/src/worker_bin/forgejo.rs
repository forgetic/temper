//! Forgejo backend handle construction and the async run loop.
//!
//! This is the `--backend forgejo` half of the worker backend seam (Phase 3).
//! It builds a [`ForgejoForge`] handle and drives the same runner workers as the
//! filesystem path — everything above `dyn Forge` is shared with
//! [`super::run`]. Two things differ and live only here:
//!
//! 1. **Identity is the token, not `as_user`.** Forgejo has no per-handle
//!    relabel; `current_user` is whatever the per-role access token maps to. Each
//!    role worker is given its own token (via env; see
//!    [`ForgejoArgs`](crate::worker_bin::args::ForgejoArgs)) and so its own
//!    handle.
//! 2. **The futures park on real network IO**, so the crate's no-op
//!    [`block_on`](crate::block_on) cannot drive them. We run them on a
//!    current-thread Tokio runtime instead.
//!
//! CI is produced by the real `forgejo-runner` and read via the Phase 3b web-UI
//! path, so there is no fake `--kind ci` worker here; `--kind ci` is rejected
//! for this backend at parse time. The web-UI username/password (when present)
//! are carried in [`ForgejoArgs`] for that read path; this module only needs the
//! token to construct the handle.

use std::sync::Arc;

use temper_forge::Forge;
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_runner::{
    Agent, MechanicalWorker, MultiRepoMechanicalWorker, MultiRepoRoleWorker, RepositoryJournal,
    RoleWorker, RunReport, WorkerRunReport,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

use crate::worker_bin::args::{ForgejoArgs, RoleBehavior, WorkerArgs, WorkerKind};
use crate::worker_bin::forgejo_engineer::ForgejoEngineer;
use crate::worker_bin::run::{
    registry_for, resolve_repository, resolve_repository_set, upsert_labels, RunError, StopSignal,
};
use crate::{runner_config, workflow};

/// Workflow role id of the engineer, the one role whose Forgejo behavior differs
/// from its filesystem fake (it must prep PR heads and push CI fix commits).
const ENGINEER_ROLE: &str = "engineer";

/// Runs a `--backend forgejo` worker to completion on a Tokio runtime.
///
/// Note `--kind ci` never reaches here: it is rejected for the Forgejo backend
/// during argument parsing (the real `forgejo-runner` is the CI producer).
pub(super) fn run(args: &WorkerArgs, forgejo: &ForgejoArgs) -> Result<RunReport, RunError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| RunError::Backend(format!("failed to start Tokio runtime: {error}")))?;
    runtime.block_on(run_async(args, forgejo))
}

async fn run_async(args: &WorkerArgs, forgejo: &ForgejoArgs) -> Result<RunReport, RunError> {
    let forge = build_forge(forgejo);
    match &args.kind {
        WorkerKind::Provision => {
            provision(&forge, args).await?;
            Ok(RunReport::default())
        }
        WorkerKind::Role {
            role,
            user: _,
            behavior,
        } => {
            let workflow = workflow();
            let compiled = workflow.compile();
            let config = runner_config();
            let role_id = RoleId::new(role);

            let registry = registry_with_forgejo_engineer(forgejo, *behavior);
            let agent = registry
                .get(&role_id)
                .ok_or_else(|| RunError::UnknownRole { role: role.clone() })?
                .clone();

            // Identity comes from the token baked into `forge`, not from a
            // handle relabel; `--user` is only a human-readable cross-check.
            if args.repositories.len() == 1 {
                let repo = resolve_repository(&forge, &args.owner, &args.name).await?;
                let worker = RoleWorker::new(
                    &workflow,
                    &compiled,
                    &forge as &dyn Forge,
                    &repo,
                    role_id.clone(),
                    agent,
                    config.execution_context(&role_id),
                );
                drive_async(args, &worker).await
            } else {
                let repositories = resolve_repository_set(&forge, &args.repositories).await?;
                let worker = MultiRepoRoleWorker::new(
                    &workflow,
                    &compiled,
                    &forge as &dyn Forge,
                    repositories,
                    role_id.clone(),
                    agent,
                    config.execution_context(&role_id),
                );
                drive_async(args, &worker).await
            }
        }
        WorkerKind::Mechanical => {
            let workflow = workflow();
            let config = runner_config();
            if args.repositories.len() == 1 {
                let repo = resolve_repository(&forge, &args.owner, &args.name).await?;
                let journal = InMemoryJournal::new();
                let worker = MechanicalWorker::new(
                    &workflow,
                    &forge as &dyn Forge,
                    &repo,
                    &journal,
                    LeasePolicy::new(config.lease_ttl),
                );
                drive_async(args, &worker).await
            } else {
                let repositories = resolve_repository_set(&forge, &args.repositories).await?;
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
                drive_async(args, &worker).await
            }
        }
        // Unreachable: parsing rejects `--kind ci` for the Forgejo backend.
        WorkerKind::Ci { .. } => Err(RunError::Backend(
            "--kind ci is not supported with --backend forgejo".to_string(),
        )),
    }
}

/// Builds a [`ForgejoForge`] from the connection details and per-role token.
///
/// When the worker was given web-UI credentials (the optional
/// [`FORGEJO_USERNAME_ENV`](crate::worker_bin::args::FORGEJO_USERNAME_ENV) /
/// [`FORGEJO_PASSWORD_ENV`](crate::worker_bin::args::FORGEJO_PASSWORD_ENV)), they
/// are wired into the config so the Phase 3b CI read path can fall back to the
/// password/web-UI live-view JSON when Forgejo 7.0.x does not serve `actions/runs`
/// over REST (ADR 0019). Without them, `list_ci_jobs` is REST-only and hard-errors
/// on that version — so any role that observes a CI gate must be given them.
///
/// The token and password never appear in logs or errors; only the base URL is
/// echoed.
fn build_forge(forgejo: &ForgejoArgs) -> ForgejoForge {
    let mut config = ForgejoConfig::new(forgejo.base_url.clone(), forgejo.token.clone());
    if let (Some(username), Some(password)) = (&forgejo.username, &forgejo.password) {
        config = config.with_web_ui_credentials(username, password);
    }
    ForgejoForge::new(config)
}

/// Builds the shared fake registry, then swaps the **engineer** entry for the
/// Forgejo-aware [`ForgejoEngineer`].
///
/// Every other role reuses its `--architect`/`--reviewer`-selected fake from
/// [`registry_for`] unchanged; only the engineer needs the connection details +
/// token + CI-sentinel policy that the backend-neutral fake lacks (real PR heads
/// and real CI fix commits, see [`ForgejoEngineer`]). The override happens
/// regardless of `role_id` so the registry is consistent; the worker then selects
/// the one agent it runs.
fn registry_with_forgejo_engineer(
    forgejo: &ForgejoArgs,
    behavior: RoleBehavior,
) -> temper_runner::AgentRegistry<dyn Forge> {
    let mut registry = registry_for(behavior);
    let engineer = ForgejoEngineer::new(
        forgejo.base_url.clone(),
        forgejo.token.clone(),
        behavior.ci_sentinel,
    );
    registry.insert(
        RoleId::new(ENGINEER_ROLE),
        Arc::new(engineer) as Arc<dyn Agent<dyn Forge>>,
    );
    registry
}

/// Optional repo+labels provisioning step, given a token with admin rights.
///
/// The Phase 4 test driver normally provisions users/tokens/repo before any
/// worker spawns (see the plan's "Provisioning dispatch"), so a
/// `--kind provision --backend forgejo` worker is optional. When used, it is the
/// repo-labels step: it assumes the repository already exists (created by the
/// driver with `auto_init`) and upserts every workflow label through the
/// backend-agnostic Forge interface. It does **not** create users or mint
/// tokens — that needs forgejo-specific admin flows that live in Temper.
async fn provision(forge: &ForgejoForge, args: &WorkerArgs) -> Result<(), RunError> {
    let workflow = workflow();
    let compiled = workflow.compile();
    let repo = resolve_repository(forge, &args.owner, &args.name).await?;
    upsert_labels(forge, &repo, &compiled).await?;
    Ok(())
}

/// How many consecutive failing ticks abort a Forgejo worker. A real server
/// under concurrent multi-process load returns transient `5xx`/conflict errors
/// (SQLite contention, racing edits to the same artifact); a level-triggered
/// poll worker must survive those and retry, exactly as production would. A long
/// run of failures, though, means a genuine misconfiguration (bad token, wrong
/// repo) — abort then so the test fails loudly instead of spinning to timeout.
const MAX_CONSECUTIVE_TICK_FAILURES: u32 = 50;

/// Drives `worker` with a **resilient** wall-clock poll loop on the current Tokio
/// runtime.
///
/// Unlike [`super::run`]'s filesystem `drive` (which uses [`PollLoop`] and aborts
/// on the first tick error, correct for the deterministic backend that never
/// errors transiently), this loop tolerates transient per-tick failures: a real
/// Forgejo under concurrent worker load intermittently returns `5xx`/conflict,
/// and a single such blip must not permanently kill a worker (which would starve
/// the whole topology — e.g. one transient `500` on the engineer's `claim_code`
/// would mean no PR is ever opened). Failing ticks are logged to stderr (never
/// with a token) and retried next interval; only [`MAX_CONSECUTIVE_TICK_FAILURES`]
/// in a row aborts.
///
/// The Forgejo backend writes wall-clock timestamps, so the deterministic
/// `ManualClock` epoch seam does not apply (`--clock wall` is enforced at parse
/// time); ticks are stamped with [`Utc::now`]. If `--wake-socket` is present, the
/// same tick path is also resumed by authenticated local webhook wakes.
async fn drive_async<W: temper_runner::Worker>(
    args: &WorkerArgs,
    worker: &W,
) -> Result<RunReport, RunError> {
    use std::time::Duration as StdDuration;
    use temper_production::wake::{
        wait_for_wake_or_poll, WakeConfig, WakeListener, WakeWaitOutcome,
    };

    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(50));
    let wake = match args.wake_socket.clone() {
        Some(socket) => Some(
            WakeListener::bind(
                WakeConfig::from_files(socket, args.wake_secret_file.clone())
                    .map_err(|error| RunError::Backend(error.to_string()))?,
            )
            .map_err(|error| RunError::Backend(error.to_string()))?,
        ),
        None => None,
    };
    let mut consecutive_failures = 0u32;
    let mut report = RunReport {
        ticks: 0,
        workers: vec![WorkerRunReport {
            name: worker.name().to_string(),
            ticks: 0,
            actions: 0,
        }],
    };

    while !stop.should_stop() {
        match worker.tick(chrono::Utc::now()).await {
            Ok(progress) => {
                consecutive_failures = 0;
                report.ticks = report.ticks.saturating_add(1);
                report.workers[0].ticks = report.workers[0].ticks.saturating_add(1);
                report.workers[0].actions = report.workers[0]
                    .actions
                    .saturating_add(u64::from(progress.actions));
                eprintln!(
                    "temper-testing-worker: worker '{}' completed tick actions={}",
                    worker.name(),
                    progress.actions
                );
            }
            Err(error) => {
                consecutive_failures += 1;
                // The worker name is safe to log; the error's Display never
                // includes the token (the backend redacts it).
                eprintln!(
                    "temper-testing-worker: worker '{}' tick failed \
                     ({consecutive_failures}/{MAX_CONSECUTIVE_TICK_FAILURES}), retrying: {error}",
                    worker.name()
                );
                if consecutive_failures >= MAX_CONSECUTIVE_TICK_FAILURES {
                    return Err(RunError::Drive(Box::new(error)));
                }
            }
        }
        if stop.should_stop() {
            break;
        }
        match wait_for_wake_or_poll(|| stop.should_stop(), interval, wake.as_ref())
            .await
            .map_err(|error| RunError::Backend(error.to_string()))?
        {
            WakeWaitOutcome::PollDeadline => {}
            WakeWaitOutcome::Stop => break,
            WakeWaitOutcome::Wake(hints) => {
                let wake_count = hints.len();
                eprintln!(
                    "temper-testing-worker: worker '{}' consumed authenticated wake batch hints={wake_count}; ticking immediately",
                    worker.name()
                );
            }
        }
    }

    Ok(report)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{DateTime, Duration, Utc};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::Duration as StdDuration;
    use temper_forge::RepositoryPath;
    use temper_production::wake::send_wake;
    use temper_runner::{Progress, Worker, WorkerError};

    use crate::worker_bin::args::{AgentsKind, Backend, ClockKind};

    struct BurstWorker {
        ticks: AtomicU64,
        socket: PathBuf,
    }

    #[async_trait]
    impl Worker for BurstWorker {
        async fn tick(&self, _now: DateTime<Utc>) -> Result<Progress, WorkerError> {
            let tick = self.ticks.fetch_add(1, Ordering::SeqCst) + 1;
            if tick == 1 {
                for _ in 0..3 {
                    send_wake(&self.socket, Some("wake-secret")).expect("wake sends");
                }
            }
            Ok(Progress::unchanged())
        }

        fn name(&self) -> &str {
            "burst-worker"
        }
    }

    #[test]
    fn forgejo_drive_coalesces_queued_wake_bursts() {
        let root = temp_root("coalesced-wakes");
        std::fs::create_dir_all(&root).expect("temp root exists");
        let socket = root.join("worker.sock");
        let secret_file = root.join("wake-secret");
        let stop_file = root.join("stop");
        std::fs::write(&secret_file, "wake-secret\n").expect("secret writes");
        let args = WorkerArgs {
            kind: WorkerKind::Mechanical,
            backend: Backend::Forgejo(ForgejoArgs {
                base_url: "http://127.0.0.1:1".into(),
                token: "token".into(),
                username: None,
                password: None,
            }),
            root: root.clone(),
            owner: "acme".into(),
            name: "service".into(),
            repositories: vec![RepositoryPath::new("acme", "service")],
            poll_interval: Duration::seconds(60),
            stop_file: Some(stop_file.clone()),
            run_secs: None,
            clock: ClockKind::Wall,
            agents: AgentsKind::Fake,
            wake_socket: Some(socket.clone()),
            wake_secret_file: Some(secret_file),
        };
        let worker = BurstWorker {
            ticks: AtomicU64::new(0),
            socket,
        };
        let stop_file_for_thread = stop_file.clone();
        let stopper = thread::spawn(move || {
            thread::sleep(StdDuration::from_millis(800));
            std::fs::write(stop_file_for_thread, b"stop").expect("stop file writes");
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime builds");

        let report = runtime
            .block_on(drive_async(&args, &worker))
            .expect("drive succeeds");
        stopper.join().expect("stopper joins");

        assert_eq!(worker.ticks.load(Ordering::SeqCst), 2);
        assert_eq!(report.ticks, 2);
        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "temper-testing-forgejo-{name}-{}-{}",
            std::process::id(),
            Utc::now()
                .timestamp_nanos_opt()
                .expect("timestamp has nanoseconds")
        ))
    }
}
