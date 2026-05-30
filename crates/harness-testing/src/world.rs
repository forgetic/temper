use async_trait::async_trait;
use harness_forge::{Forge, RepositoryId};
use harness_forge_filesystem::FilesystemForge;
use harness_forge_memory::MemoryForge;
use harness_runner::{
    AgentRegistry, CiPolicy, CiWorker, InProcessStage, InProcessWorkerContext,
    InProcessWorkerFactory, MultiProcessStage, PassCiPolicy, RunReport, RunnerConfig, Stage,
    StageError, Worker, WorkerProcess,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::agents::fake_registry;
use crate::ci::{FilesystemCiSink, MemoryCiSink};
use crate::workflow;

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

pub async fn full_reference_stage(
    config: RunnerConfig,
) -> Result<InProcessStage<MemoryForge>, StageError> {
    full_reference_stage_with(config, fake_registry(), PassCiPolicy).await
}

pub async fn full_reference_stage_with<P>(
    config: RunnerConfig,
    agents: AgentRegistry<MemoryForge>,
    ci_policy: P,
) -> Result<InProcessStage<MemoryForge>, StageError>
where
    P: CiPolicy + Clone + Send + Sync + 'static,
{
    let forge = MemoryForge::new();
    let stage =
        InProcessStage::with_identity(forge, workflow(), config, agents, |forge, binding| {
            forge.as_user(binding.user.clone())
        })
        .await?;
    Ok(stage.with_extra_worker_factory(MemoryCiWorkerFactory { policy: ci_policy }))
}

pub async fn full_reference_filesystem_stage(
    config: RunnerConfig,
) -> Result<FilesystemReferenceStage, StageError> {
    full_reference_filesystem_stage_with(config, fake_registry(), PassCiPolicy).await
}

pub async fn full_reference_multiprocess_stage(
    config: RunnerConfig,
) -> Result<FilesystemMultiProcessReferenceStage, StageError> {
    full_reference_multiprocess_stage_with(config, fake_registry(), PassCiPolicy).await
}

pub async fn full_reference_filesystem_stage_with<P>(
    config: RunnerConfig,
    agents: AgentRegistry<FilesystemForge>,
    ci_policy: P,
) -> Result<FilesystemReferenceStage, StageError>
where
    P: CiPolicy + Clone + Send + Sync + 'static,
{
    let root = TempRoot::new("runner-reference");
    let forge = root.forge();
    let stage =
        InProcessStage::with_identity(forge, workflow(), config, agents, |forge, binding| {
            forge.as_user(binding.user.clone())
        })
        .await?
        .with_extra_worker_factory(FilesystemCiWorkerFactory { policy: ci_policy });
    Ok(FilesystemReferenceStage { _root: root, stage })
}

pub async fn full_reference_multiprocess_stage_with<P>(
    config: RunnerConfig,
    agents: AgentRegistry<FilesystemForge>,
    ci_policy: P,
) -> Result<FilesystemMultiProcessReferenceStage, StageError>
where
    P: CiPolicy + Clone + Send + Sync + 'static,
{
    let root = TempRoot::new("runner-reference-multiprocess");
    let forge = root.forge();
    let stage = MultiProcessStage::with_handle_factory(
        forge,
        workflow(),
        config,
        agents,
        filesystem_process_handle,
    )
    .await?
    .with_extra_worker_factory(FilesystemCiWorkerFactory { policy: ci_policy });
    Ok(FilesystemMultiProcessReferenceStage { _root: root, stage })
}

fn filesystem_process_handle(
    forge: &FilesystemForge,
    process: WorkerProcess<'_>,
) -> FilesystemForge {
    let handle = FilesystemForge::new(forge.root());
    match process {
        WorkerProcess::Role(binding) => handle.as_user(binding.user.clone()),
        WorkerProcess::Mechanical | WorkerProcess::Extra { .. } => handle,
    }
}

pub struct FilesystemReferenceStage {
    _root: TempRoot,
    stage: InProcessStage<FilesystemForge>,
}

pub struct FilesystemMultiProcessReferenceStage {
    _root: TempRoot,
    stage: MultiProcessStage<FilesystemForge>,
}

#[async_trait]
impl Stage for FilesystemReferenceStage {
    async fn run_to_quiescence(&self, budget: u64) -> Result<RunReport, StageError> {
        self.stage.run_to_quiescence(budget).await
    }

    fn forge(&self) -> &dyn Forge {
        self.stage.forge()
    }

    fn repo(&self) -> &RepositoryId {
        self.stage.repo()
    }
}

#[async_trait]
impl Stage for FilesystemMultiProcessReferenceStage {
    async fn run_to_quiescence(&self, budget: u64) -> Result<RunReport, StageError> {
        self.stage.run_to_quiescence(budget).await
    }

    fn forge(&self) -> &dyn Forge {
        self.stage.forge()
    }

    fn repo(&self) -> &RepositoryId {
        self.stage.repo()
    }
}

pub fn memory_ci_worker<'a>(
    context: InProcessWorkerContext<'a, MemoryForge>,
) -> Box<dyn Worker + 'a> {
    memory_ci_worker_with_policy(context, PassCiPolicy)
}

fn memory_ci_worker_with_policy<'a, P>(
    context: InProcessWorkerContext<'a, MemoryForge>,
    policy: P,
) -> Box<dyn Worker + 'a>
where
    P: CiPolicy + Send + Sync + 'a,
{
    Box::new(CiWorker::with_policy(
        context.forge,
        context.repo,
        MemoryCiSink::new(context.forge.clone()),
        policy,
    ))
}

fn filesystem_ci_worker_with_policy<'a, P>(
    context: InProcessWorkerContext<'a, FilesystemForge>,
    policy: P,
) -> Box<dyn Worker + 'a>
where
    P: CiPolicy + Send + Sync + 'a,
{
    Box::new(CiWorker::with_policy(
        context.forge,
        context.repo,
        FilesystemCiSink::new(context.forge.clone()),
        policy,
    ))
}

struct MemoryCiWorkerFactory<P> {
    policy: P,
}

impl<P> InProcessWorkerFactory<MemoryForge> for MemoryCiWorkerFactory<P>
where
    P: CiPolicy + Clone + Send + Sync + 'static,
{
    fn build<'a>(&self, context: InProcessWorkerContext<'a, MemoryForge>) -> Box<dyn Worker + 'a> {
        memory_ci_worker_with_policy(context, self.policy.clone())
    }
}

struct FilesystemCiWorkerFactory<P> {
    policy: P,
}

impl<P> InProcessWorkerFactory<FilesystemForge> for FilesystemCiWorkerFactory<P>
where
    P: CiPolicy + Clone + Send + Sync + 'static,
{
    fn build<'a>(
        &self,
        context: InProcessWorkerContext<'a, FilesystemForge>,
    ) -> Box<dyn Worker + 'a> {
        filesystem_ci_worker_with_policy(context, self.policy.clone())
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(suite: &str) -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "harness-runner-{suite}-test-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
