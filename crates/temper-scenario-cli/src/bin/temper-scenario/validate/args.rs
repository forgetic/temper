// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

const DEFAULT_REPO: &str = "ai/temper";

pub(super) const USAGE: &str = "\
Run a validation bundle and render final PR validation artifacts from structured evidence.

Usage: temper-scenario validate --pr <N> --sha <SHA> --scenario <PATH> --output-dir <DIR> [--temper-bin <PATH>] [--repo <OWNER/NAME>] [--target-kind <KIND> --target-issue <N>]

Options:
  --pr <N>               Pull request/report number under validation
  --sha <SHA>            Exact commit SHA under validation
  --target-kind <KIND>   Workflow artifact kind for an issue validation target
  --target-issue <N>     Workflow issue number paired with --target-kind
  --scenario <PATH>      Scenario directory or manifest file to run
  --temper-bin <PATH>    Standalone `temper` binary; when omitted, the validator resolves an existing binary or builds `cargo build --bin temper`
  --output-dir <DIR>     Artifact directory for run evidence, hook logs, Markdown report, and JSON result
  --artifact-dir <DIR>   Alias for --output-dir
  --repo <OWNER/NAME>    Repository recorded in the structured validator result (default: ai/temper)
  -h, --help             Print help

This command is the cohesive validator workflow: it runs the selected manifest
bundle on the validation-grade live stack, writes structured run evidence to the
artifact directory, invokes the same validate-pr report builder against that
evidence, and retains Markdown/JSON validation output. Lower-level `run
--evidence-out` and `validate-pr --run-evidence` remain available for manual
use. The `manifest` runner is the only public runner and always uses real
Forgejo, host `forgejo-runner` CI, standalone Temper, and Jig fake-LLM agents.";

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct Args {
    pub(super) pr_number: u64,
    pub(super) sha: String,
    pub(super) scenario: PathBuf,
    pub(super) temper_bin: Option<PathBuf>,
    pub(super) output_dir: PathBuf,
    pub(super) repo: String,
    pub(super) target_kind: Option<String>,
    pub(super) target_issue: Option<u64>,
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
    let mut temper_bin = None;
    let mut output_dir = None;
    let mut repo = None;
    let mut target_kind = None;
    let mut target_issue = None;
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
        if arg == "--target-kind" {
            let value = flag_value(args, index, "--target-kind")?;
            set_target_kind(&mut target_kind, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--target-kind") {
            set_target_kind(&mut target_kind, value)?;
            index += 1;
            continue;
        }
        if arg == "--target-issue" {
            let value = flag_value(args, index, "--target-issue")?;
            set_target_issue(&mut target_issue, value)?;
            index += 2;
            continue;
        }
        if let Some(value) = inline_flag_value(arg, "--target-issue") {
            set_target_issue(&mut target_issue, value)?;
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

    if target_kind.is_some() != target_issue.is_some() {
        eprintln!(
            "temper-scenario validate: --target-kind and --target-issue must be provided together\n\n{USAGE}"
        );
        return Err(());
    }

    Ok(ParseResult::Args(Args {
        pr_number,
        sha,
        scenario,
        temper_bin,
        output_dir,
        repo: repo.unwrap_or_else(|| DEFAULT_REPO.to_string()),
        target_kind,
        target_issue,
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

fn set_target_kind(target_kind: &mut Option<String>, value: &str) -> Result<(), ()> {
    let value = value.trim();
    if value.is_empty() {
        eprintln!("temper-scenario validate: --target-kind must not be empty\n\n{USAGE}");
        return Err(());
    }
    if target_kind.replace(value.to_string()).is_some() {
        eprintln!("temper-scenario validate: duplicate --target-kind option\n\n{USAGE}");
        return Err(());
    }
    Ok(())
}

fn set_target_issue(target_issue: &mut Option<u64>, value: &str) -> Result<(), ()> {
    let parsed = value.parse::<u64>().ok().filter(|number| *number > 0);
    let Some(parsed) = parsed else {
        eprintln!(
            "temper-scenario validate: --target-issue must be a positive integer: {value}\n\n{USAGE}"
        );
        return Err(());
    };
    if target_issue.replace(parsed).is_some() {
        eprintln!("temper-scenario validate: duplicate --target-issue option\n\n{USAGE}");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn required_args() -> Vec<String> {
        [
            "--pr",
            "7",
            "--sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--scenario",
            "scenarios/exact-head",
            "--output-dir",
            "target/evidence",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn workflow_issue_target_is_typed_and_requires_both_flags() {
        let mut args = required_args();
        args.extend(["--target-kind".to_string(), "plan".to_string()]);
        assert!(parse_args(&args).is_err());

        args.extend(["--target-issue".to_string(), "7".to_string()]);
        let ParseResult::Args(parsed) = parse_args(&args).expect("target parses") else {
            panic!("expected parsed arguments");
        };
        assert_eq!(parsed.target_kind.as_deref(), Some("plan"));
        assert_eq!(parsed.target_issue, Some(7));
    }
}
