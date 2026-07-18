//! Early-main Linux supervisor helper for generated Cargo test harnesses.

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) =
        temper_agent::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        return status;
    }

    eprintln!("temper-agent-containment-helper is an internal supervisor entrypoint");
    ExitCode::FAILURE
}
