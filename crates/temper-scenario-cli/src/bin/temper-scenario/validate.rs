// SPDX-License-Identifier: MPL-2.0

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use temper_scenario_core::{
    EvidenceKind, StructuredEvidenceEntry, ValidationAssertion, ValidationStatus,
    ValidationVerdict, ValidatorBinaryIdentity, ValidatorResult, check_scenario,
};

use super::run_context::ScenarioRunFacts;
use super::{run_evidence, runner_registry, validate_pr};

#[path = "validate/args.rs"]
mod args;
#[path = "validate/exact_head.rs"]
mod exact_head;

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

    if let Some(failure) = run_outcome.execution_failure.as_deref() {
        eprintln!("temper-scenario validate: scenario execution failed: {failure}");
    }
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
    execution_failure: Option<String>,
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

    let facts = ScenarioRunFacts::from_check_report(&report);
    let selected_runner = runner_registry::select_runner(manifest)
        .map_err(|error| RunError::Message(error.message(&report.scenario_path)))?;
    let temper_bin = resolve_temper_binary(args).map_err(RunError::Message)?;

    let evidence_context =
        run_evidence::RunEvidenceContext::from_check_report(&report, &facts, &selected_runner);
    let started = Instant::now();
    let mut artifact = match selected_runner.run_and_print(
        &report.scenario_path,
        manifest_path,
        &facts,
        temper_bin.as_deref(),
        &evidence_context,
    ) {
        Ok(artifact) => artifact,
        Err(message) => {
            let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            evidence_context.failure_artifact(message, elapsed)
        }
    };
    let execution_failed = artifact.verdict == run_evidence::RunEvidenceVerdict::Failed;
    let execution_failure = artifact
        .execution
        .as_ref()
        .and_then(|execution| execution.failure.clone());

    let assertion_evidence =
        run_evidence::evaluate_manifest_assertions(manifest_path, &artifact)
            .map_err(|error| RunError::Message(format!("evaluate manifest assertions: {error}")))?;
    if let Some(assertions) = assertion_evidence {
        artifact.record_assertions(assertions);
    }

    if !execution_failed {
        run_evidence::append_script_assertions(manifest_path, &mut artifact, &args.output_dir)
            .map_err(|error| RunError::Message(format!("evaluate script assertions: {error}")))?;
    } else {
        artifact.limitations.push(
            "After-convergence script assertions were not executed because convergence did not complete."
                .to_string(),
        );
    }

    let assertions_failed = execution_failed
        || artifact
            .assertions
            .as_ref()
            .is_some_and(|assertions| assertions.has_failures());
    if let Some(assertions) = artifact.assertions.as_ref() {
        run_evidence::print_assertions(assertions);
    }

    if !artifact
        .artifacts
        .artifact_paths
        .iter()
        .any(|path| path == &evidence_path.display().to_string())
    {
        artifact
            .artifacts
            .artifact_paths
            .push(evidence_path.display().to_string());
    }
    let evidence_path = artifact
        .write_to_path(evidence_path)
        .map_err(RunError::Message)?;
    Ok(RunOutcome {
        evidence_path,
        assertions_failed,
        execution_failure,
    })
}

fn resolve_temper_binary(args: &Args) -> Result<Option<PathBuf>, String> {
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
    let mut result = ValidatorResult::from_validation_report(report.clone(), args.repo.clone());
    enrich_validator_result(&mut result, evidence_path)?;
    if let (Some(kind), Some(issue)) = (args.target_kind.as_deref(), args.target_issue) {
        exact_head::apply_issue_target(&mut result, kind, issue, &args.repo);
    }
    write_json_result(&json_path, &result)?;

    Ok(ValidationArtifacts {
        markdown_path,
        json_path,
        report,
    })
}

fn enrich_validator_result(
    result: &mut ValidatorResult,
    evidence_path: &Path,
) -> Result<(), ValidationError> {
    let loaded =
        run_evidence::load_run_evidence(evidence_path).map_err(ValidationError::RunEvidence)?;
    let artifact = loaded.artifact;
    result.feature = artifact.scenario.feature.clone();
    result.plan = artifact.scenario.plan.clone();
    result.mapping_id = artifact.scenario.mapping_id.clone();
    result.scenario_name = artifact
        .scenario
        .mapped_scenario
        .clone()
        .or_else(|| Some(artifact.scenario.name.clone()));
    result.scenario_path = Some(artifact.scenario.scenario_path.clone());
    result.source_branch = artifact.scenario.source_branch.clone();
    result.exact_head_sha = artifact.scenario.checkout_head_sha.clone();
    result.resolved_content_digest = artifact.scenario.resolved_content_digest.clone();
    result.standalone_binary = artifact
        .binary
        .as_ref()
        .map(|binary| ValidatorBinaryIdentity {
            path: binary.path.clone(),
            sha256: binary.sha256.clone(),
            size_bytes: binary.size_bytes,
        });
    result.duration_ms = artifact
        .execution
        .as_ref()
        .map(|execution| execution.total_duration_ms)
        .or_else(|| {
            artifact
                .convergence
                .as_ref()
                .and_then(|convergence| convergence.total_elapsed_ms)
        });

    push_unique_string(
        &mut result.retained_paths,
        loaded.path.display().to_string(),
    );
    for path in artifact
        .artifacts
        .log_paths
        .iter()
        .chain(artifact.artifacts.artifact_paths.iter())
    {
        push_unique_string(&mut result.retained_paths, path.clone());
    }
    for limitation in &artifact.limitations {
        push_unique_string(&mut result.limitations, limitation.clone());
    }
    result.follow_up_intent = artifact.follow_up_intent.clone();

    result.verdict = match artifact.verdict {
        run_evidence::RunEvidenceVerdict::Failed => ValidationVerdict::Failed,
        run_evidence::RunEvidenceVerdict::Inconclusive
            if result.verdict != ValidationVerdict::Failed =>
        {
            ValidationVerdict::Inconclusive
        }
        _ => result.verdict,
    };

    if let Some(assertions) = artifact.assertions {
        for assertion in assertions.results {
            let evidence_id = format!("assertion:{}", assertion.id);
            let status = match assertion.status.as_str() {
                "passed" => ValidationStatus::Satisfied,
                "failed" | "timed_out" => ValidationStatus::Failed,
                _ => ValidationStatus::Unproven,
            };
            let mut evidence = StructuredEvidenceEntry::new(
                evidence_id.clone(),
                EvidenceKind::ScenarioRun,
                format!(
                    "Scenario assertion `{}` completed with status `{}`.",
                    assertion.id, assertion.status
                ),
            );
            if !assertion.details.is_empty() {
                evidence.details = Some(assertion.details.join("\n"));
            }
            evidence.artifact_path = assertion
                .status_path
                .clone()
                .or(assertion.context_path.clone())
                .or(assertion.stdout_path.clone());
            result.evidence.push(evidence);
            result.acceptance_criteria.push(ValidationAssertion {
                description: assertion.description,
                required: assertion.required,
                status,
                evidence_refs: vec![evidence_id],
            });
        }
    }
    Ok(())
}

fn push_unique_string(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
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
