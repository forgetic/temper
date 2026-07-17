// SPDX-License-Identifier: MPL-2.0

//! `temper-worker` — the slim, worker-only binary.
//!
//! It links the worker tier and `temper-config` and nothing else (no engine or
//! agent code). It spawns the sibling `temper-agent` binary for each coding job.

use std::process::ExitCode;

const USAGE: &str = "\
temper-worker — run only the orchestration-worker service

usage: temper-worker [--config <PATH>] [--secrets <PATH>]

Long-polls the engine for coding jobs and runs each by spawning the out-of-process
`temper-agent`. Reads the same config + credentials files as
`temper serve worker`. Equivalent to the public runtime surface
`temper serve worker`.";

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) =
        temper_worker::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        return status;
    }

    // Install the global tracing subscriber before arg parsing or the runtime
    // starts, so startup output flows through the env-aware subscriber.
    temper_log::init_logging();

    // Composition root: snapshot the real environment once (the sanctioned
    // `std::env` boundary), then hand plain data to the hermetic loader.
    let env = temper_config::EnvMap::from_system();
    let paths = temper_config::PathResolver::from_system();
    temper_config::service_main(
        "temper-worker",
        USAGE,
        std::env::args().skip(1),
        &env,
        &paths,
        |resolved| {
            temper_worker_service::run(
                resolved,
                temper_worker_service::sibling_program("temper-agent"),
            )
        },
    )
}
