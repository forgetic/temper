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
                context,
                cwd,
                trace.as_ref(),
                request.progress,
                request.fence,
                request.cancellation,
            )
            .await;
        if let Some(trace) = trace {
            terminal::finish_and_flush(&trace, &self.trace_collector, &outcome, job_id).await;
        }
        outcome
    }
}
