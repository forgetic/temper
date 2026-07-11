// SPDX-License-Identifier: MPL-2.0

use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver};

use super::report::print_report;
use super::run_plan;

/// `temper plan [OPTIONS]` usage.
pub const PLAN_USAGE: &str = "\
Preview a temper deployment bundle without mutating the forge.

Loads config.toml + credentials.toml + workflow, validates them, builds the same
forge provisioning model that `temper apply` uses, then inspects every configured
repository with read-only calls. Secret values are never printed.

Usage: temper [GLOBAL OPTIONS] plan [OPTIONS]

Options:
  --existing-repo         Supported compatibility behavior: require every
                          configured repo to already exist
  -h, --help              Print help

Global options:
  -c, --config <DIR|FILE>      Path to configuration file or bundle directory
      --secrets <DIR|FILE>     Explicit credentials.toml
      --format <human|json>    Output format";

#[derive(Debug, Clone, Default)]
struct ParsedPlanArgs {
    help: bool,
    options: LoadOptions,
    existing_repo: bool,
}

/// Everything `temper plan` needs beyond the loaded bundle.
#[derive(Debug, Clone, Default)]
pub struct PlanOptions {
    /// Where to read `config.toml` and `credentials.toml`.
    pub options: LoadOptions,
    /// Match `temper apply --existing-repo` for every configured repository.
    pub existing_repo: bool,
    /// Requested output format.
    pub format: OutputFormat,
    /// Environment snapshot used for path expansion.
    pub env: EnvMap,
    /// Base directories used to resolve default config locations.
    pub paths: PathResolver,
}

/// The unified binary's `temper plan` entry point.
pub fn plan_main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
    format: OutputFormat,
) -> ExitCode {
    let parsed = match parse_plan_args(args, options) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper plan: {error}\n\n{PLAN_USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{PLAN_USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = PlanOptions {
        options: parsed.options,
        existing_repo: parsed.existing_repo,
        format,
        env: env.clone(),
        paths: paths.clone(),
    };
    match run_plan(&opts) {
        Ok(report) => {
            if let Err(error) = print_report(&report, format) {
                eprintln!("temper plan: {error}");
                return ExitCode::FAILURE;
            }
            if report.has_error_findings() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("temper plan: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_plan_args(args: Vec<String>, options: LoadOptions) -> Result<ParsedPlanArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedPlanArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedPlanArgs {
        options,
        ..Default::default()
    };
    for arg in args {
        match arg.as_str() {
            "--existing-repo" => parsed.existing_repo = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}
