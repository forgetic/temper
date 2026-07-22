//! [`AgentRunner`] implementation and per-attempt trace bracketing.

use std::path::Path;

use temper_protocol_agent::WorkspaceContext;

use super::{OutOfProcessRunner, terminal};
use crate::agent_runner::{AgentRunError, AgentRunOutput, AgentRunRequest, AgentRunner};

impl AgentRunner for OutOfProcessRunner {
    async fn run(
        &self,
        job_id: &str,
        context: &WorkspaceContext,
        cwd: &Path,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.run_attempt(AgentRunRequest::unsupervised(job_id, context, cwd))
            .await
    }

    async fn run_request(
        &self,
        request: AgentRunRequest<'_>,
    ) -> Result<AgentRunOutput, AgentRunError> {
        self.run_attempt(request).await
    }
}

impl OutOfProcessRunner {
    async fn run_attempt(
        &self,
        request: AgentRunRequest<'_>,
    ) -> Result<AgentRunOutput, AgentRunError> {
        let job_id = request.job_id;
        let context = request.context;
        let cwd = request.cwd;
        let fence = request.fence.clone();
        let cancellation = request.cancellation.clone();
        // Keep one owner around the complete trace-before-quiescence boundary.
        // The process supervisor's inner owner may leave after local cleanup,
        // but WorkerShell must not race cancellation against terminal forwarding.
        let _terminal_cancellation_owner = cancellation.register_async_owner();
        let tracing_required = self.trace_collector.tracing_enabled();
        let trace = match self.trace_collector.begin_run(job_id, context) {
            Ok(trace) => trace,
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.start_failed",
                    job_id,
                    correlation_key = context.correlation_key.as_str(),
                    %error,
                    "worker could not start durable agent tracing; continuing without it"
                );
                None
            }
        };
        let outcome = self
            .run_agent(
                job_id,
                &request.attempt_id,
                context,
                cwd,
                trace.as_ref(),
                request.progress,
                request.fence,
                request.cancellation,
            )
            .await;
        let cancelled = cancellation.is_cancelled()
            || !fence.is_open()
            || outcome
                .as_ref()
                .is_err_and(|error| error.class == temper_protocol_worker::FailureClass::Canceled);
        if let Some(trace) = trace {
            terminal::finish_and_flush(
                &trace,
                &self.trace_collector,
                &outcome,
                cancelled,
                &cancellation,
                job_id,
            )
            .await;
        } else if tracing_required && cancelled {
            cancellation.terminal_trace_pending(crate::TerminalTraceBlocker::trace_unavailable());
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.cancelled_terminal_pending",
                job_id,
                "worker cannot prove cancellation quiescence because enabled durable tracing did not start"
            );
            std::future::pending::<()>().await;
        }
        outcome
    }
}
