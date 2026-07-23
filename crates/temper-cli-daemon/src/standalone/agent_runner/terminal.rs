use std::time::Duration;

use temper_agent::CodingAgentError;
use temper_protocol_activity::FailureCodeV1;
use temper_worker::{JobCancellation, TerminalTraceBlocker, TraceCollector, TraceRun};

use super::coding_agent_failure_class;

const ORDINARY_TERMINAL_ACTIVITY_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

pub(super) async fn finish_and_acknowledge<T>(
    trace: Option<TraceRun>,
    collector: &TraceCollector,
    cancellation: &JobCancellation,
    tracing_required: bool,
    worker_cancellation_requested: bool,
    outcome: &Result<T, CodingAgentError>,
) {
    if let Some(trace) = trace {
        // Authoritative cancellation has one canonical terminal boundary even
        // when the native future returned a late success.
        let terminal = if worker_cancellation_requested {
            trace.finish_cancelled()
        } else {
            match outcome {
                Ok(_) => trace.finish_success(None),
                Err(error) => trace.finish_failure(
                    FailureCodeV1::Internal,
                    coding_agent_failure_class(error, false),
                ),
            }
        };
        match terminal {
            Ok(sequence) if worker_cancellation_requested => {
                cancellation.terminal_trace_pending(
                    TerminalTraceBlocker::awaiting_acknowledgement(trace.run_id(), sequence),
                );
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
                        "standalone worker ordinary terminal activity flush did not complete before its deadline; preserving the agent outcome"
                    );
                }
            }
            Err(error) if worker_cancellation_requested => {
                cancellation.terminal_trace_pending(TerminalTraceBlocker::persistence_failed(
                    trace.run_id(),
                ));
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.cancelled_terminal_pending",
                    run_id = trace.run_id(),
                    %error,
                    "standalone worker cannot prove cancellation quiescence until terminal activity is durable and acknowledged"
                );
                std::future::pending::<()>().await;
            }
            Err(error) => {
                tracing::warn!(
                    target: "temper::worker",
                    service = "worker",
                    event = "agent.activity.terminal_failed",
                    run_id = trace.run_id(),
                    %error,
                    "standalone worker could not persist the terminal agent activity event"
                );
            }
        }
    } else if tracing_required && worker_cancellation_requested {
        cancellation.terminal_trace_pending(TerminalTraceBlocker::trace_unavailable());
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "agent.activity.cancelled_terminal_pending",
            "standalone worker cannot prove cancellation quiescence because enabled durable tracing did not start"
        );
        std::future::pending::<()>().await;
    }
}
