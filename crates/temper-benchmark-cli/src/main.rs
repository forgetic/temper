// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use temper_benchmark_cli::{
    AnalyzeOptions, HarnessRunOptions, LiveRunOptions, analyze_trace, compare_benchmarks,
    ingest_trace, load_comparison_input, render_aggregate_markdown, render_comparison_markdown,
    render_run_summary_markdown, run_harness, run_live, write_canonical_export,
    write_comparison_artifacts, write_run_summary,
};
use temper_config::EnvMap;

const ANALYSIS_TRACE_FILE: &str = "trace.export.jsonl";

const USAGE: &str = "\
temper-benchmark: agent-session benchmark trace tooling

Usage:
  temper-benchmark analyze --trace <PATH> --output-dir <DIR>
  temper-benchmark run --benchmark <MANIFEST> --mode harness --agent-bin <PATH> --output-dir <DIR> [--repetitions <N>]
  TEMPER_BENCHMARK_LIVE=1 temper-benchmark run --benchmark <MANIFEST> --mode live --agent-bin <PATH> --output-dir <DIR> [--config <PATH>] [--secrets <PATH>] [--pool <NAME>] [--repetitions <N>]
  temper-benchmark normalize --trace <PATH> --output <FILE>
  temper-benchmark compare --base <ARTIFACT-OR-SUMMARY> --head <ARTIFACT-OR-SUMMARY> [--output-dir <DIR>]
  temper-benchmark --help

Commands:
  analyze    Derive metrics and write run.json, run.md, and a canonical trace export
  run        Execute fresh direct agent sessions in CI-safe harness or credential-gated live mode
  normalize  Validate journal/events/export input and write canonical export JSONL
  compare    Render a report-only comparison without rerunning either benchmark
";

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    if let Some(status) = temper_worker::dispatch_linux_supervisor_helper(env::args_os().skip(1)) {
        return status;
    }

    let args = env::args().skip(1).collect::<Vec<_>>();
    // Composition root: snapshot the process environment once, then pass plain
    // data through live opt-in and normal Temper config/credential resolution.
    let environment = EnvMap::from_system();
    match args.as_slice() {
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        [command, rest @ ..] if command == "analyze" => match parse_analyze_args(rest) {
            Ok(args) => analyze(args),
            Err(message) => usage_error(message),
        },
        [command, rest @ ..] if command == "run" => match parse_run_args(rest) {
            Ok(args) => run(args, &environment),
            Err(message) => usage_error(message),
        },
        [command, trace_flag, trace, output_flag, output]
            if command == "normalize" && trace_flag == "--trace" && output_flag == "--output" =>
        {
            normalize(PathBuf::from(trace), PathBuf::from(output))
        }
        [command, rest @ ..] if command == "compare" => match parse_compare_args(rest) {
            Ok(args) => compare(args),
            Err(message) => usage_error(message),
        },
        [] => {
            print!("{USAGE}");
            ExitCode::from(64)
        }
        _ => usage_error("invalid arguments"),
    }
}

fn usage_error(message: impl std::fmt::Display) -> ExitCode {
    eprintln!("{message}\n\n{USAGE}");
    ExitCode::from(64)
}

struct AnalyzeArgs {
    trace: PathBuf,
    output_dir: PathBuf,
}

fn parse_analyze_args(args: &[String]) -> Result<AnalyzeArgs, String> {
    let mut values = parse_path_flags(args, &["--trace", "--output-dir"], "analyze")?;
    Ok(AnalyzeArgs {
        trace: values
            .remove("--trace")
            .ok_or_else(|| "analyze requires `--trace`".to_string())?,
        output_dir: values
            .remove("--output-dir")
            .ok_or_else(|| "analyze requires `--output-dir`".to_string())?,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunMode {
    Harness,
    Live,
}

struct RunArgs {
    mode: RunMode,
    benchmark: PathBuf,
    agent_bin: PathBuf,
    output_dir: PathBuf,
    repetitions: Option<u32>,
    config: Option<PathBuf>,
    credentials: Option<PathBuf>,
    worker_pool: Option<String>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut values = BTreeMap::<String, String>::new();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if ![
            "--benchmark",
            "--mode",
            "--agent-bin",
            "--output-dir",
            "--repetitions",
            "--config",
            "--secrets",
            "--pool",
        ]
        .contains(&flag)
        {
            return Err(format!("unknown run argument `{flag}`"));
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for `{flag}`"))?;
        if values.insert(flag.to_string(), value.clone()).is_some() {
            return Err(format!("duplicate argument `{flag}`"));
        }
        index += 2;
    }
    let mode = match values
        .remove("--mode")
        .ok_or_else(|| "run requires `--mode <harness|live>`".to_string())?
        .as_str()
    {
        "harness" => RunMode::Harness,
        "live" => RunMode::Live,
        mode => {
            return Err(format!(
                "unsupported benchmark mode `{mode}`; expected `harness` or `live`"
            ));
        }
    };
    let repetitions = values
        .remove("--repetitions")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| format!("--repetitions expects a positive integer, got `{value}`"))
                .and_then(|value| {
                    (value > 0)
                        .then_some(value)
                        .ok_or_else(|| "--repetitions must be greater than zero".to_string())
                })
        })
        .transpose()?;
    let config = values.remove("--config").map(PathBuf::from);
    let credentials = values.remove("--secrets").map(PathBuf::from);
    let worker_pool = values.remove("--pool");
    if mode == RunMode::Harness
        && (config.is_some() || credentials.is_some() || worker_pool.is_some())
    {
        return Err(
            "`--config`, `--secrets`, and `--pool` are live-mode options and cannot be used with harness mode"
                .to_string(),
        );
    }
    Ok(RunArgs {
        mode,
        benchmark: PathBuf::from(
            values
                .remove("--benchmark")
                .ok_or_else(|| "run requires `--benchmark`".to_string())?,
        ),
        agent_bin: PathBuf::from(
            values
                .remove("--agent-bin")
                .ok_or_else(|| "run requires `--agent-bin`".to_string())?,
        ),
        output_dir: PathBuf::from(
            values
                .remove("--output-dir")
                .ok_or_else(|| "run requires `--output-dir`".to_string())?,
        ),
        repetitions,
        config,
        credentials,
        worker_pool,
    })
}

struct CompareArgs {
    base: PathBuf,
    head: PathBuf,
    output_dir: Option<PathBuf>,
}

fn parse_compare_args(args: &[String]) -> Result<CompareArgs, String> {
    let mut values = parse_path_flags(args, &["--base", "--head", "--output-dir"], "compare")?;
    Ok(CompareArgs {
        base: values
            .remove("--base")
            .ok_or_else(|| "compare requires `--base`".to_string())?,
        head: values
            .remove("--head")
            .ok_or_else(|| "compare requires `--head`".to_string())?,
        output_dir: values.remove("--output-dir"),
    })
}

fn parse_path_flags(
    args: &[String],
    allowed: &[&str],
    command: &str,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut values = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        if !allowed.contains(&flag.as_str()) {
            return Err(format!("unknown {command} argument `{flag}`"));
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for `{flag}`"))?;
        if values.insert(flag.clone(), PathBuf::from(value)).is_some() {
            return Err(format!("duplicate argument `{flag}`"));
        }
        index += 2;
    }
    Ok(values)
}

fn analyze(args: AnalyzeArgs) -> ExitCode {
    let result = (|| {
        let trace = ingest_trace(&args.trace)?;
        let summary = analyze_trace(&trace, &AnalyzeOptions::default());
        write_run_summary(&summary, &args.output_dir)?;
        write_canonical_export(&trace, args.output_dir.join(ANALYSIS_TRACE_FILE))?;
        print!("{}", render_run_summary_markdown(&summary));
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    report_result(result)
}

fn run(args: RunArgs, env: &EnvMap) -> ExitCode {
    let result = (|| {
        let aggregate = match args.mode {
            RunMode::Harness => run_harness(&HarnessRunOptions {
                benchmark: args.benchmark,
                agent_bin: args.agent_bin,
                output_dir: args.output_dir,
                repetitions: args.repetitions,
            })?,
            RunMode::Live => run_live(
                &LiveRunOptions {
                    benchmark: args.benchmark,
                    agent_bin: args.agent_bin,
                    output_dir: args.output_dir,
                    repetitions: args.repetitions,
                    config: args.config,
                    credentials: args.credentials,
                    worker_pool: args.worker_pool,
                },
                env,
            )?,
        };
        print!("{}", render_aggregate_markdown(&aggregate));
        Ok::<_, Box<dyn std::error::Error>>(())
    })();
    report_result(result)
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
