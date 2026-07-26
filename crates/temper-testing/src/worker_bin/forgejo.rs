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
//! CI is produced by the real `forgejo-runner` and read through Forgejo's
//! token-authenticated Actions APIs, so there is no fake `--kind ci` worker here;
//! `--kind ci` is rejected for this backend at parse time.

use std::sync::Arc;

use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_forge_model::Forge;
use temper_runner::{
    Agent, MechanicalWorker, MultiRepoMechanicalWorker, MultiRepoRoleWorker, RepositoryJournal,
    RoleWorker, RunReport,
};
use temper_workflow::{InMemoryJournal, LeasePolicy, RoleId};

use crate::worker_bin::args::{ForgejoArgs, ProfileKind, RoleBehavior, WorkerArgs, WorkerKind};
use crate::worker_bin::forgejo_drive::drive_async;
use crate::worker_bin::forgejo_engineer::{ForgejoBasicEngineer, ForgejoEngineer};
use crate::worker_bin::run::{
    RunError, registry_for, resolve_repository, resolve_repository_set,
    resolve_workflow_and_config, upsert_labels,
};

/// Workflow role id of the engineer, the one role whose Forgejo behavior differs
/// from its filesystem fake (it must prep PR heads and push CI fix commits).
const ENGINEER_ROLE: &str = "engineer";

/// Runs a `--backend forgejo` worker to completion on the engine runtime.
///
/// Note `--kind ci` never reaches here: it is rejected for the Forgejo backend
/// during argument parsing (the real `forgejo-runner` is the CI producer).
pub(super) fn run(args: &WorkerArgs, forgejo: &ForgejoArgs) -> Result<RunReport, RunError> {
    let args = args.clone();
    let forgejo = forgejo.clone();
    temper_engine_io::block_on_with(move |cx, _handle| async move {
        run_async(&cx, &args, &forgejo).await
    })
}

async fn run_async(
    cx: &temper_engine_io::Cx,
    args: &WorkerArgs,
    forgejo: &ForgejoArgs,
) -> Result<RunReport, RunError> {
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
            let (workflow, config) = resolve_workflow_and_config(args)?;
            let compiled = workflow.compile();
            let role_id = RoleId::new(role);

            let registry = registry_with_forgejo_engineer(cx, forgejo, *behavior);
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
                drive_async(cx, args, &worker).await
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
                drive_async(cx, args, &worker).await
            }
        }
        WorkerKind::Mechanical => {
            let (workflow, config) = resolve_workflow_and_config(args)?;
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
                drive_async(cx, args, &worker).await
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
                drive_async(cx, args, &worker).await
            }
        }
        // Unreachable: parsing rejects `--kind ci` for the Forgejo backend.
        WorkerKind::Ci { .. } => Err(RunError::Backend(
            "--kind ci is not supported with --backend forgejo".to_string(),
        )),
    }
}

/// Builds a token-authenticated [`ForgejoForge`] for one role.
///
/// The token never appears in logs or errors; only the base URL is echoed.
fn build_forge(forgejo: &ForgejoArgs) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(
        forgejo.base_url.clone(),
        forgejo.token.clone(),
    ))
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
    cx: &temper_engine_io::Cx,
    forgejo: &ForgejoArgs,
    behavior: RoleBehavior,
) -> temper_runner::AgentRegistry<dyn Forge> {
    let mut registry = registry_for(behavior);
    // The engineer transition differs by profile (reference `claim_code` +
    // explicit open + `request_review` vs basic single `open_pr`), so the Forgejo
    // engineer wrapping the matching state machine differs too; the prep
    // side-effects (real PR head + CI sentinel) are identical.
    let engineer: Arc<dyn Agent<dyn Forge>> = match behavior.profile {
        ProfileKind::Reference => Arc::new(ForgejoEngineer::new(
            cx.clone(),
            forgejo.base_url.clone(),
            forgejo.token.clone(),
            behavior.ci_sentinel,
        )),
        ProfileKind::Basic => Arc::new(ForgejoBasicEngineer::new(
            cx.clone(),
            forgejo.base_url.clone(),
            forgejo.token.clone(),
            behavior.ci_sentinel,
        )),
    };
    registry.insert(RoleId::new(ENGINEER_ROLE), engineer);
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
    let (workflow, _config) = resolve_workflow_and_config(args)?;
    let compiled = workflow.compile();
    let repo = resolve_repository(forge, &args.owner, &args.name).await?;
    upsert_labels(forge, &repo, &compiled).await?;
    Ok(())
}
