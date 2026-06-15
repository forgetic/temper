// SPDX-License-Identifier: MPL-2.0

//! `temper init` — the interactive first-run command.
//!
//! It walks an operator through the minimum needed to stand up a deployment,
//! then writes the on-disk artifacts and provisions the forge:
//!
//! 1. **Collect answers** ([`collect_answers`]) — five questions plus two secret
//!    prompts (forge URL, workflow, webhook address, admin user + password,
//!    provider API key). No disk or network I/O.
//! 2. **Preflight** ([`preflight_clobber`]) — check *all* target paths up front
//!    so the flow never writes file I then aborts at file III.
//! 3. **Write** `config.toml`, `workflow.json` (the embedded basic-delivery
//!    bytes), and a freshly generated `webhook-secret` (chmod 600).
//! 4. **Provision** the forge idempotently (admin user+password → admin REST
//!    token → plan → `temper_provision::provision`). No intake issue is seeded.
//! 5. **Write** `credentials.toml` (chmod 600) from the admin identity, the
//!    minted role/bot identities, and the provider key.
//! 6. **Summarize** what was written and what to run next.
//!
//! ## The live-forge seam
//!
//! Steps 1–3, 5, 6 are pure-ish (prompts + disk only); only step 4 needs a forge
//! and a runtime. [`run_init`] takes a `&mut dyn Provisioner` so the live call is
//! injectable: [`main`] passes [`ForgejoProvisioner`] (mints an admin token and
//! calls the Forgejo adapter on a real runtime); the unit test passes a stub
//! that returns a canned [`Provisioned`] without touching a network. Issue #183's
//! e2e drives [`run_init`] with [`ScriptedPrompter`](temper_cli_common::ScriptedPrompter)
//! + a real [`ForgejoProvisioner`].

mod collect;
mod provisioner;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, LoadOptions, TerminalPrompter, parse_common_args};

pub use collect::{Answers, collect_answers};
pub use provisioner::{ForgejoProvisioner, ProvisionOutcome, ProvisionRequest, Provisioner};
pub use write::{InitArtifacts, build_artifacts, preflight_clobber, write_artifacts};

/// `temper init [OPTIONS]` usage.
pub const USAGE: &str = "\
Interactively configure and provision a temper deployment.

Walks you through your forge URL, admin credentials, and LLM provider key, then
writes config.toml + workflow.json + a webhook secret, provisions the forge
(idempotently), and writes credentials.toml.

Usage: temper init [OPTIONS]

Options:
  --config        <FILE>  Where to write config.toml
  --credentials   <FILE>  Where to write credentials.toml
  --force                 Overwrite existing local files
  --existing-repo         Provision onto a repo that already exists
  -h, --help              Print help";

/// Everything `temper init` needs that is *not* gathered interactively: the
/// resolved file targets, the clobber flag, the workspace root, and whether the
/// repo already exists.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    /// Where the config + credentials files go (and the webhook secret beside
    /// the config).
    pub options: LoadOptions,
    /// Overwrite pre-existing local files instead of refusing.
    pub force: bool,
    /// Provision onto a repo that must already exist (`--existing-repo`).
    pub existing_repo: bool,
    /// The per-job worker workspace root written into `[worker] workspace`.
    /// `None` lets the daemon's default (`~/.local/state/temper/workspace`)
    /// apply by omitting the key.
    pub workspace: Option<PathBuf>,
}

/// A failure in the `temper init` flow.
#[derive(Debug)]
pub enum InitError {
    /// An interactive prompt failed (e.g. EOF on stdin).
    Prompt(std::io::Error),
    /// An answer named an unsupported choice (e.g. a hosted GitHub URL).
    Unsupported(String),
    /// A local file already exists and `--force` was not given (preflight).
    Clobber(Vec<PathBuf>),
    /// Writing a config/credentials/workflow/secret file failed.
    Write(String),
    /// Provisioning the forge failed.
    Provision(String),
    /// A path could not be resolved (no `--config`/env/HOME).
    Path(String),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InitError::Prompt(error) => write!(f, "reading input failed: {error}"),
            InitError::Unsupported(detail) => write!(f, "{detail}"),
            InitError::Clobber(paths) => {
                write!(
                    f,
                    "these files already exist (pass --force to overwrite): {}",
                    paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            InitError::Write(detail) => write!(f, "{detail}"),
            InitError::Provision(detail) => write!(f, "provisioning failed: {detail}"),
            InitError::Path(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for InitError {}

impl From<std::io::Error> for InitError {
    fn from(error: std::io::Error) -> Self {
        InitError::Prompt(error)
    }
}

/// The unified binary's `temper init` entry point: parse args, build a
/// [`TerminalPrompter`] + the real [`ForgejoProvisioner`], and run the flow.
pub fn main(args: std::env::Args) -> ExitCode {
    let parsed = match parse_common_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("temper init: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
    };
    if parsed.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let mut existing_repo = false;
    let mut force = false;
    for arg in &parsed.rest {
        match arg.as_str() {
            "--force" => force = true,
            "--existing-repo" => existing_repo = true,
            other => {
                eprintln!("temper init: unexpected argument `{other}`\n\n{USAGE}");
                return ExitCode::from(EX_USAGE);
            }
        }
    }

    let opts = InitOptions {
        options: parsed.options,
        force,
        existing_repo,
        workspace: None,
    };
    let mut prompter = TerminalPrompter::stdio();
    let mut provisioner = ForgejoProvisioner;
    match run_init(&mut prompter, &mut provisioner, &opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("temper init: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The testable core of `temper init`: collect answers, preflight, write the
/// local files, provision the forge, write credentials, and summarize.
///
/// The provisioning step is the only one that touches a forge; it goes through
/// `provisioner`, so a [`ScriptedPrompter`](temper_cli_common::ScriptedPrompter)
/// plus a stub [`Provisioner`] exercises the entire flow offline.
pub fn run_init(
    p: &mut dyn temper_cli_common::Prompter,
    provisioner: &mut dyn Provisioner,
    opts: &InitOptions,
) -> Result<(), InitError> {
    // 1. Collect answers (prompts only).
    let answers = collect_answers(p)?;

    // 2. Build the artifacts (pure) and preflight every local target up front.
    let artifacts = build_artifacts(&answers, opts)?;
    preflight_clobber(&artifacts, opts.force)?;

    // 3. Write config.toml, workflow.json, and the fresh webhook secret.
    write_artifacts(&artifacts, opts.force)?;

    // 4. Provision the forge idempotently (admin token, plan, orchestration).
    let request = ProvisionRequest {
        base_url: answers.forge_url.clone(),
        admin_user: answers.admin_user.clone(),
        admin_password: answers.admin_password.clone(),
        owner: answers.repo_owner.clone(),
        name: answers.repo_name.clone(),
        webhook_url: answers.webhook_url(),
        webhook_secret_file: artifacts.webhook_secret_path.clone(),
        existing_repo: opts.existing_repo,
    };
    let outcome = provisioner
        .provision(&request)
        .map_err(InitError::Provision)?;

    // 5. Build + write credentials.toml (chmod 600).
    write::write_credentials(&answers, &artifacts, &outcome, opts.force)?;

    // 6. Summary.
    p.note(&format!("Wrote {}", artifacts.config_path.display()));
    p.note(&format!("Wrote {}", artifacts.workflow_path.display()));
    p.note(&format!(
        "Wrote {} (chmod 600)",
        artifacts.webhook_secret_path.display()
    ));
    p.note(&format!(
        "Wrote {} (chmod 600)",
        artifacts.credentials_path.display()
    ));
    p.note(&format!(
        "Provisioned {}/{} with {} role(s) and the `{}` automation bot.",
        outcome.provisioned.owner,
        outcome.provisioned.name,
        outcome.provisioned.roles.len(),
        outcome.provisioned.automation.user,
    ));
    p.note("");
    p.note("Now run `temper daemon` to start the engine, worker, and agent.");
    Ok(())
}
