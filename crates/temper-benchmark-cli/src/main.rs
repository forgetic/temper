// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use temper_benchmark_cli::{
    compare_benchmarks, ingest_trace, load_comparison_input, render_comparison_markdown,
    write_canonical_export, write_comparison_artifacts,
};

const USAGE: &str = "\
temper-benchmark: agent-session benchmark trace tooling

Usage:
  temper-benchmark normalize --trace <PATH> --output <FILE>
  temper-benchmark compare --base <ARTIFACT-OR-SUMMARY> --head <ARTIFACT-OR-SUMMARY> [--output-dir <DIR>]
  temper-benchmark --help

Commands:
  normalize  Validate journal/events/export input and write canonical export JSONL
  compare    Render a report-only comparison without rerunning either benchmark
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
        [command, rest @ ..] if command == "compare" => match parse_compare_args(rest) {
            Ok(args) => compare(args),
            Err(message) => {
                eprintln!("{message}\n\n{USAGE}");
                ExitCode::from(64)
            }
        },
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

struct CompareArgs {
    base: PathBuf,
    head: PathBuf,
    output_dir: Option<PathBuf>,
}

fn parse_compare_args(args: &[String]) -> Result<CompareArgs, String> {
    let mut base = None;
    let mut head = None;
    let mut output_dir = None;
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for `{flag}`"))?;
        let destination = match flag.as_str() {
            "--base" => &mut base,
            "--head" => &mut head,
            "--output-dir" => &mut output_dir,
            _ => return Err(format!("unknown compare argument `{flag}`")),
        };
        if destination.replace(PathBuf::from(value)).is_some() {
            return Err(format!("duplicate compare argument `{flag}`"));
        }
        index += 2;
    }
    Ok(CompareArgs {
        base: base.ok_or_else(|| "compare requires `--base`".to_string())?,
        head: head.ok_or_else(|| "compare requires `--head`".to_string())?,
        output_dir,
    })
}

fn compare(args: CompareArgs) -> ExitCode {
    let result = (|| {
        let base = load_comparison_input(&args.base)?;
        let head = load_comparison_input(&args.head)?;
        let comparison = compare_benchmarks(&base, &head)?;
        let markdown = render_comparison_markdown(&comparison);
        if let Some(output_dir) = args.output_dir {
            write_comparison_artifacts(&comparison, output_dir)?;
        }
        print!("{markdown}");
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    report_result(result)
}

fn normalize(trace_path: PathBuf, output_path: PathBuf) -> ExitCode {
    let result = (|| {
        let trace = ingest_trace(&trace_path)?;
        write_canonical_export(&trace, &output_path)?;
        let summary = serde_json::to_string_pretty(&trace.run_summary())?;
        println!("{summary}");
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    report_result(result)
}

fn report_result(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper-benchmark: {error}");
            ExitCode::FAILURE
        }
    }
}
