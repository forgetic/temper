// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use temper_benchmark_cli::{ingest_trace, write_canonical_export};

const USAGE: &str = "\
temper-benchmark: agent-session benchmark trace tooling

Usage:
  temper-benchmark normalize --trace <PATH> --output <FILE>
  temper-benchmark --help

Commands:
  normalize  Validate journal/events/export input and write canonical export JSONL
";

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        [command, trace_flag, trace, output_flag, output]
            if command == "normalize" && trace_flag == "--trace" && output_flag == "--output" =>
        {
            normalize(PathBuf::from(trace), PathBuf::from(output))
        }
        [] => {
            print!("{USAGE}");
            ExitCode::from(64)
        }
        _ => {
            eprintln!("invalid arguments\n\n{USAGE}");
            ExitCode::from(64)
        }
    }
}

fn normalize(trace_path: PathBuf, output_path: PathBuf) -> ExitCode {
    let result = (|| {
        let trace = ingest_trace(&trace_path)?;
        write_canonical_export(&trace, &output_path)?;
        let summary = serde_json::to_string_pretty(&trace.run_summary())?;
        println!("{summary}");
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-benchmark: {error}");
            ExitCode::FAILURE
        }
    }
}
