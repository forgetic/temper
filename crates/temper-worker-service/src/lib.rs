// SPDX-License-Identifier: MPL-2.0

//! The orchestration **worker** service: long-polls the engine and runs coding
//! jobs by spawning the out-of-process agent.
//!
//! [`run`] is shared by the slim `temper-worker` binary and the unified binary's
//! `temper serve worker` path. The worker links **no** agent/LLM code: the
//! coding agent runs as a separate process (the `temper-agent` binary, or
//! `temper agent`), and the provider/identity wiring is injected into that
//! process's environment from the resolved config.

mod adapt;
mod codebase_memory_maintenance;
mod run;

pub use adapt::{
    AgentInvocation, AgentSupervisionKind, agent_invocation,
    agent_invocation_with_first_party_program, agent_runtime_limits, agent_tool_config,
    git_base_url, role_identities, selected_agent_runtime_limits, selected_worker_auth,
    session_recovery_policy, worker_agent_trace_config, worker_config, worker_liveness_limits,
};
pub use codebase_memory_maintenance::codebase_memory_maintenance_config;
pub use run::{run, self_subcommand, sibling_program};
pub use temper_worker::{
    CodebaseMemoryMaintenanceConfig, CodebaseMemoryMaintenanceTask,
    run_codebase_memory_maintenance, spawn_codebase_memory_maintenance_task,
};
