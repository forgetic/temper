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
mod run;

pub use adapt::{
    agent_tool_config, git_base_url, role_identities, selected_worker_auth,
    worker_agent_trace_config, worker_config,
};
pub use run::{run, self_subcommand, sibling_program};
