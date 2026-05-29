use harness_forge_memory::MemoryForge;
use harness_runner::{
    AgentRegistry, CiPolicy, CiWorker, InProcessStage, InProcessWorkerContext,
    InProcessWorkerFactory, PassCiPolicy, RunnerConfig, StageError, Worker,
};

use super::agents::fake_registry;
use super::ci::MemoryCiSink;
use super::workflow;

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
