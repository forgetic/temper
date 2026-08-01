use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitStatus};

use serde_json::Value;

fn main() {
    let mut args = env::args_os();
    let program = args.next().unwrap_or_else(|| OsString::from("temper-dev"));

    let Some(command) = args.next() else {
        print_usage(&program);
        process::exit(2);
    };

    if command == OsStr::new("dev-test-full") {
        let passthrough_args: Vec<OsString> = args.collect();
        process::exit(run_dev_test_full(&passthrough_args));
    }
    if command == OsStr::new("dev-scenario-run") {
        let scenario_args: Vec<OsString> = args.collect();
        process::exit(run_dev_scenario_run(&scenario_args));
    }
    if command == OsStr::new("dev-benchmark-harness") {
        let benchmark_args: Vec<OsString> = args.collect();
        process::exit(run_dev_benchmark_harness(&benchmark_args));
    }

    eprintln!("unknown temper-dev command: {}", command.to_string_lossy());
    print_usage(&program);
    process::exit(2);
}

fn print_usage(program: &OsStr) {
    eprintln!(
        "usage: {} <command>\n\ncommands:\n  dev-test-full [nextest-args...]\n  dev-scenario-run <scenario-path>\n  dev-benchmark-harness",
        program.to_string_lossy()
    );
}

fn run_dev_test_full(passthrough_args: &[OsString]) -> i32 {
    let quick_status = if has_passthrough_arg(passthrough_args, "--no-run") {
        run_cargo(&[OsStr::new("dev-test-quick")], passthrough_args)
    } else {
        run_cargo(
            &[OsStr::new("dev-test-quick"), OsStr::new("--no-fail-fast")],
            passthrough_args,
        )
    };

    let capstone_status = run_cargo(&[OsStr::new("dev-test-e2e-capstones")], passthrough_args);

    if quick_status == 0 {
        capstone_status
    } else {
        quick_status
    }
}

fn run_dev_scenario_run(args: &[OsString]) -> i32 {
    let scenario = match args {
        [] => {
            eprintln!("temper-dev: dev-scenario-run requires an explicit mapped scenario path");
            return 2;
        }
        [scenario] => scenario.clone(),
        [_, extra, ..] => {
            eprintln!(
                "temper-dev: unexpected dev-scenario-run argument: {}",
                extra.to_string_lossy()
            );
            return 2;
        }
    };

    let build_status = run_cargo(
        &[
            OsStr::new("build"),
            OsStr::new("--bin"),
            OsStr::new("temper"),
        ],
        &[],
    );
    if build_status != 0 {
        return build_status;
    }

    run_cargo_owned(&scenario_run_args(scenario, &target_debug_temper_binary()))
}

fn scenario_run_args(scenario: OsString, temper_binary: &Path) -> Vec<OsString> {
    vec![
        OsString::from("run"),
        OsString::from("-p"),
        OsString::from("temper-scenario-cli"),
        OsString::from("--"),
        OsString::from("run"),
        OsString::from("--temper-bin"),
        temper_binary.as_os_str().to_owned(),
        scenario,
    ]
}

fn target_debug_temper_binary() -> PathBuf {
    target_dir()
        .join("debug")
        .join(format!("temper{}", env::consts::EXE_SUFFIX))
}

fn target_dir() -> PathBuf {
    env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target"))
}

const BENCHMARK_NAME: &str = "cross-cutting-rust-change";
const HARNESS_DISCLAIMER: &str = "not representative LLM performance";

fn run_dev_benchmark_harness(args: &[OsString]) -> i32 {
    if let Some(extra) = args.first() {
        eprintln!(
            "temper-dev: unexpected dev-benchmark-harness argument: {}",
            extra.to_string_lossy()
        );
        return 2;
    }
    let build_status = run_cargo_owned(&[
        OsString::from("build"),
        OsString::from("-p"),
        OsString::from("temper-agent-session"),
        OsString::from("--bin"),
        OsString::from("temper-agent"),
    ]);
    if build_status != 0 {
        return build_status;
    }

    let output_dir = target_dir().join("benchmark-harness").join(BENCHMARK_NAME);
    if let Err(error) = remove_old_artifacts(&output_dir) {
        eprintln!("temper-dev: {error}");
        return 1;
    }
    let benchmark = format!("benchmarks/agent-sessions/{BENCHMARK_NAME}/benchmark.toml");
    let run_status = run_cargo_owned(&[
        OsString::from("run"),
        OsString::from("--quiet"),
        OsString::from("-p"),
        OsString::from("temper-benchmark-cli"),
        OsString::from("--bin"),
        OsString::from("temper-benchmark"),
        OsString::from("--"),
        OsString::from("run"),
        OsString::from("--benchmark"),
        OsString::from(benchmark),
        OsString::from("--mode"),
        OsString::from("harness"),
        OsString::from("--agent-bin"),
        target_dir()
            .join("debug")
            .join(format!("temper-agent{}", env::consts::EXE_SUFFIX))
            .into_os_string(),
        OsString::from("--output-dir"),
        output_dir.clone().into_os_string(),
    ]);
    if run_status != 0 {
        return run_status;
    }
    match verify_benchmark_artifacts(&output_dir) {
        Ok(()) => {
            eprintln!(
                "temper-dev: verified harness artifacts in {}",
                output_dir.display()
            );
            0
        }
        Err(error) => {
            eprintln!("temper-dev: benchmark artifact verification failed: {error}");
            1
        }
    }
}

fn remove_old_artifacts(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    }
    .map_err(|error| format!("cannot remove {}: {error}", path.display()))
}

fn verify_benchmark_artifacts(root: &Path) -> Result<(), String> {
    let aggregate = read_json(&root.join("aggregate.json"))?;
    expect_text(&aggregate, "/benchmark", BENCHMARK_NAME)?;
    expect_text(&aggregate, "/mode", "harness")?;
    expect_at_least(&aggregate, "/outcomes/succeeded", 1)?;
    expect_at_least(&aggregate, "/metrics/mutations/median", 4)?;
    expect_exact(&aggregate, "/metrics/mutation_turns/count", 1)?;
    expect_exact(&aggregate, "/metrics/mutation_turns/median", 2)?;
    expect_exact(&aggregate, "/metrics/single_mutation_turns/count", 1)?;
    expect_exact(&aggregate, "/metrics/single_mutation_turns/median", 1)?;
    expect_exact(&aggregate, "/metrics/max_mutations_per_turn/count", 1)?;
    expect_exact(&aggregate, "/metrics/max_mutations_per_turn/median", 3)?;
    expect_at_least(&aggregate, "/metrics/validation_invalidations/median", 1)?;

    let repetition = root.join("repetitions/001");
    let run = read_json(&repetition.join("run.json"))?;
    expect_text(&run, "/benchmark/name", BENCHMARK_NAME)?;
    expect_text(&run, "/terminal/status", "succeeded")?;
    expect_at_least(&run, "/metrics/tools/by_name/read/calls", 5)?;
    expect_at_least(&run, "/metrics/tools/by_name/write/calls", 4)?;
    expect_at_least(&run, "/metrics/tools/by_name/bash/calls", 2)?;
    expect_at_least(&run, "/metrics/tools/by_name/submit_for_pr/calls", 1)?;
    expect_exact(&run, "/metrics/structure/mutation_turns", 2)?;
    expect_exact(&run, "/metrics/structure/single_mutation_turns", 1)?;
    expect_exact(&run, "/metrics/structure/max_mutations_per_turn", 3)?;
    expect_at_least(&run, "/metrics/structure/post_validation_mutations", 1)?;
    expect_at_least(&run, "/metrics/structure/revalidations", 1)?;
    expect_at_least(&run, "/validation/succeeded", 2)?;
    verify_disclaimer(&root.join("aggregate.md"))?;
    verify_disclaimer(&repetition.join("run.md"))
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn expect_text(value: &Value, pointer: &str, expected: &str) -> Result<(), String> {
    match value.pointer(pointer).and_then(Value::as_str) {
        Some(actual) if actual == expected => Ok(()),
        actual => Err(format!("{pointer} was {actual:?}; expected {expected:?}")),
    }
}

fn expect_at_least(value: &Value, pointer: &str, minimum: u64) -> Result<(), String> {
    match value.pointer(pointer).and_then(Value::as_u64) {
        Some(actual) if actual >= minimum => Ok(()),
        actual => Err(format!(
            "{pointer} was {actual:?}; expected at least {minimum}"
        )),
    }
}

fn expect_exact(value: &Value, pointer: &str, expected: u64) -> Result<(), String> {
    match value.pointer(pointer).and_then(Value::as_u64) {
        Some(actual) if actual == expected => Ok(()),
        actual => Err(format!("{pointer} was {actual:?}; expected {expected}")),
    }
}

fn verify_disclaimer(path: &Path) -> Result<(), String> {
    let report =
        fs::read_to_string(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    if report.contains(HARNESS_DISCLAIMER) {
        Ok(())
    } else {
        Err(format!("{} omitted the harness disclaimer", path.display()))
    }
}

fn has_passthrough_arg(args: &[OsString], needle: &str) -> bool {
    args.iter().any(|arg| arg == OsStr::new(needle))
}

fn run_cargo(fixed_args: &[&OsStr], passthrough_args: &[OsString]) -> i32 {
    let args = fixed_args
        .iter()
        .map(|arg| (*arg).to_os_string())
        .chain(passthrough_args.iter().cloned())
        .collect::<Vec<_>>();
    run_cargo_owned(&args)
}

fn run_cargo_owned(args: &[OsString]) -> i32 {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    eprintln!(
        "temper-dev: running cargo{}",
        display_args(args.iter().map(OsString::as_os_str))
    );

    let status = Command::new(&cargo).args(args).status();

    match status {
        Ok(status) => exit_code(status),
        Err(error) => {
            eprintln!(
                "temper-dev: failed to run {}: {error}",
                cargo.to_string_lossy()
            );
            1
        }
    }
}

fn display_args<'a>(args: impl Iterator<Item = &'a OsStr>) -> String {
    let mut display = String::new();
    for arg in args {
        let _ = write!(&mut display, " {}", arg.to_string_lossy());
    }
    display
}

fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use super::{run_dev_scenario_run, scenario_run_args};

    #[test]
    fn scenario_driver_requires_an_explicit_path() {
        assert_eq!(run_dev_scenario_run(&[]), 2);
    }

    #[test]
    fn scenario_driver_constructs_an_implicit_live_run() {
        let args = scenario_run_args(
            OsString::from("scenarios/proof"),
            Path::new("custom-target/debug/temper"),
        );
        let args = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            [
                "run",
                "-p",
                "temper-scenario-cli",
                "--",
                "run",
                "--temper-bin",
                "custom-target/debug/temper",
                "scenarios/proof",
            ]
        );
        assert!(!args.iter().any(|arg| arg == "--tier"));
    }

    #[test]
    fn cargo_config_exposes_one_scenario_run_alias() {
        let aliases = include_str!("../../../../.cargo/config.toml")
            .lines()
            .filter(|line| line.starts_with("dev-scenario-run"))
            .collect::<Vec<_>>();

        assert_eq!(
            aliases,
            [
                "dev-scenario-run = \"run --quiet -p temper-dev --bin temper-dev -- dev-scenario-run\""
            ]
        );
    }
}
