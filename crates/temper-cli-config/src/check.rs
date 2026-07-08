// SPDX-License-Identifier: MPL-2.0

mod finding;
mod findings;
mod online;
mod options;
mod output;

use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver};
use temper_config::{Finding, LoadedPaths};

use crate::load_for_with_secret_validation;
use finding::{CheckCategory, CheckFinding};
use findings::{add_offline_findings, error_finding, scoped_findings};
use online::add_online_findings;
use options::{CheckAction, CheckOptions, parse_check_args};
use output::{has_blocking_findings, print_validation_json};

pub(crate) fn print_validation_human(loaded: &LoadedPaths, findings: &[Finding]) {
    let findings = findings
        .iter()
        .map(|finding| {
            if finding.error {
                CheckFinding::offline_error(
                    "config",
                    CheckCategory::Config,
                    finding.message.clone(),
                )
            } else {
                CheckFinding::offline_note("config", CheckCategory::Config, finding.message.clone())
            }
        })
        .collect::<Vec<_>>();
    output::print_validation_human(loaded, &findings);
}

pub(crate) fn has_error_findings(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| finding.error)
}

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
Validate the resolved Temper config and credentials.

Usage: temper [GLOBAL OPTIONS] check [OPTIONS]

Options:
      --component <standalone|engine|worker>
                           Component scope to validate (default: standalone;
                           webhook intake is validated under engine or standalone)
      --pool <NAME>        Worker pool to validate with --component worker
      --strict             Treat notes and warnings as failures
      --online             Also run component-scoped Forge/provider reachability checks
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
        OutputFormat::Human => output::print_validation_human(&report.loaded, &report.findings),
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
    findings: Vec<CheckFinding>,
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
            if check_options.online {
                add_online_findings(&resolved, env, paths, check_options, &mut findings);
            }
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
