// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use crate::run_context::ScenarioTier;

const DEFAULT_REPO: &str = "ai/temper";

pub(super) const USAGE: &str = "\
Run a validation bundle and render final PR validation artifacts from structured evidence.

Usage: temper-scenario validate --pr <N> --sha <SHA> --scenario <PATH> --output-dir <DIR> [--tier <live|hermetic>] [--temper-bin <PATH>] [--repo <OWNER/NAME>]

Options:
  --pr <N>               Pull request number under validation
  --sha <SHA>            Merged/main commit SHA under validation
  --scenario <PATH>      Scenario directory or manifest file to run
  --tier <live|hermetic> Confidence tier to run (default: live)
  --temper-bin <PATH>    Standalone `temper` binary for live runners; when omitted, live runners resolve an existing binary or build `cargo build --bin temper`
  --output-dir <DIR>     Artifact directory for run evidence, hook logs, Markdown report, and JSON result
  --artifact-dir <DIR>   Alias for --output-dir
  --repo <OWNER/NAME>    Repository recorded in the structured validator result (default: ai/temper)
  -h, --help             Print help

This command is the cohesive validator workflow: it runs the selected manifest
bundle on the validation-grade live stack, writes structured run evidence to the
artifact directory, invokes the same validate-pr report builder against that
evidence, and retains Markdown/JSON validation output. Lower-level `run
--evidence-out` and `validate-pr --run-evidence` remain available for manual
use. The `manifest` runner is the only public runner; explicit `--tier hermetic`
requests are rejected instead of substituting MemoryForge or in-process paths.";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct Args {
    pub(super) pr_number: u64,
    pub(super) sha: String,
    pub(super) scenario: PathBuf,
    pub(super) tier: ScenarioTier,
    pub(super) tier_explicit: bool,
    pub(super) temper_bin: Option<PathBuf>,
    pub(super) output_dir: PathBuf,
    pub(super) repo: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum ParseResult {
    Help,
    Args(Args),
}

pub(super) fn parse_args(args: &[String]) -> Result<ParseResult, ()> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help" | "help"))
    {
        return Ok(ParseResult::Help);
    }

    let mut pr_number = None;
    let mut sha = None;
    let mut scenario = None;
    let mut tier = None;
    let mut temper_bin = None;
    let mut output_dir = None;
    let mut repo = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--pr" {
            let value = flag_value(args, index, "--pr")?;
            set_pr_number(&mut pr_number, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--pr") {
            set_pr_number(&mut pr_number, value)?;
            index += 1;
            continue;
        }
        if arg == "--sha" {
            let value = flag_value(args, index, "--sha")?;
            set_sha(&mut sha, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--sha") {
            set_sha(&mut sha, value)?;
            index += 1;
            continue;
        }
        if arg == "--scenario" {
            let value = flag_value(args, index, "--scenario")?;
            set_path(&mut scenario, value, "--scenario")?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--scenario") {
            set_path(&mut scenario, value, "--scenario")?;
            index += 1;
            continue;
        }
        if arg == "--tier" {
            let value = flag_value(args, index, "--tier")?;
            set_tier(&mut tier, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--tier") {
            set_tier(&mut tier, value)?;
            index += 1;
            continue;
        }
        if arg == "--temper-bin" {
            let value = flag_value(args, index, "--temper-bin")?;
            set_path(&mut temper_bin, value, "--temper-bin")?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--temper-bin") {
            set_path(&mut temper_bin, value, "--temper-bin")?;
            index += 1;
            continue;
        }
        if arg == "--output-dir" || arg == "--artifact-dir" {
            let flag = arg.as_str();
            let value = flag_value(args, index, flag)?;
            set_path(&mut output_dir, value, flag)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--output-dir") {
            set_path(&mut output_dir, value, "--output-dir")?;
            index += 1;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--artifact-dir") {
            set_path(&mut output_dir, value, "--artifact-dir")?;
            index += 1;
            continue;
        }
        if arg == "--repo" {
            let value = flag_value(args, index, "--repo")?;
            set_repo(&mut repo, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--repo") {
            set_repo(&mut repo, value)?;
            index += 1;
            continue;
        }
        eprintln!("temper-scenario validate: unexpected argument `{arg}`\n\n{USAGE}");
        return Err(());
    }

    let Some(pr_number) = pr_number else {
        eprintln!("temper-scenario validate: missing --pr\n\n{USAGE}");
        return Err(());
    };
    let Some(sha) = sha else {
        eprintln!("temper-scenario validate: missing --sha\n\n{USAGE}");
        return Err(());
    };
    let Some(scenario) = scenario else {
        eprintln!("temper-scenario validate: missing --scenario\n\n{USAGE}");
        return Err(());
    };
    let Some(output_dir) = output_dir else {
        eprintln!("temper-scenario validate: missing --output-dir\n\n{USAGE}");
        return Err(());
    };

    let tier_explicit = tier.is_some();
    Ok(ParseResult::Args(Args {
        pr_number,
        sha,
        scenario,
        tier: tier.unwrap_or(ScenarioTier::Live),
        tier_explicit,
        temper_bin,
        output_dir,
        repo: repo.unwrap_or_else(|| DEFAULT_REPO.to_string()),
    }))
}

fn flag_value<'a>(args: &'a [String], index: usize, flag: &str) -> Result<&'a str, ()> {
    let Some(value) = args.get(index + 1) else {
        eprintln!("temper-scenario validate: {flag} requires a value\n\n{USAGE}");
        return Err(());
    };
    if value.starts_with("--") {
        eprintln!("temper-scenario validate: {flag} requires a value\n\n{USAGE}");
        return Err(());
    }
    Ok(value)
}

fn inline_flag_value<'a>(arg: &'a str, flag: &str) -> Option<&'a str> {
    arg.strip_prefix(flag)
        .and_then(|suffix| suffix.strip_prefix('='))
}

fn set_pr_number(pr_number: &mut Option<u64>, value: &str) -> Result<(), ()> {
    let parsed = value.parse::<u64>().ok().filter(|number| *number > 0);
    let Some(parsed) = parsed else {
        eprintln!("temper-scenario validate: --pr must be a positive integer: {value}\n\n{USAGE}");
        return Err(());
    };
    if pr_number.replace(parsed).is_some() {
        eprintln!("temper-scenario validate: duplicate --pr option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_sha(sha: &mut Option<String>, value: &str) -> Result<(), ()> {
    if value.trim().is_empty() {
        eprintln!("temper-scenario validate: --sha must not be empty\n\n{USAGE}");
        return Err(());
    }
    if sha.replace(value.to_string()).is_some() {
        eprintln!("temper-scenario validate: duplicate --sha option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_path(path: &mut Option<PathBuf>, value: &str, flag: &str) -> Result<(), ()> {
    if value.is_empty() {
        eprintln!("temper-scenario validate: {flag} requires a value\n\n{USAGE}");
        return Err(());
    }
    if path.replace(PathBuf::from(value)).is_some() {
        eprintln!("temper-scenario validate: duplicate {flag} option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_tier(tier: &mut Option<ScenarioTier>, value: &str) -> Result<(), ()> {
    let Some(parsed) = ScenarioTier::parse(value) else {
        eprintln!(
            "temper-scenario validate: unknown --tier `{value}` (expected live or hermetic)\n\n{USAGE}"
        );
        return Err(());
    };
    if tier.replace(parsed).is_some() {
        eprintln!("temper-scenario validate: duplicate --tier option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_repo(repo: &mut Option<String>, value: &str) -> Result<(), ()> {
    if value.trim().is_empty() {
        eprintln!("temper-scenario validate: --repo must not be empty\n\n{USAGE}");
        return Err(());
    }
    if repo.replace(value.to_string()).is_some() {
        eprintln!("temper-scenario validate: duplicate --repo option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}
