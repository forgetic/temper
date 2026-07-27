use std::sync::Arc;

use temper_protocol_worker::{Assign, FailureClass};

use super::{CodingExecutor, execute, failure};
use crate::agent_runner::AgentRunner;
use crate::executor::{JobExecutionContext, JobExecutor, JobOutcome};

impl<R: AgentRunner + 'static> CodingExecutor<R> {
    /// Overrides the worker-owned exact-head validator command.
    #[doc(hidden)]
    pub fn with_native_validator_command(mut self, command: super::NativeValidatorCommand) -> Self {
        self.native_validator_command = command;
        self
    }

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
        let native_validator_command = self.native_validator_command.clone();
        let containment_factory = self.containment_factory.clone();
        async move {
            let attempt_id = execution.attempt.id.clone();
            let containment_factory = match containment_factory {
                Some(factory) => factory,
                None => match crate::process_containment::production_factory(
                    &assign.job_id,
                    &attempt_id,
                ) {
                    Ok(factory) => factory,
                    Err(error) => {
                        return failure(
                            FailureClass::Transient,
                            format!("create attempt process containment: {error}"),
                        );
                    }
                },
            };
            execution
                .cancellation
                .install_containment_factory(containment_factory);
            let cancellation = execution.cancellation.clone();
            let outcome = execute(
                config,
                runner,
                pr_freshness_guard,
                native_validator_command,
                assign,
                execution,
            )
            .await;
            // Direct callers receive the same joined process-owner guarantee
            // as WorkerShell across workspace, fingerprint, and gate commands.
            cancellation.wait_for_process_owners().await;
            outcome
        }
    }
}
