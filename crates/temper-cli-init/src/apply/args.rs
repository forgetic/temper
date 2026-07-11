// SPDX-License-Identifier: MPL-2.0

//! `temper apply` arguments and composition-root entry point.

use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvMap, LoadOptions, PathResolver, TerminalPrompter};

use crate::provisioner::ForgejoProvisioner;

use super::run::run_apply;

pub const APPLY_USAGE: &str = "\
Apply a temper deployment plan to the forge.

Loads config.toml + the selected credential source, renders every configured
Forgejo repository from one validated workflow, shows the desired model, then
applies it after confirmation.

Usage: temper [GLOBAL OPTIONS] apply [OPTIONS]

Options:
  --yes                   Apply the provisioning plan without confirmation
  --existing-repo         Supported compatibility behavior: require every
                          configured repo to already exist
  -h, --help              Print help";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ApplyCredentialMode {
    /// Programmatic mode: provisioning may use loaded credentials but never
    /// writes local secret material.
    SkipLocalCredentials,
    /// CLI mode: merge successful results into the selected durable TOML file.
    UpdateLocalCredentials,
}

impl Default for ApplyCredentialMode {
    fn default() -> Self {
        Self::UpdateLocalCredentials
    }
}

#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    pub options: LoadOptions,
    pub existing_repo: bool,
    pub yes: bool,
    pub credential_mode: ApplyCredentialMode,
    pub env: EnvMap,
    pub paths: PathResolver,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ParsedApplyArgs {
    pub(super) help: bool,
    pub(super) options: LoadOptions,
    pub(super) existing_repo: bool,
    pub(super) yes: bool,
}

pub fn apply_main(args: Vec<String>, env: &EnvMap, paths: &PathResolver) -> ExitCode {
    apply_main_with_options(args, env, paths, LoadOptions::default())
}

pub fn apply_main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let parsed = match parse_apply_args(args, options) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper apply: {error}\n\n{APPLY_USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{APPLY_USAGE}");
        return ExitCode::SUCCESS;
    }

    let opts = ApplyOptions {
        options: parsed.options,
        existing_repo: parsed.existing_repo,
        yes: parsed.yes,
        credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
        env: env.clone(),
        paths: paths.clone(),
    };
    let mut prompter = TerminalPrompter::stdio();
    let mut provisioner = ForgejoProvisioner;
    match run_apply(&mut prompter, &mut provisioner, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper apply: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(super) fn parse_apply_args(
    args: Vec<String>,
    options: LoadOptions,
) -> Result<ParsedApplyArgs, String> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "-h" | "--help"))
    {
        return Ok(ParsedApplyArgs {
            help: true,
            options,
            ..Default::default()
        });
    }

    let mut parsed = ParsedApplyArgs {
        options,
        ..Default::default()
    };
    for arg in args {
        match arg.as_str() {
            "--existing-repo" => parsed.existing_repo = true,
            "--yes" => parsed.yes = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(parsed)
}
