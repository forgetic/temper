// SPDX-License-Identifier: MPL-2.0

mod findings;
mod options;
mod output;

use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver};
use temper_config::{Finding, LoadedPaths};

use crate::load_for_with_secret_validation;
use findings::{add_offline_findings, error_finding, scoped_findings};
use options::{CheckAction, CheckOptions, parse_check_args};
use output::{has_blocking_findings, print_validation_json};

pub(crate) use output::{has_error_findings, print_validation_human};

/// Everything top-level `temper check` needs, with no ambient environment access.
pub struct CheckInputs<'a> {
    /// The program arguments after `check`.
    pub args: Vec<String>,
    /// Global file-location options parsed before `check`.
    pub options: LoadOptions,
    /// Global output format parsed before `check`.
    pub format: OutputFormat,
    /// The injected environment snapshot.
    pub env: &'a EnvMap,
    /// The injected base directories for default-location discovery.
    pub paths: &'a PathResolver,
}

pub const CHECK_USAGE: &str = "\
Validate the resolved Temper config and credentials offline.

Usage: temper [GLOBAL OPTIONS] check [OPTIONS]

Options:
      --component <standalone|engine|worker|trigger>
                           Component scope to validate (default: standalone)
      --pool <NAME>        Worker pool to validate with --component worker
      --strict             Treat notes and warnings as failures
      --online             Accepted for future provider/reachability checks; currently offline only
  -h, --help               Print help

Global options:
  -c, --config <DIR|FILE>   Path to configuration file or bundle directory
      --secrets <DIR|FILE>  Explicit secret source directory or credentials.toml
      --format <human|json> Output format (default: human)";

pub fn check(inputs: CheckInputs) -> ExitCode {
    let CheckInputs {
        args,
        options,
        format,
        env,
        paths,
    } = inputs;
    match parse_check_args(&args) {
        Ok(CheckAction::Help) => {
            println!("{CHECK_USAGE}");
            ExitCode::SUCCESS
        }
        Ok(CheckAction::Run(check_options)) => {
            run_check(&options, format, env, paths, check_options)
        }
        Err(error) => {
            eprintln!("temper check: {error}\n\n{CHECK_USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn run_check(
    options: &LoadOptions,
    format: OutputFormat,
    env: &EnvMap,
    paths: &PathResolver,
    check_options: CheckOptions,
) -> ExitCode {
    let report = validation_report(options, env, paths, &check_options);
    match format {
        OutputFormat::Human => print_validation_human(&report.loaded, &report.findings),
        OutputFormat::Json => {
            if let Err(error) =
                print_validation_json(&report.loaded, &report.findings, &check_options)
            {
                eprintln!("temper check: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    if report.load_failed || has_blocking_findings(&report.findings, check_options.strict) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Debug, Clone)]
struct ValidationReport {
    loaded: LoadedPaths,
    findings: Vec<Finding>,
    load_failed: bool,
}

fn validation_report(
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
    check_options: &CheckOptions,
) -> ValidationReport {
    match load_for_with_secret_validation(options, env, paths, false) {
        Ok((resolved, loaded)) => {
            let mut findings = scoped_findings(&resolved, check_options);
            add_offline_findings(&resolved, check_options, &mut findings);
            ValidationReport {
                loaded,
                findings,
                load_failed: false,
            }
        }
        Err(error) => ValidationReport {
            loaded: LoadedPaths::default(),
            findings: vec![error_finding(error.to_string())],
            load_failed: true,
        },
    }
}
