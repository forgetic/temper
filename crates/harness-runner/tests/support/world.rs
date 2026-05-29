use harness_forge_memory::MemoryForge;
use harness_runner::{
    CiWorker, InProcessStage, InProcessWorkerContext, RunnerConfig, StageError, Worker,
};

use super::agents::fake_registry;
use super::ci::MemoryCiSink;
use super::workflow;

pub async fn full_reference_stage(
    config: RunnerConfig,
) -> Result<InProcessStage<MemoryForge>, StageError> {
    let forge = MemoryForge::new();
    let stage = InProcessStage::with_identity(
        forge,
        workflow(),
        config,
        fake_registry(),
        |forge, binding| forge.as_user(binding.user.clone()),
    )
    .await?;
    Ok(stage.with_extra_worker_factory(memory_ci_worker))
}

pub fn memory_ci_worker<'a>(
    context: InProcessWorkerContext<'a, MemoryForge>,
) -> Box<dyn Worker + 'a> {
    Box::new(CiWorker::new(
        context.forge,
        context.repo,
        MemoryCiSink::new(context.forge.clone()),
    ))
}
