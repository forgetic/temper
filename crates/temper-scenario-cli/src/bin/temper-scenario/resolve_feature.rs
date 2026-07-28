// SPDX-License-Identifier: MPL-2.0

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use temper_scenario_core::{
    DEFAULT_SCENARIOS_DIR, ForgeIssueKey, ResolveFeatureScenarioRequest, resolve_feature_scenario,
};

const EX_USAGE: u8 = 64;

const USAGE: &str = "\
Resolve exactly one active scenario mapped to a feature at the checked-out HEAD.\n\
\n\
Usage: temper-scenario resolve-feature --feature <OWNER/REPO#N> --landing-base <REVISION> [OPTIONS]\n\
\n\
Required options:\n\
  --feature <OWNER/REPO#N>  Feature issue to resolve\n\
  --landing-base <REVISION> Supplied feature-landing base revision\n\
\n\
Options:\n\
  --checkout <DIR>          Git checkout root (default: current directory)\n\
  --scenarios-dir <DIR>     Scenario corpus path relative to checkout (default: scenarios)\n\
  --expected-digest <SHA>   Require exact sha256:<64 lowercase hex> digest\n\
  --json-out <PATH>         Also write the CI-consumable JSON result to PATH\n\
  -h, --help                Print help\n\
\n\
Successful stdout is deterministic temper.scenario.feature-mapping.v1 JSON.";

pub(super) fn command(args: &[String]) -> ExitCode {
    let args = match parse_args(args) {
        Ok(ParseResult::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(ParseResult::Args(args)) => args,
        Err(()) => return ExitCode::from(EX_USAGE),
    };
    let mut request = ResolveFeatureScenarioRequest::new(
        &args.checkout,
        &args.scenarios_dir,
        args.feature,
        args.landing_base,
    );
    request.expected_digest = args.expected_digest;
    let resolved = match resolve_feature_scenario(&request) {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("temper-scenario resolve-feature: {error}");
            return ExitCode::FAILURE;
        }
    };
    let json = match serde_json::to_string_pretty(&resolved) {
        Ok(json) => json + "\n",
        Err(error) => {
            eprintln!("temper-scenario resolve-feature: serialize mapping result: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(path) = args.json_out.as_deref() {
        if let Err(error) = write_json(path, &json) {
            eprintln!("temper-scenario resolve-feature: {error}");
            return ExitCode::FAILURE;
        }
    }
    print!("{json}");
    ExitCode::SUCCESS
}

struct Args {
    checkout: PathBuf,
    scenarios_dir: PathBuf,
    feature: ForgeIssueKey,
    landing_base: String,
    expected_digest: Option<String>,
    json_out: Option<PathBuf>,
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
    let mut checkout = None;
    let mut scenarios_dir = None;
    let mut feature = None;
    let mut landing_base = None;
    let mut expected_digest = None;
    let mut json_out = None;
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        let Some(value) = args.get(index + 1) else {
            return usage_error(format!("{flag} requires a value"));
        };
        if value.starts_with("--") {
            return usage_error(format!("{flag} requires a value"));
        }
        let duplicate = match flag.as_str() {
            "--checkout" => checkout.replace(PathBuf::from(value)).is_some(),
            "--scenarios-dir" => scenarios_dir.replace(PathBuf::from(value)).is_some(),
            "--feature" => {
                let parsed = value.parse::<ForgeIssueKey>().map_err(|message| {
                    eprintln!("temper-scenario resolve-feature: --feature {message}\n\n{USAGE}");
                })?;
                feature.replace(parsed).is_some()
            }
            "--landing-base" => landing_base.replace(value.clone()).is_some(),
            "--expected-digest" => expected_digest.replace(value.clone()).is_some(),
            "--json-out" => json_out.replace(PathBuf::from(value)).is_some(),
            other => return usage_error(format!("unexpected option `{other}`")),
        };
        if duplicate {
            return usage_error(format!("duplicate {flag} option"));
        }
        index += 2;
    }
    let feature = feature.ok_or_else(|| {
        eprintln!("temper-scenario resolve-feature: missing --feature\n\n{USAGE}");
    })?;
    let landing_base = landing_base
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            eprintln!("temper-scenario resolve-feature: missing --landing-base\n\n{USAGE}");
        })?;
    Ok(ParseResult::Args(Args {
        checkout: checkout.unwrap_or_else(|| PathBuf::from(".")),
        scenarios_dir: scenarios_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_SCENARIOS_DIR)),
        feature,
        landing_base,
        expected_digest,
        json_out,
    }))
}

fn write_json(path: &Path, json: &str) -> Result<(), String> {
    if path.is_dir() {
        return Err(format!("--json-out is a directory: {}", path.display()));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!("create JSON output directory {}: {error}", parent.display())
        })?;
    }
    fs::write(path, json).map_err(|error| format!("write JSON output {}: {error}", path.display()))
}

fn usage_error<T>(message: String) -> Result<T, ()> {
    eprintln!("temper-scenario resolve-feature: {message}\n\n{USAGE}");
    Err(())
}
