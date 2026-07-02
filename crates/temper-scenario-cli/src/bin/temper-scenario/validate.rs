// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use temper_scenario_core::{ValidationVerdict, ValidatorResult, check_scenario};

use super::run_context::ScenarioRunFacts;
use super::{run_evidence, runner_registry, validate_pr};

#[path = "validate/args.rs"]
mod args;

use args::{Args, ParseResult};

const EX_USAGE: u8 = 64;
const RUN_EVIDENCE_FILE: &str = "run-evidence.json";
const PRIMARY_TEMPER_BIN_ENV: &str = "TEMPER_SCENARIO_TEMPER_BIN";
const COMPAT_TEMPER_BIN_ENV: &str = "TEMPER_BIN";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match args::parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{}", args::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    if let Err(error) = fs::create_dir_all(&args.output_dir) {
        eprintln!(
            "temper-scenario validate: create artifact directory {}: {error}",
            args.output_dir.display()
        );
        return ExitCode::FAILURE;
    }

    let evidence_path = args.output_dir.join(RUN_EVIDENCE_FILE);
    let run_outcome = match run_scenario_evidence(&args, &evidence_path) {
        Ok(outcome) => outcome,
        Err(RunError::InvalidScenario(report)) => {
            eprintln!(
                "temper-scenario validate: scenario check failed for {}",
                super::display_path(&report.scenario_path)
            );
            super::print_report_diagnostics(&report);
            return ExitCode::FAILURE;
        }
        Err(RunError::Message(message)) => {
            eprintln!("temper-scenario validate: {message}");
            return ExitCode::FAILURE;
        }
    };

    let validation = match write_validation_artifacts(&args, &run_outcome.evidence_path) {
        Ok(validation) => validation,
        Err(ValidationError::InvalidScenario(report)) => {
            eprintln!(
                "temper-scenario validate: scenario check failed while rendering report for {}",
                super::display_path(&report.scenario_path)
            );
            super::print_report_diagnostics(&report);
            return ExitCode::FAILURE;
        }
        Err(ValidationError::RunEvidence(message)) => {
            eprintln!("temper-scenario validate: {message}");
            return ExitCode::FAILURE;
        }
        Err(ValidationError::Write(message)) => {
            eprintln!("temper-scenario validate: {message}");
            return ExitCode::FAILURE;
        }
    };

    println!("validation artifacts: {}", args.output_dir.display());
    println!("run evidence: {}", run_outcome.evidence_path.display());
    println!("validation report: {}", validation.markdown_path.display());
    println!("validation result: {}", validation.json_path.display());

    if run_outcome.assertions_failed {
        eprintln!(
            "temper-scenario validate: scenario assertions failed; evidence and validation report were retained"
        );
    }

    if validation.report.verdict == ValidationVerdict::Failed || run_outcome.assertions_failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Debug)]
struct RunOutcome {
    evidence_path: PathBuf,
    assertions_failed: bool,
}

#[derive(Debug)]
enum RunError {
    InvalidScenario(Box<temper_scenario_core::CheckReport>),
    Message(String),
}

fn run_scenario_evidence(args: &Args, evidence_path: &Path) -> Result<RunOutcome, RunError> {
    let report = check_scenario(&args.scenario);
    if !report.is_valid() {
        return Err(RunError::InvalidScenario(Box::new(report)));
    }

    let manifest = report.manifest.as_ref().ok_or_else(|| {
        RunError::Message(format!(
            "no runnable scenario manifest found at {}",
            super::display_path(&args.scenario)
        ))
    })?;
    let manifest_path = report.manifest_path.as_deref().ok_or_else(|| {
        RunError::Message(format!(
            "no scenario manifest found at {}",
            super::display_path(&report.scenario_path)
        ))
    })?;

    let facts = ScenarioRunFacts::from_check_report(&report, args.tier);
    let selected_runner = runner_registry::select_runner(manifest, args.tier)
        .map_err(|error| RunError::Message(error.message(&report.scenario_path)))?;
    let temper_bin = resolve_temper_binary(&selected_runner, args).map_err(RunError::Message)?;

    let evidence_context =
        run_evidence::RunEvidenceContext::from_check_report(&report, &facts, &selected_runner);
    let mut artifact = selected_runner
        .run_and_print(
            &report.scenario_path,
            manifest_path,
            &facts,
            args.tier,
            temper_bin.as_deref(),
            &evidence_context,
        )
        .map_err(RunError::Message)?;

    let assertion_evidence =
        run_evidence::evaluate_manifest_assertions(manifest_path, &artifact)
            .map_err(|error| RunError::Message(format!("evaluate manifest assertions: {error}")))?;
    if let Some(assertions) = assertion_evidence {
        artifact.assertions = Some(assertions);
    }

    run_evidence::append_script_assertions(manifest_path, &mut artifact, &args.output_dir)
        .map_err(|error| RunError::Message(format!("evaluate script assertions: {error}")))?;

    let assertions_failed = artifact
        .assertions
        .as_ref()
        .is_some_and(|assertions| assertions.has_failures());
    if let Some(assertions) = artifact.assertions.as_ref() {
        run_evidence::print_assertions(assertions);
    }

    let evidence_path = artifact
        .write_to_path(evidence_path)
        .map_err(RunError::Message)?;
    Ok(RunOutcome {
        evidence_path,
        assertions_failed,
    })
}

fn resolve_temper_binary(
    selected_runner: &runner_registry::SelectedRunner,
    args: &Args,
) -> Result<Option<PathBuf>, String> {
    if !selected_runner.requires_standalone_temper(args.tier) {
        return Ok(args.temper_bin.clone());
    }
    if let Some(path) = args.temper_bin.as_ref() {
        return Ok(Some(path.clone()));
    }
    if let Some(path) = resolve_env_temper_binary()? {
        println!("resolved temper binary: {}", path.display());
        return Ok(Some(path));
    }
    if let Some(path) = resolve_fallback_temper_binary()? {
        println!("resolved temper binary: {}", path.display());
        return Ok(Some(path));
    }

    let path = build_temper_binary()?;
    println!("built temper binary: {}", path.display());
    Ok(Some(path))
}

fn resolve_env_temper_binary() -> Result<Option<PathBuf>, String> {
    for name in [PRIMARY_TEMPER_BIN_ENV, COMPAT_TEMPER_BIN_ENV] {
        let Some(raw) = env::var_os(name) else {
            continue;
        };
        if raw.is_empty() {
            eprintln!(
                "temper-scenario validate: {name} is set but empty; resolving or building standalone `temper` instead"
            );
            continue;
        }
        let path = PathBuf::from(raw);
        if path.is_file() {
            return fs::canonicalize(&path).map(Some).map_err(|error| {
                format!(
                    "canonicalize {name} standalone temper binary {}: {error}",
                    path.display()
                )
            });
        }
        eprintln!(
            "temper-scenario validate: {name}={} is not a file; resolving or building standalone `temper` instead",
            path.display()
        );
    }
    Ok(None)
}

fn resolve_fallback_temper_binary() -> Result<Option<PathBuf>, String> {
    for candidate in fallback_temper_candidates() {
        if candidate.is_file() {
            return fs::canonicalize(&candidate).map(Some).map_err(|error| {
                format!(
                    "canonicalize fallback standalone temper binary {}: {error}",
                    candidate.display()
                )
            });
        }
    }
    Ok(None)
}

fn build_temper_binary() -> Result<PathBuf, String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    eprintln!("temper-scenario validate: running cargo build --bin temper");
    let status = Command::new(&cargo)
        .args(["build", "--bin", "temper"])
        .status()
        .map_err(|error| format!("failed to run {}: {error}", cargo.to_string_lossy()))?;
    if !status.success() {
        return Err(format!("cargo build --bin temper failed with {status}"));
    }

    let binary = target_debug_temper_binary();
    if !binary.is_file() {
        return Err(format!(
            "cargo build --bin temper completed but no standalone `temper` binary was found at {}",
            binary.display()
        ));
    }
    fs::canonicalize(&binary).map_err(|error| {
        format!(
            "canonicalize built standalone temper binary {}: {error}",
            binary.display()
        )
    })
}

fn fallback_temper_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(current_exe) = env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            push_temper_in_dir(&mut candidates, dir);
            if dir
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new("deps"))
            {
                if let Some(parent) = dir.parent() {
                    push_temper_in_dir(&mut candidates, parent);
                }
            }
        }
    }
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        push_temper_in_dir(&mut candidates, &target_dir.join("debug"));
        push_temper_in_dir(&mut candidates, &target_dir.join("release"));
    }
    if let Ok(current_dir) = env::current_dir() {
        push_temper_in_dir(&mut candidates, &current_dir.join("target/debug"));
        push_temper_in_dir(&mut candidates, &current_dir.join("target/release"));
        if let Some(root) = find_repo_root(&current_dir) {
            push_temper_in_dir(&mut candidates, &root.join("target/debug"));
            push_temper_in_dir(&mut candidates, &root.join("target/release"));
        }
    }
    candidates
}

fn target_debug_temper_binary() -> PathBuf {
    if let Some(target_dir) = env::var_os("CARGO_TARGET_DIR") {
        return PathBuf::from(target_dir)
            .join("debug")
            .join(format!("temper{}", env::consts::EXE_SUFFIX));
    }
    let target_dir = env::current_dir()
        .ok()
        .and_then(|current_dir| find_repo_root(&current_dir))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("target");
    target_dir
        .join("debug")
        .join(format!("temper{}", env::consts::EXE_SUFFIX))
}

fn push_temper_in_dir(candidates: &mut Vec<PathBuf>, dir: &Path) {
    push_unique(
        candidates,
        dir.join(format!("temper{}", env::consts::EXE_SUFFIX)),
    );
}

fn push_unique(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start)
    } else {
        start
    };
    loop {
        if current.join("Cargo.toml").is_file() && current.join("scenarios").is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}

#[derive(Debug)]
struct ValidationArtifacts {
    markdown_path: PathBuf,
    json_path: PathBuf,
    report: temper_scenario_core::ValidationReport,
}

#[derive(Debug)]
enum ValidationError {
    InvalidScenario(Box<temper_scenario_core::CheckReport>),
    RunEvidence(String),
    Write(String),
}

fn write_validation_artifacts(
    args: &Args,
    evidence_path: &Path,
) -> Result<ValidationArtifacts, ValidationError> {
    let markdown_path =
        validate_pr::validation_report_path(&args.output_dir, args.pr_number, &args.sha);
    let report_args = validate_pr::Args {
        pr_number: args.pr_number,
        sha: args.sha.clone(),
        scenario: Some(args.scenario.clone()),
        run_evidence: Some(evidence_path.to_path_buf()),
        tier: args.tier,
        tier_explicit: args.tier_explicit,
        temper_bin: None,
        output_dir: args.output_dir.clone(),
    };
    let report =
        validate_pr::build_report(&report_args, &markdown_path).map_err(|error| match error {
            validate_pr::Error::InvalidScenario(report) => ValidationError::InvalidScenario(report),
            validate_pr::Error::RunEvidence(message) => ValidationError::RunEvidence(message),
        })?;
    validate_pr::write_report(&markdown_path, &report).map_err(ValidationError::Write)?;

    let json_path = markdown_path.with_extension("json");
    let result = ValidatorResult::from_validation_report(report.clone(), args.repo.clone());
    write_json_result(&json_path, &result)?;

    Ok(ValidationArtifacts {
        markdown_path,
        json_path,
        report,
    })
}

fn write_json_result(path: &Path, result: &ValidatorResult) -> Result<(), ValidationError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ValidationError::Write(format!(
                "failed to create JSON output directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let json = serde_json::to_string_pretty(result).map_err(|error| {
        ValidationError::Write(format!(
            "failed to serialize validation result JSON: {error}"
        ))
    })?;
    fs::write(path, format!("{json}\n")).map_err(|error| {
        ValidationError::Write(format!(
            "failed to write validation result JSON {}: {error}",
            path.display()
        ))
    })
}
