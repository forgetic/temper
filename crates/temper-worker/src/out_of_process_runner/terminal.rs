use std::time::Duration;

use temper_protocol_activity::FailureCodeV1;

use crate::agent_runner::{AgentRunError, AgentRunOutput};
use crate::trace::{TraceCollector, TraceRun};

const TERMINAL_ACTIVITY_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) async fn finish_and_flush(
    trace: &TraceRun,
    collector: &TraceCollector,
    outcome: &Result<AgentRunOutput, AgentRunError>,
    job_id: &str,
) {
    let terminal = match outcome {
        Ok(_) => trace.finish_success(None),
        Err(error) => trace.finish_failure(FailureCodeV1::ChildProcess, error.class),
    };
    match terminal {
        Ok(sequence) => {
            if !collector
                .await_acknowledged(trace.run_id(), sequence, TERMINAL_ACTIVITY_FLUSH_TIMEOUT)
                .await
            {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.terminal_flush_timeout",
                    run_id = trace.run_id(),
                    job_id,
                    "worker terminal activity flush did not complete before its deadline; preserving the agent outcome"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.terminal_failed",
                run_id = trace.run_id(),
                job_id,
                %error,
                "worker could not persist the terminal agent activity event"
            );
        }
    }
}
