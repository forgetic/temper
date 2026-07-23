// SPDX-License-Identifier: MPL-2.0

//! Wake execution and optional webhook intake wiring for [`Daemon`].
//!
//! The daemon coordinator admits [`WakeWork`](super::wake_coordinator::WakeWork)
//! before this module performs any Forge read. Routes retain both stable
//! repository ids and human-facing paths, so delivery handling never performs
//! repository-by-path resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use temper_forge::{ChangeKind, Forge, HintArtifactKind, Repository, RepositoryId, RepositoryPath};
use temper_log::WorkItemRef;
use temper_protocol_worker::Artifact;
use temper_runner::ArtifactAddress;
use temper_workflow::{CiState, CiStatus, CompiledWorkflow, RoleId, ValidatedWorkflow};

mod missing_ci_recovery;

use self::missing_ci_recovery::recover_missing_current_head_ci;

use crate::RoleFeedTarget;
use crate::lease_applier::WallClock;
use crate::webhook::WebhookConfig;

use super::machine::DaemonCompletion;
use super::wake_coordinator::{
    CiWakeFacts, WakeLane, WakeOutcome, WakeScope, WakeTargets, WakeWork, merge_change_kind,
    prioritized_targets,
};
#[cfg(test)]
use super::wake_scope::WakeTarget;
use super::{CoordinatedMechanical, Daemon, WakeExecutor};

impl Daemon {
    /// Enables verified `POST /forgejo/webhook` intake and installs coordinated
    /// role wake execution.
    pub fn with_webhook<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        config: Arc<WebhookConfig>,
        clock: WallClock,
    ) -> Self {
        self.with_webhook_and_mechanical(forge, workflow, compiled, config, clock, None)
    }

    /// Enables only the verified HTTP webhook route, retaining an already
    /// installed wake executor. Production startup uses this after configuring
    /// poll/startup/mechanical execution independently of webhook availability.
    pub fn with_webhook_config(self, config: Arc<WebhookConfig>) -> Self {
        let _ = self.cq.send(DaemonCompletion::ConfigureWebhook {
            config: (*config).clone(),
        });
        self
    }

    /// Enables webhook intake and coordinated role/mechanical execution.
    pub fn with_webhook_and_mechanical<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        config: Arc<WebhookConfig>,
        clock: WallClock,
        mechanical: Option<Arc<dyn CoordinatedMechanical>>,
    ) -> Self {
        let daemon = self.with_wake_execution(
            forge,
            workflow,
            compiled,
            config.targets.clone(),
            clock,
            mechanical,
        );
        daemon.with_webhook_config(config)
    }

    /// Installs coordinated wake execution without enabling an HTTP webhook.
    /// This is used by poll/startup/change-source-only deployments too.
    pub fn with_wake_execution<F: Forge + Send + Sync + ?Sized + 'static>(
        self,
        forge: Arc<F>,
        workflow: Arc<ValidatedWorkflow>,
        compiled: Arc<CompiledWorkflow>,
        wake_targets: Vec<RoleFeedTarget>,
        clock: WallClock,
        mechanical: Option<Arc<dyn CoordinatedMechanical>>,
    ) -> Self {
        let has_mechanical = mechanical.is_some();
        let configuration = wake_repositories(&wake_targets, has_mechanical);
        let routes = configuration
            .repositories
            .iter()
            .map(|(path, id, lanes)| {
                let roles = lanes
                    .iter()
                    .filter_map(|lane| match lane {
                        WakeLane::Role(role) => Some(role.clone()),
                        WakeLane::Mechanical => None,
                    })
                    .collect();
                (
                    repository_key(path),
                    WakeRoute {
                        path: path.clone(),
                        id: id.clone(),
                        roles,
                    },
                )
            })
            .collect();
        let executor = Arc::new(ForgeWakeExecutor {
            daemon: self.wake_execution_handle(),
            forge,
            workflow,
            compiled,
            routes,
            repositories: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
            clock,
            mechanical,
        });
        *self.wake_executor_slot.lock().expect("wake executor slot") = Some(executor);
        let _ = self.cq.send(DaemonCompletion::ConfigureWakeRepositories {
            repositories: configuration
                .repositories
                .into_iter()
                .map(|(path, _id, lanes)| (path, lanes))
                .collect(),
            unresolved_lanes: configuration.unresolved_lanes,
            configured_repository_limit: configuration.configured_repository_limit,
        });
        self
    }

    fn wake_execution_handle(&self) -> Self {
        Self {
            cq: self.cq.clone(),
            wake_executor_slot: Arc::new(std::sync::Mutex::new(None)),
            context_reader_slot: Arc::clone(&self.context_reader_slot),
            trace_query_slot: Arc::clone(&self.trace_query_slot),
            trace_journal_slot: Arc::clone(&self.trace_journal_slot),
            change_source_listeners: Arc::new(std::sync::Mutex::new(Vec::new())),
            artifact_catalog: Arc::clone(&self.artifact_catalog),
            artifact_context: self.artifact_context.clone(),
        }
    }
}

struct WakeRepositoryConfiguration {
    repositories: Vec<(RepositoryPath, RepositoryId, BTreeSet<WakeLane>)>,
    unresolved_lanes: BTreeSet<WakeLane>,
    configured_repository_limit: usize,
}

fn wake_repositories(
    targets: &[RoleFeedTarget],
    has_mechanical: bool,
) -> WakeRepositoryConfiguration {
    let mut repositories: BTreeMap<String, (RepositoryPath, RepositoryId, BTreeSet<WakeLane>)> =
        BTreeMap::new();
    let mut unresolved_lanes = BTreeSet::new();
    if has_mechanical {
        unresolved_lanes.insert(WakeLane::Mechanical);
    }
    let configured_repository_limit = targets
        .iter()
        .map(|target| target.repo.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    for target in targets {
        unresolved_lanes.insert(WakeLane::Role(target.role.clone()));
        let path = format!("{}/{}", target.path.owner, target.path.name);
        let entry = repositories.entry(path).or_insert_with(|| {
            (target.path.clone(), target.repo.clone(), {
                let mut lanes = BTreeSet::new();
                if has_mechanical {
                    lanes.insert(WakeLane::Mechanical);
                }
                lanes
            })
        });
        entry.2.insert(WakeLane::Role(target.role.clone()));
    }
    WakeRepositoryConfiguration {
        repositories: repositories.into_values().collect(),
        unresolved_lanes,
        configured_repository_limit,
    }
}

#[derive(Clone)]
struct WakeRoute {
    path: RepositoryPath,
    id: RepositoryId,
    roles: BTreeSet<RoleId>,
}

struct ForgeWakeExecutor<F: Forge + Send + Sync + ?Sized + 'static> {
    daemon: Daemon,
    forge: Arc<F>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: Arc<CompiledWorkflow>,
    routes: BTreeMap<String, WakeRoute>,
    /// Repository representations are loaded by stable id at most once per
    /// configured route, then reused by targeted and broad enrichment.
    repositories: Arc<std::sync::Mutex<BTreeMap<String, Repository>>>,
    clock: WallClock,
    mechanical: Option<Arc<dyn CoordinatedMechanical>>,
}

impl<F: Forge + Send + Sync + ?Sized + 'static> ForgeWakeExecutor<F> {
    async fn repository(&self, route: &WakeRoute) -> Result<Repository, String> {
        let key = repository_key(&route.path);
        if let Some(repository) = self
            .repositories
            .lock()
            .expect("wake repository cache")
            .get(&key)
            .cloned()
        {
            return Ok(repository);
        }
        let repository = self
            .forge
            .get_repository(&route.id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("configured wake repository {} was not found", route.id))?;
        self.repositories
            .lock()
            .expect("wake repository cache")
            .insert(key, repository.clone());
        Ok(repository)
    }

    async fn execute(&self, work: WakeWork) -> WakeOutcome {
        let Some(route) = self.routes.get(&repository_key(&work.repo)).cloned() else {
            return WakeOutcome::Failed {
                reason: format!(
                    "wake work has no configured route for {}/{}",
                    work.repo.owner, work.repo.name
                ),
            };
        };
        let repository = match self.repository(&route).await {
            Ok(repository) => repository,
            Err(reason) => return WakeOutcome::Failed { reason },
        };

        let mut broad_roles = BTreeSet::new();
        let mut targeted_roles: BTreeMap<ArtifactAddress, (ChangeKind, BTreeSet<RoleId>)> =
            BTreeMap::new();
        let mut mechanical_broad = false;
        let mut mechanical_targets = WakeTargets::new();
        let mut ci_facts = BTreeMap::<ArtifactAddress, CiWakeFacts>::new();

        // Trigger metadata is batch-level observability state. Collect it before
        // broad lanes subsume exact execution so provenance survives every
        // deterministic coordinator compaction path.
        for scope in work.batch.lanes().values() {
            for ((kind, number), target) in scope.targets() {
                let Some(incoming) = target.ci.clone() else {
                    continue;
                };
                ci_facts
                    .entry(ArtifactAddress::new(*kind, *number))
                    .and_modify(|current| *current = current.clone().merge(incoming.clone()))
                    .or_insert(incoming);
            }
        }

        for (lane, scope) in work.batch.lanes() {
            match lane {
                WakeLane::Role(role) if route.roles.contains(role) => match scope {
                    WakeScope::Broad { .. } => {
                        broad_roles.insert(role.clone());
                    }
                    WakeScope::Targeted(targets) => {
                        for ((kind, number), target) in targets {
                            let entry = targeted_roles
                                .entry(ArtifactAddress::new(*kind, *number))
                                .or_insert_with(|| (target.change, BTreeSet::new()));
                            entry.0 = merge_change_kind(entry.0, target.change);
                            entry.1.insert(role.clone());
                        }
                    }
                },
                WakeLane::Mechanical => {
                    let targets = match scope {
                        WakeScope::Broad { targets, .. } => {
                            mechanical_broad = true;
                            targets
                        }
                        WakeScope::Targeted(targets) => targets,
                    };
                    for (address, target) in targets {
                        mechanical_targets
                            .entry(*address)
                            .and_modify(|current| current.merge(target.clone()))
                            .or_insert_with(|| target.clone());
                    }
                }
                WakeLane::Role(_) => {}
            }
        }

        let mut failures = Vec::new();
        let now = (self.clock)();

        // A missing-CI wake is only an intent. Revalidate and, when still safe,
        // install the attention barrier before mechanical or role work can act
        // on this exact target in the coalesced repository generation.
        for (address, facts) in &ci_facts {
            if let Some(recovery) = facts.recovery.as_ref() {
                recover_missing_current_head_ci(
                    self.forge.as_ref(),
                    &repository,
                    self.workflow.as_ref(),
                    self.compiled.as_ref(),
                    now,
                    *address,
                    recovery,
                )
                .await;
            }
        }

        // Exact mechanical transitions run in explicit priority order before
        // retained broad reconciliation. Role work starts only after all
        // mechanical mutation has completed for this repository generation.
        let mechanical_changed = execute_mechanical_work(
            self.mechanical.as_ref(),
            &route.path,
            &mechanical_targets,
            mechanical_broad,
            &mut failures,
        )
        .await;

        // A mechanical transition is itself a fresh change hint. Do not rely
        // solely on Forgejo delivering the resulting label/state webhook: wake
        // all subscribed roles once after a mutating pass so work created by
        // automation is visible immediately, while unchanged idle passes stay
        // request-neutral for role scans.
        if mechanical_changed {
            broad_roles.extend(route.roles.iter().cloned());
        }

        // Broad work subsumes targeted work only for the same lane, including
        // a broad role follow-up introduced by a local mechanical mutation.
        for (_, roles) in targeted_roles.values_mut() {
            roles.retain(|role| !broad_roles.contains(role));
        }
        targeted_roles.retain(|_, (_, roles)| !roles.is_empty());

        let mut broad_ci_selected = BTreeMap::new();
        if !broad_roles.is_empty() {
            let roles = broad_roles.into_iter().collect::<Vec<_>>();
            match crate::feed::enqueue_scanned_roles_wake(
                &self.daemon,
                self.forge.as_ref(),
                &repository,
                self.workflow.as_ref(),
                self.compiled.as_ref(),
                now,
                &roles,
            )
            .await
            {
                Ok(result) => {
                    for (address, queue, role) in result.enqueued_work {
                        broad_ci_selected.entry(address).or_insert((queue, role));
                    }
                }
                Err(error) => failures.push(format!("broad role wake failed: {error}")),
            }
        }

        let mut observed_ci_targets = BTreeSet::new();
        for (address, (_change, roles)) in targeted_roles {
            let roles = roles.into_iter().collect::<Vec<_>>();
            match self
                .daemon
                .enqueue_targeted_role_work(
                    self.forge.as_ref(),
                    &repository,
                    self.workflow.as_ref(),
                    self.compiled.as_ref(),
                    now,
                    address,
                    &roles,
                )
                .await
            {
                Ok(result) => {
                    if let Some(facts) = ci_facts.get(&address).cloned() {
                        emit_ci_wake_observation(
                            &route.path,
                            address,
                            facts,
                            result.ci_status,
                            result.enqueued_work.first(),
                            now,
                        );
                        observed_ci_targets.insert(address);
                    }
                    let artifact = protocol_artifact(address);
                    let repo_label = format!("{}/{}", route.path.owner, route.path.name);
                    for role in roles {
                        self.daemon
                            .reconcile_pending_targeted_role_jobs(
                                &repo_label,
                                role.as_str(),
                                artifact.clone(),
                                result
                                    .current_job_ids
                                    .get(&role)
                                    .cloned()
                                    .unwrap_or_default(),
                            )
                            .await;
                    }
                }
                Err(error) => failures.push(format!(
                    "targeted role wake failed for {}#{}: {error}",
                    artifact_kind(address.kind),
                    address.number
                )),
            }
        }

        for (address, facts) in ci_facts {
            if !observed_ci_targets.contains(&address) {
                emit_ci_wake_observation(
                    &route.path,
                    address,
                    facts,
                    None,
                    broad_ci_selected.get(&address),
                    now,
                );
            }
        }

        if failures.is_empty() {
            WakeOutcome::Succeeded
        } else {
            WakeOutcome::Failed {
                reason: failures.join("; "),
            }
        }
    }
}

fn emit_ci_wake_observation(
    repo: &RepositoryPath,
    address: ArtifactAddress,
    facts: CiWakeFacts,
    fresh_status: Option<CiStatus>,
    selected: Option<&(temper_workflow::QueueId, RoleId)>,
    detected_at: chrono::DateTime<chrono::Utc>,
) {
    if address.kind != HintArtifactKind::PullRequest {
        return;
    }
    let fresh_verdict = fresh_status.and_then(|status| match status.state() {
        CiState::Passed => Some(crate::CiTerminalVerdict::Passed),
        CiState::Failed => Some(crate::CiTerminalVerdict::Failed),
        CiState::Pending => None,
    });
    // A fresh pending aggregate means the terminal edge was superseded by a
    // rerun before execution; do not report the stale terminal verdict.
    if fresh_status.is_some() && fresh_verdict.is_none() {
        return;
    }
    let Some(verdict) = fresh_verdict.or(facts.verdict) else {
        return;
    };
    let completed_at = fresh_status
        .and_then(|status| status.completed_at())
        .or(facts.completed_at);
    let detection_latency_ms = completed_at.map(|completed_at| {
        u64::try_from(
            detected_at
                .signed_duration_since(completed_at)
                .num_milliseconds(),
        )
        .unwrap_or(0)
    });
    let item = WorkItemRef::pull_request(
        format!("{}/{}", repo.owner, repo.name),
        address.number.get(),
    );
    let (queue, role) = selected
        .map(|(queue, role)| (Some(queue.as_str()), Some(role.as_str())))
        .unwrap_or((None, None));
    temper_log::emit::emit_ci_completed(temper_log::emit::CiCompleted {
        item: &item,
        conclusion: match verdict {
            crate::CiTerminalVerdict::Passed => "success",
            crate::CiTerminalVerdict::Failed => "failure",
        },
        duration_ms: 0,
        trigger_source: Some(facts.source.as_str()),
        detection_latency_ms,
        queue,
        role,
    });
}

async fn execute_mechanical_work(
    mechanical: Option<&Arc<dyn CoordinatedMechanical>>,
    repo: &RepositoryPath,
    targets: &WakeTargets,
    broad: bool,
    failures: &mut Vec<String>,
) -> bool {
    let Some(mechanical) = mechanical else {
        return false;
    };
    let mut changed = false;

    for ((kind, number), change) in prioritized_targets(targets) {
        let address = ArtifactAddress::new(kind, number);
        match mechanical
            .run_coordinated_targeted(repo.clone(), address, change)
            .await
        {
            Ok(target_changed) => changed |= target_changed,
            Err(error) => failures.push(format!(
                "targeted mechanical wake failed for {}#{}: {error}",
                artifact_kind(address.kind),
                address.number
            )),
        }
    }

    if broad {
        match mechanical.run_coordinated_broad(repo.clone()).await {
            Ok(broad_changed) => changed |= broad_changed,
            Err(error) => failures.push(format!("broad mechanical wake failed: {error}")),
        }
    }
    changed
}

impl<F: Forge + Send + Sync + ?Sized + 'static> WakeExecutor for ForgeWakeExecutor<F> {
    fn run(
        &self,
        work: WakeWork,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = WakeOutcome> + Send>> {
        let this = Arc::new(ForgeWakeExecutor {
            daemon: self.daemon.clone(),
            forge: Arc::clone(&self.forge),
            workflow: Arc::clone(&self.workflow),
            compiled: Arc::clone(&self.compiled),
            routes: self.routes.clone(),
            repositories: Arc::clone(&self.repositories),
            clock: self.clock.clone(),
            mechanical: self.mechanical.clone(),
        });
        Box::pin(async move { this.execute(work).await })
    }
}

fn protocol_artifact(address: ArtifactAddress) -> Artifact {
    Artifact {
        item: serde_json::json!(address.number.get()),
        kind: artifact_kind(address.kind).to_string(),
    }
}

fn artifact_kind(kind: HintArtifactKind) -> &'static str {
    match kind {
        HintArtifactKind::Issue => "issue",
        HintArtifactKind::PullRequest => "pull_request",
    }
}

fn repository_key(repo: &RepositoryPath) -> String {
    format!("{}/{}", repo.owner, repo.name)
}

#[cfg(test)]
#[path = "webhook_wiring/tests.rs"]
mod tests;
