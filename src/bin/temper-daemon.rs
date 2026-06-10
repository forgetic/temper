// SPDX-License-Identifier: MPL-2.0

use std::{net::SocketAddr, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let bind = parse_args(std::env::args().skip(1))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to build tokio runtime: {error}"))?;

    let daemon = temper_daemon::Daemon::new();
    runtime
        .block_on(temper_daemon::serve(&daemon, bind))
        .map_err(|error| format!("serve failed: {error}"))
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<SocketAddr, String> {
    let mut bind = "127.0.0.1:8080".parse().expect("default bind is valid");
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => {
                let value = args
                    .next()
                    .ok_or_else(|| "missing value for --bind".to_string())?;
                bind = value
                    .parse()
                    .map_err(|error| format!("invalid --bind address {value:?}: {error}"))?;
            }
            unknown => return Err(format!("unknown argument: {unknown}")),
        }
    }

    Ok(bind)
}
