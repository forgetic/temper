// SPDX-License-Identifier: MPL-2.0

//! Focused feature-landing validation over one resolved scenario.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_scenario_core::{
    ForgeIssueKey, ResolveFeatureScenarioRequest, ResolvedFeatureScenario, ValidatorResult,
    resolve_feature_scenario,
};

use super::validate;

const EX_USAGE: u8 = 64;
const MAPPING_FILE: &str = "feature-scenario-mapping.json";
const AUDIT_FILE: &str = "focused-validation-audit.json";
const FAILURE_FILE: &str = "focused-validation-failure.txt";

const USAGE: &str = "\
Resolve and run exactly one mapped scenario from a feature-landing head.

Usage: temper-scenario validate-feature --feature <OWNER/REPO#N> --landing-base <REVISION> --source-branch <BRANCH> --pr <N> --sha <SHA> --output-dir <DIR> [--temper-bin <PATH>]

Required options:
  --feature <OWNER/REPO#N>  Feature issue mapped by the scenario
  --landing-base <REVISION> Feature-landing PR base revision
  --source-branch <BRANCH>  Feature-landing PR source branch
  --pr <N>                  Feature-landing pull request number
  --sha <SHA>               Exact checked-out PR head SHA
  --output-dir <DIR>        Directory retaining mapping and evidence artifacts

Options:
  --temper-bin <PATH>        Prebuilt standalone Temper binary
  -h, --help                 Print help

The command resolves one active mapping at HEAD, verifies its source branch and
exact head, writes feature-scenario-mapping.json, and delegates to the live-only
validator. Mapping and validation failures remain non-zero while retaining the
artifacts produced before failure.";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };

    if let Err(error) = fs::create_dir_all(&args.output_dir) {
        eprintln!(
            "temper-scenario validate-feature: create artifact directory {}: {error}",
            args.output_dir.display()
        );
        return ExitCode::FAILURE;
    }

    let request = ResolveFeatureScenarioRequest::new(
        ".",
        temper_scenario_core::DEFAULT_SCENARIOS_DIR,
        args.feature.clone(),
        &args.landing_base,
    );
    let resolved = match resolve_feature_scenario(&request) {
        Ok(resolved) => resolved,
        Err(error) => {
            return retained_failure(&args.output_dir, format!("feature mapping failed: {error}"));
        }
    };
    if let Err(error) = write_json(&args.output_dir.join(MAPPING_FILE), &resolved) {
        return retained_failure(&args.output_dir, error);
    }

    print_mapping_audit(&resolved);
    if resolved.source_branch != args.source_branch {
        return retained_failure(
            &args.output_dir,
            format!(
                "mapped source branch `{}` does not match landing PR source branch `{}`",
                resolved.source_branch, args.source_branch
            ),
        );
    }
    if resolved.head_sha != args.sha {
        return retained_failure(
            &args.output_dir,
            format!(
                "checked-out HEAD `{}` does not match landing PR head `{}`; evidence would be stale",
                resolved.head_sha, args.sha
            ),
        );
    }

    let validation_status = validate::command(&validation_args(&args, &resolved));
    let result_path = args.output_dir.join(format!(
        "validation-pr-{}-{}.json",
        args.pr_number, args.sha
    ));
    let result = match load_result(&result_path) {
        Ok(result) => Some(result),
        Err(error) if validation_status == ExitCode::SUCCESS => {
            let _ = write_audit(&args.output_dir, &resolved, None, "evidence_missing");
            return retained_failure(&args.output_dir, error);
        }
        Err(error) => {
            eprintln!("temper-scenario validate-feature: {error}");
            None
        }
    };

    let status = if validation_status == ExitCode::SUCCESS {
        "passed"
    } else {
        "failed"
    };
    if let Err(error) = write_audit(&args.output_dir, &resolved, result.as_ref(), status) {
        return retained_failure(&args.output_dir, error);
    }
    if let Some(result) = result.as_ref() {
        print_result_audit(result);
    }
    if validation_status != ExitCode::SUCCESS {
        return validation_status;
    }

    let result = result.expect("successful validation loaded a result");
    let diagnostics = exact_mapping_diagnostics(&resolved, &result);
    if diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        let _ = write_audit(
            &args.output_dir,
            &resolved,
            Some(&result),
            "contract_failed",
        );
        retained_failure(
            &args.output_dir,
            format!(
                "focused evidence contract failed: {}",
                diagnostics.join("; ")
            ),
        )
    }
}

fn validation_args(args: &Args, resolved: &ResolvedFeatureScenario) -> Vec<String> {
    let mut validation = vec![
        "--pr".to_string(),
        args.pr_number.to_string(),
        "--sha".to_string(),
        args.sha.clone(),
        "--scenario".to_string(),
        resolved.scenario_path.clone(),
        "--output-dir".to_string(),
        args.output_dir.display().to_string(),
        "--repo".to_string(),
        args.feature.repo.clone(),
        "--target-kind".to_string(),
        "feature".to_string(),
        "--target-issue".to_string(),
        args.feature.number.to_string(),
    ];
    if let Some(path) = args.temper_bin.as_ref() {
        validation.push("--temper-bin".to_string());
        validation.push(path.display().to_string());
    }
    validation
}

fn exact_mapping_diagnostics(
    mapping: &ResolvedFeatureScenario,
    result: &ValidatorResult,
) -> Vec<String> {
    let mut diagnostics = result.validate_contract();
    for (field, actual, expected) in [
        (
            "feature",
            result.feature.as_deref(),
            Some(mapping.feature.to_string()),
        ),
        (
            "mapping_id",
            result.mapping_id.as_deref(),
            Some(mapping.mapping_id.clone()),
        ),
        (
            "scenario_name",
            result.scenario_name.as_deref(),
            Some(mapping.scenario_name.clone()),
        ),
        (
            "scenario_path",
            result.scenario_path.as_deref(),
            Some(mapping.scenario_path.clone()),
        ),
        (
            "source_branch",
            result.source_branch.as_deref(),
            Some(mapping.source_branch.clone()),
        ),
        (
            "exact_head_sha",
            result.exact_head_sha.as_deref(),
            Some(mapping.head_sha.clone()),
        ),
        (
            "resolved_content_digest",
            result.resolved_content_digest.as_deref(),
            Some(mapping.digest.clone()),
        ),
    ] {
        if actual != expected.as_deref() {
            diagnostics.push(format!(
                "validator `{field}` {:?} does not match mapping {:?}",
                actual, expected
            ));
        }
    }
    let expected_plan = mapping.plan.as_ref().map(ToString::to_string);
    if result.plan != expected_plan {
        diagnostics.push(format!(
            "validator `plan` {:?} does not match mapping {:?}",
            result.plan, expected_plan
        ));
    }
    diagnostics
}

fn print_mapping_audit(mapping: &ResolvedFeatureScenario) {
    println!("focused mapping: {}", mapping.mapping_id);
    println!("focused feature: {}", mapping.feature);
    println!(
        "focused scenario: {} ({})",
        mapping.scenario_name, mapping.scenario_path
    );
    println!("focused source branch: {}", mapping.source_branch);
    println!("focused exact head: {}", mapping.head_sha);
    println!("focused content digest: {}", mapping.digest);
}

fn print_result_audit(result: &ValidatorResult) {
    println!("focused verdict: {}", result.verdict);
    println!(
        "focused binary sha256: {}",
        result
            .standalone_binary
            .as_ref()
            .map(|binary| binary.sha256.as_str())
            .unwrap_or("unavailable")
    );
    let required = result
        .validated_claims
        .iter()
        .chain(result.acceptance_criteria.iter())
        .filter(|assertion| assertion.required)
        .count();
    println!("focused required assertions: {required}");
}

fn load_result(path: &Path) -> Result<ValidatorResult, String> {
    let bytes = fs::read(path)
        .map_err(|error| format!("read validator result {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("parse validator result {}: {error}", path.display()))
}

fn write_audit(
    output_dir: &Path,
    mapping: &ResolvedFeatureScenario,
    result: Option<&ValidatorResult>,
    status: &str,
) -> Result<(), String> {
    let audit = serde_json::json!({
        "schema": "temper.scenario.focused-validation-audit.v1",
        "status": status,
        "mapping": mapping,
        "validator_result": result,
    });
    write_json(&output_dir.join(AUDIT_FILE), &audit)
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    fs::write(path, format!("{json}\n"))
        .map_err(|error| format!("write {}: {error}", path.display()))
}

fn retained_failure(output_dir: &Path, message: String) -> ExitCode {
    let path = output_dir.join(FAILURE_FILE);
    if let Err(error) = fs::write(&path, format!("{message}\n")) {
        eprintln!(
            "temper-scenario validate-feature: could not retain failure at {}: {error}",
            path.display()
        );
    }
    eprintln!("temper-scenario validate-feature: {message}");
    ExitCode::FAILURE
}

#[derive(Debug)]
struct Args {
    feature: ForgeIssueKey,
    landing_base: String,
    source_branch: String,
    pr_number: u64,
    sha: String,
    output_dir: PathBuf,
    temper_bin: Option<PathBuf>,
}

enum ParseResult {
    Help,
    Args(Args),
}

fn parse_args(args: &[String]) -> Result<ParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseResult::Help);
    }
    let allowed = [
        "--feature",
        "--landing-base",
        "--source-branch",
        "--pr",
        "--sha",
        "--output-dir",
        "--temper-bin",
    ];
    let mut values = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        if !allowed.contains(&flag) {
            return usage_error(format!("unexpected option `{flag}`"));
        }
        let Some(value) = args.get(index + 1).filter(|value| !value.starts_with("--")) else {
            return usage_error(format!("{flag} requires a value"));
        };
        if values.insert(flag, value.as_str()).is_some() {
            return usage_error(format!("duplicate {flag} option"));
        }
        index += 2;
    }

    let feature = required(&values, "--feature")?
        .parse::<ForgeIssueKey>()
        .map_err(|error| usage_error_message(format!("--feature {error}")))?;
    let pr_number = required(&values, "--pr")?
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| usage_error_message("--pr must be a positive integer"))?;
    Ok(ParseResult::Args(Args {
        feature,
        landing_base: required(&values, "--landing-base")?.to_string(),
        source_branch: required(&values, "--source-branch")?.to_string(),
        pr_number,
        sha: required(&values, "--sha")?.to_string(),
        output_dir: PathBuf::from(required(&values, "--output-dir")?),
        temper_bin: values.get("--temper-bin").map(PathBuf::from),
    }))
}

fn required<'a>(values: &'a BTreeMap<&str, &str>, flag: &str) -> Result<&'a str, ()> {
    values
        .get(flag)
        .copied()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| usage_error_message(format!("missing {flag}")))
}

fn usage_error<T>(message: String) -> Result<T, ()> {
    usage_error_message(message);
    Err(())
}

fn usage_error_message(message: impl std::fmt::Display) {
    eprintln!("temper-scenario validate-feature: {message}\n\n{USAGE}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_scenario_core::{FeatureMappingChange, FeatureScenarioBaseComparison};

    #[test]
    fn focused_validator_args_preserve_mapping_without_a_tier() {
        let args = Args {
            feature: ForgeIssueKey::new("ai/temper", 824).expect("feature key"),
            landing_base: "main".to_string(),
            source_branch: "feature/824".to_string(),
            pr_number: 42,
            sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            output_dir: PathBuf::from("artifacts/focused"),
            temper_bin: Some(PathBuf::from("custom-target/debug/temper")),
        };
        let resolved = ResolvedFeatureScenario {
            schema: "temper.scenario.feature-mapping.v1".to_string(),
            mapping_id: "ai/temper#824:proof".to_string(),
            feature: args.feature.clone(),
            plan: Some(ForgeIssueKey::new("ai/temper", 825).expect("plan key")),
            scenario_name: "proof".to_string(),
            scenario_path: "scenarios/proof".to_string(),
            manifest_path: "scenarios/proof/scenario.toml".to_string(),
            source_branch: args.source_branch.clone(),
            head_sha: args.sha.clone(),
            landing_base: args.landing_base.clone(),
            landing_base_sha: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            base_comparison: FeatureScenarioBaseComparison::New,
            content_changed_from_base: true,
            change_intent: FeatureMappingChange::New,
            digest: "sha256:proof".to_string(),
        };

        let validation = validation_args(&args, &resolved);

        assert_eq!(
            validation,
            [
                "--pr",
                "42",
                "--sha",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--scenario",
                "scenarios/proof",
                "--output-dir",
                "artifacts/focused",
                "--repo",
                "ai/temper",
                "--target-kind",
                "feature",
                "--target-issue",
                "824",
                "--temper-bin",
                "custom-target/debug/temper",
            ]
        );
        assert!(!validation.iter().any(|arg| arg == "--tier"));
    }
}
