use super::TraceError;

/// The runner boundary that failed open after durable trace admission failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityTraceRunner {
    OutOfProcess,
    Standalone,
}

impl ActivityTraceRunner {
    const fn description(self) -> &'static str {
        match self {
            Self::OutOfProcess => "worker",
            Self::Standalone => "standalone worker",
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::OutOfProcess => "out_of_process",
            Self::Standalone => "standalone",
        }
    }
}

/// Emits the ordinary fail-open trace warning without relying on `%error` for
/// aggregate quota values that operators need to reclaim capacity.
pub fn warn_activity_trace_start_failed(
    runner: ActivityTraceRunner,
    job_id: &str,
    correlation_key: &str,
    error: &TraceError,
) {
    if let TraceError::AggregateQuotaExceeded {
        physical_used_bytes,
        logical_reserved_bytes,
        requested_bytes,
        limit,
        dirty_run_count,
    } = error
    {
        let message = format!(
            "{} could not start durable agent tracing; continuing without it (physical used bytes {}, logical reserved bytes {}, requested bytes {}, limit {}, dirty runs {})",
            runner.description(),
            physical_used_bytes,
            logical_reserved_bytes,
            requested_bytes,
            limit,
            dirty_run_count,
        );
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "agent.activity.start_failed",
            runner = runner.as_str(),
            job_id,
            correlation_key,
            physical_used_bytes,
            logical_reserved_bytes,
            requested_bytes,
            limit,
            dirty_run_count,
            %error,
            "{message}"
        );
    } else {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            event = "agent.activity.start_failed",
            runner = runner.as_str(),
            job_id,
            correlation_key,
            %error,
            "{} could not start durable agent tracing; continuing without it",
            runner.description()
        );
    }
}
