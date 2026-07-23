// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::sync::Arc;

use skein::runtime::RuntimeHandle;
use temper_engine::{
    ArtifactContextBundleService, ArtifactContextPolicy, ConfiguredRepositoryCatalog, Daemon,
    RepositoryTarget, RoleFeedMode, RoleFeedTarget,
};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{Forge, RepositoryId, RepositoryPath};
use temper_workflow::{CompiledWorkflow, LeasePolicy, RoleId, ValidatedWorkflow};

use super::stack::HermeticRealStack;

impl HermeticRealStack {
    pub(super) fn build_daemon(&self, handle: &RuntimeHandle) -> Daemon {
        let applier = Arc::new(temper_engine::LeaseApplier::new(
            self.forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "hermetic-daemon",
            Arc::new(
                temper_engine::ForgeApplier::new(self.forge.clone(), self.workflow.clone())
                    .with_child_issue_hook(Arc::new(self.hooks.clone())),
            ),
            self.clock.capability(),
        ));
        let artifact_context = service(self.forge.clone(), self.workflow.clone(), &self.repo_ids);
        let daemon = Daemon::with_applier(Arc::new(handle.clone()), applier)
            .with_artifact_context_service(artifact_context)
            .with_forge_context_reader(self.forge.clone(), self.workflow.clone());
        let daemon = match self.trace_journal.as_ref() {
            Some(journal) => daemon.with_trace_journal(journal.clone()),
            None => daemon,
        };
        let daemon = match self.apply_grace {
            Some(grace) => daemon.with_apply_grace(grace),
            None => daemon,
        };
        with_wake_execution(
            daemon,
            self.forge.clone(),
            self.workflow.clone(),
            &self.compiled,
            &self.repo_ids,
            &self.role,
            &self.clock,
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn with_wake_execution(
    daemon: Daemon,
    forge: Arc<MemoryForge>,
    workflow: Arc<ValidatedWorkflow>,
    compiled: &CompiledWorkflow,
    repo_ids: &BTreeMap<String, RepositoryId>,
    role: &str,
    clock: &super::MutableWallClock,
) -> Daemon {
    let role = RoleId::new(role.to_string());
    let targets = repo_ids
        .iter()
        .map(|(path, id)| {
            let (owner, name) = path
                .split_once('/')
                .expect("hermetic repository path is owner/name");
            RoleFeedTarget {
                repo: id.clone(),
                path: RepositoryPath::new(owner, name),
                role: role.clone(),
                mode: RoleFeedMode::Wake,
            }
        })
        .collect();
    daemon.with_wake_execution(
        forge,
        workflow,
        Arc::new(compiled.clone()),
        targets,
        clock.capability(),
        None,
    )
}

pub(super) fn daemon_service(daemon: &Daemon) -> Arc<ArtifactContextBundleService> {
    daemon
        .artifact_context_service()
        .expect("hermetic daemon has artifact-context service")
}

pub(super) fn service(
    forge: Arc<MemoryForge>,
    workflow: Arc<ValidatedWorkflow>,
    repo_ids: &BTreeMap<String, RepositoryId>,
) -> Arc<ArtifactContextBundleService> {
    let catalog = ConfiguredRepositoryCatalog::new(
        repo_ids.iter().map(|(path, id)| {
            let (owner, name) = path
                .split_once('/')
                .expect("hermetic repository path is owner/name");
            RepositoryTarget::new(id.clone(), RepositoryPath::new(owner, name))
        }),
        "https://forge.example",
    )
    .expect("hermetic repository catalog is valid");
    let forge: Arc<dyn Forge> = forge;
    Arc::new(ArtifactContextBundleService::new(
        forge,
        workflow,
        catalog,
        ArtifactContextPolicy::default(),
    ))
}
