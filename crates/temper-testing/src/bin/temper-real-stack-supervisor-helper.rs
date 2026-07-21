//! Early-main Linux supervisor dispatcher for hermetic real-stack tests.
//!
//! This binary deliberately does no runtime or fixture initialization before
//! checking the private supervisor mode. The ownership-loss integration test
//! passes its Cargo-built path into `HermeticRealStackBuilder` explicitly.

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match temper_process_containment::dispatch_linux_supervisor_helper(std::env::args_os().skip(1))
    {
        Some(status) => status,
        None => {
            eprintln!(
                "temper-real-stack-supervisor-helper is private to process containment tests"
            );
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("temper-real-stack-supervisor-helper is only available on Linux");
    std::process::ExitCode::FAILURE
}
