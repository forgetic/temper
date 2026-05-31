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

use harness_forge::Forge;
use harness_forge_forgejo::{ForgejoConfig, ForgejoForge};
use harness_runner::{Agent, MechanicalWorker, RoleWorker, RunReport, WorkerRunReport};
use harness_workflow::{InMemoryJournal, LeasePolicy, RoleId};

use crate::worker_bin::args::{AgentsKind, ForgejoArgs, RoleBehavior, WorkerArgs, WorkerKind};
use crate::worker_bin::forgejo_engineer::{ForgejoEngineer, ForgejoLlmPrep};
use crate::worker_bin::run::{
    real_registry_for, registry_for, resolve_repository, upsert_labels, RunError, StopSignal,
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

            // `--agents fake` (default) keeps the deterministic fakes with the
            // Forgejo-aware engineer; `--agents real` swaps in the LLM agents from
            // `harness-agents`, the engineer carrying the Forgejo prep hook.
            let registry = match args.agents {
                AgentsKind::Fake => registry_with_forgejo_engineer(forgejo, args, *behavior),
                AgentsKind::Real => real_registry_with_forgejo_prep(forgejo, args, *behavior)?,
            };
            let agent = registry
                .get(&role_id)
                .ok_or_else(|| RunError::UnknownRole { role: role.clone() })?
                .clone();

            // Identity comes from the token baked into `forge`, not from a
            // handle relabel; `--user` is only a human-readable cross-check.
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
        }
        WorkerKind::Mechanical => {
            let workflow = workflow();
            let config = runner_config();
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
    args: &WorkerArgs,
    behavior: RoleBehavior,
) -> harness_runner::AgentRegistry<dyn Forge> {
    let mut registry = registry_for(behavior);
    let engineer = ForgejoEngineer::new(
        forgejo.base_url.clone(),
        forgejo.token.clone(),
        args.owner.clone(),
        args.name.clone(),
        behavior.ci_sentinel,
    );
    registry.insert(
        RoleId::new(ENGINEER_ROLE),
        Arc::new(engineer) as Arc<dyn Agent<dyn Forge>>,
    );
    registry
}

/// Builds the **real** (LLM) registry for the Forgejo backend, with the engineer
/// carrying a Forgejo [`ForgejoLlmPrep`] hook (real PR head + CI sentinel commit).
///
/// Mirrors [`registry_with_forgejo_engineer`] but for `--agents real`: the
/// architect/reviewer variants and engineer prep come from
/// [`real_registry_for`], so every role is the DeepSeek-backed LLM agent while
/// the engineer keeps the real-PR/real-CI side effects. The DeepSeek key is read
/// at runtime by `real_registry_for`; a missing key fails as a `Backend` setup
/// error before any worker ticks.
fn real_registry_with_forgejo_prep(
    forgejo: &ForgejoArgs,
    args: &WorkerArgs,
    behavior: RoleBehavior,
) -> Result<harness_runner::AgentRegistry<dyn Forge>, RunError> {
    let prep = Arc::new(ForgejoLlmPrep::new(
        forgejo.base_url.clone(),
        forgejo.token.clone(),
        args.owner.clone(),
        args.name.clone(),
        behavior.ci_sentinel,
    )) as Arc<dyn harness_agents::EngineerPrep<dyn Forge>>;
    real_registry_for(behavior, prep)
}

/// Optional repo+labels provisioning step, given a token with admin rights.
///
/// The Phase 4 test driver normally provisions users/tokens/repo before any
/// worker spawns (see the plan's "Provisioning dispatch"), so a
/// `--kind provision --backend forgejo` worker is optional. When used, it is the
/// repo-labels step: it assumes the repository already exists (created by the
/// driver with `auto_init`) and upserts every workflow label through the
/// backend-agnostic Forge interface. It does **not** create users or mint
/// tokens — that needs forgejo-specific admin flows that live in the harness.
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
/// time); ticks are stamped with [`Utc::now`].
async fn drive_async<W: harness_runner::Worker>(
    args: &WorkerArgs,
    worker: &W,
) -> Result<RunReport, RunError> {
    use std::time::Duration as StdDuration;

    let stop = StopSignal::new(args.stop_file.clone(), args.run_secs);
    let interval = args
        .poll_interval
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_millis(50));
    let mut consecutive_failures = 0u32;

    while !stop.should_stop() {
        match worker.tick(chrono::Utc::now()).await {
            Ok(_) => consecutive_failures = 0,
            Err(error) => {
                consecutive_failures += 1;
                // The worker name is safe to log; the error's Display never
                // includes the token (the backend redacts it).
                eprintln!(
                    "harness-testing-worker: worker '{}' tick failed \
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
        tokio::time::sleep(interval).await;
    }

    Ok(RunReport {
        ticks: 0,
        workers: vec![WorkerRunReport {
            name: worker.name().to_string(),
            ticks: 0,
            actions: 0,
        }],
    })
}
