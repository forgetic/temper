// SPDX-License-Identifier: MPL-2.0

//! `temper-agent` — the slim, agent-only binary.
//!
//! It links the agent tier (the coding loop + LLM/HTTP stack) and nothing else.
//! The worker spawns it per coding job; it is also reachable as `temper agent`.

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) =
        temper_agent_session::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        return status;
    }

    // Install the global tracing subscriber before arg parsing or the runtime
    // starts, so startup output flows through the env-aware subscriber.
    temper_log::init_logging();

    temper_agent_session::main(std::env::args().skip(1))
}
