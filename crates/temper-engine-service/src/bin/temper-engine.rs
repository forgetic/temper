// SPDX-License-Identifier: MPL-2.0

//! `temper-engine` — the slim, engine-only binary.
//!
//! It links the engine tier and `temper-config` and nothing else (no worker or
//! agent code). The whole program is: parse `--config`/`--secrets`, resolve the
//! deployment, run the engine.

use std::process::ExitCode;

const USAGE: &str = "\
temper-engine — run only the orchestrator (engine) service

usage: temper-engine [--config <PATH>] [--secrets <PATH>]

Reads the same config + credentials files as the unified `temper daemon`, runs
only the engine, and serves until SIGINT/SIGTERM. Equivalent to
`temper daemon --service engine`.";

fn main() -> ExitCode {
    // Install the global tracing subscriber before arg parsing or the runtime
    // starts, so startup output flows through the env-aware subscriber.
    temper_log::init_logging();

    // Composition root: snapshot the real environment once (the sanctioned
    // `std::env` boundary), then hand plain data to the hermetic loader.
    let env = temper_config::EnvMap::from_system();
    let paths = temper_config::PathResolver::from_system();
    temper_config::service_main(
        "temper-engine",
        USAGE,
        std::env::args().skip(1),
        &env,
        &paths,
        temper_engine_service::run,
    )
}
