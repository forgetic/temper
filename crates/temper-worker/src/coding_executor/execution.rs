use std::sync::Arc;

use temper_protocol_worker::Assign;

use super::{CodingExecutor, execute};
use crate::agent_runner::AgentRunner;
use crate::executor::{JobExecutionContext, JobExecutor, JobOutcome};

impl<R: AgentRunner + 'static> CodingExecutor<R> {
    /// Overrides process containment for worker-owned git and pre-push
    /// commands. Production composition leaves this unset and retains automatic
    /// cgroup/supervisor selection; hermetic process tests use it to select an
    /// explicitly built supervisor helper without process-global state.
    #[doc(hidden)]
    pub fn with_containment_factory(
        mut self,
        factory: temper_process_containment::ContainmentFactory,
    ) -> Self {
        self.containment_factory = Some(factory);
        self
    }
}

impl<R: AgentRunner + 'static> JobExecutor for CodingExecutor<R> {
    fn execute(
        &self,
        assign: Assign,
        execution: JobExecutionContext,
    ) -> impl std::future::Future<Output = JobOutcome> + Send {
        let config = self.config.clone();
        let runner = Arc::clone(&self.runner);
        let pr_freshness_guard = self.pr_freshness_guard.clone();
        let containment_factory = self.containment_factory.clone();
        async move {
            execute(
                config,
                runner,
                pr_freshness_guard,
                containment_factory,
                assign,
                execution,
            )
            .await
        }
    }
}
