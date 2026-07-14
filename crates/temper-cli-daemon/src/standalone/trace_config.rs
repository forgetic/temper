// SPDX-License-Identifier: MPL-2.0

//! Operator-visible degradation warnings for standalone trace storage.

use temper_config::Resolved;
use temper_engine::EngineAgentTraceConfig;
use temper_worker::WorkerAgentTraceConfig;

pub(super) fn warn_if_engine_storage_unavailable(
    resolved: &Resolved,
    traces: &EngineAgentTraceConfig,
) {
    if resolved.observability.agent_traces.capture_requested() && traces.journal_root.is_none() {
        tracing::warn!(
            target: "temper::engine",
            service = "engine",
            "agent tracing disabled: no durable paths.state_dir is available for the engine journal"
        );
    }
}

pub(super) fn warn_if_worker_storage_unavailable(
    resolved: &Resolved,
    traces: &WorkerAgentTraceConfig,
) {
    if resolved.observability.agent_traces.capture_requested() && traces.spool_root.is_none() {
        tracing::warn!(
            target: "temper::worker",
            service = "worker",
            "agent tracing disabled: no durable paths.state_dir is available for the worker spool"
        );
    }
}
