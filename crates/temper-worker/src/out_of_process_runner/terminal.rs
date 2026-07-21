use std::time::Duration;

use temper_protocol_activity::FailureCodeV1;

use crate::agent_runner::{AgentRunError, AgentRunOutput};
use crate::executor::JobCancellation;
use crate::trace::{TraceCollector, TraceRun};

const ORDINARY_TERMINAL_ACTIVITY_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) async fn finish_and_flush(
    trace: &TraceRun,
    collector: &TraceCollector,
    outcome: &Result<AgentRunOutput, AgentRunError>,
    cancelled: bool,
    cancellation: &JobCancellation,
    job_id: &str,
) {
    // RunResources writes the synthetic cancellation as soon as process and
    // endpoint cleanup joins. TraceRun::finish_cancelled is intentionally
    // idempotent, so this outer orchestration layer recovers that exact durable
    // sequence instead of misclassifying AlreadyTerminal as a write failure.
    let terminal = if cancelled {
        trace.finish_cancelled()
    } else {
        match outcome {
            Ok(_) => trace.finish_success(None),
            Err(error) => trace.finish_failure(FailureCodeV1::ChildProcess, error.class),
        }
    };
    match terminal {
        Ok(sequence) if cancelled => {
            cancellation.quiescence_pending(format!(
                "terminal trace {} sequence {sequence} is awaiting durable acknowledgement",
                trace.run_id()
            ));
            collector
                .await_terminal_acknowledged(trace.run_id(), sequence)
                .await;
        }
        Ok(sequence) => {
            if !collector
                .await_acknowledged(
                    trace.run_id(),
                    sequence,
                    ORDINARY_TERMINAL_ACTIVITY_FLUSH_TIMEOUT,
                )
                .await
            {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.terminal_flush_timeout",
                    run_id = trace.run_id(),
                    job_id,
                    "worker ordinary terminal activity flush did not complete before its deadline; preserving the agent outcome"
                );
            }
        }
        Err(error) if cancelled => {
            cancellation.quiescence_pending(format!(
                "cancelled terminal trace {} could not be persisted: {error}",
                trace.run_id()
            ));
            tracing::warn!(
                target: "temper::worker",
                service = "worker",
                event = "agent.activity.cancelled_terminal_pending",
                run_id = trace.run_id(),
                job_id,
                %error,
                "worker cannot prove cancellation quiescence until terminal activity is durable and acknowledged"
            );
            std::future::pending::<()>().await;
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
