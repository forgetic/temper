// SPDX-License-Identifier: MPL-2.0

//! `temper init` — the interactive first-run command.
//!
//! It walks an operator through the minimum needed to stand up a deployment,
//! writes the on-disk artifacts, and only provisions the forge when explicitly
//! asked via `--apply`:
//!
//! 1. **Collect answers** ([`collect_answers`]) — five questions plus two secret
//!    prompts (forge URL, workflow, webhook address, admin user + password,
//!    provider API key). No disk or network I/O.
//! 2. **Preflight** ([`preflight_clobber`]) — check *all* target paths up front
//!    so the flow never writes file I then aborts at file III.
//! 3. **Write** `config.toml`, `workflow.json` (the embedded basic-delivery
//!    bytes), and a freshly generated `webhook-secret` (chmod 600).
//! 4. **Write** a local `credentials.toml` (chmod 600) with the operator-supplied
//!    admin password and provider key.
//! 5. **Optionally provision** the forge idempotently when `--apply` is set
//!    (admin user+password → admin REST token → plan →
//!    `temper_provision::provision`), then update `credentials.toml` with the
//!    minted admin/role/bot tokens. No intake issue or repository seed content
//!    is created.
//! 6. **Summarize** what was written, whether provisioning ran, and what to run
//!    next.
//!
//! ## The live-forge seam
//!
//! Steps 1–4 and 6 are local (prompts + disk only); only the optional apply
//! portion of step 5 needs a forge and a runtime. [`run_init`] takes a
//! `&mut dyn Provisioner` so the live call is
//! injectable: [`main`] passes [`ForgejoProvisioner`] (mints an admin token and
//! calls the Forgejo adapter on a real runtime); the unit test passes a stub
//! that returns a canned [`Provisioned`] without touching a network. Issue #183's
//! e2e drives [`run_init`] with [`ScriptedPrompter`](temper_cli_common::ScriptedPrompter)
//! + a real [`ForgejoProvisioner`].

mod apply;
mod args;
mod collect;
mod provisioner;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvLookup, EnvMap, LoadOptions, PathResolver, TerminalPrompter};

use args::parse_init_args;

pub use apply::{APPLY_USAGE, ApplyOptions, apply_main, run_apply};
pub use args::{InitOverrides, InitTopology, RepoSelection};
pub use collect::{Answers, collect_answers};
pub use provisioner::{ForgejoProvisioner, ProvisionOutcome, ProvisionRequest, Provisioner};
pub use write::{InitArtifacts, build_artifacts, preflight_clobber, write_artifacts};

/// `temper init [OPTIONS]` usage.
pub const USAGE: &str = "\
Interactively configure a temper deployment.

Walks you through your forge URL, admin credentials, and LLM provider key, then
writes config.toml + workflow.json + a webhook secret + credentials.toml.
Forge-side provisioning (repo/users/labels/webhook registration) only runs when
--apply is set; --yes skips that apply confirmation.

Usage: temper init [OPTIONS]

Options:
  --config        <PATH>        Where to write config.toml, or bundle directory
  --secrets       <PATH>        Explicit secret source directory or credentials.toml
  --force                       Overwrite existing local files
  --apply                       After writing local files, provision the forge
  --yes                         With --apply, skip the provisioning confirmation
  --existing-repo               Provision onto a repo that already exists
  --topology      <standalone>  Local topology to initialize (only standalone today)
  --repo          <owner/name>  Managed repository to provision
  --forge         <URL>         Forgejo URL; skips the Forge URL prompt
  --bind          <ADDR>        Daemon bind / webhook advertise address override
  --workspace     <PATH>        Per-job worker workspace root to write
  --provider      <deepseek>    LLM provider profile (only deepseek today)
  --provider-url  <URL>         Base URL override for the provider
  --non-interactive             Run without prompts; all required values must
                                be supplied via flags or environment variables
  --admin-user   <VALUE>        Forgejo admin username (only non-interactive)
  -h, --help                    Print help

Environment variables (only honoured with --non-interactive):
  TEMPER_INIT_ADMIN_PASSWORD    Forgejo admin password
  TEMPER_INIT_PROVIDER_KEY      LLM provider API key";

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
    /// After writing local files, run the forge provisioning/apply path.
    pub apply: bool,
    /// Skip the confirmation before `--apply` performs forge-side mutations.
    pub yes: bool,
    /// The topology selected by `--topology` (standalone only today).
    pub topology: InitTopology,
    /// Non-interactive answers selected by local-dev flags.
    pub overrides: InitOverrides,
    /// Run without prompts; all required values must be supplied via flags or
    /// environment variables.
    pub non_interactive: bool,
    /// The per-job worker workspace root written into `[worker] workspace`.
    /// `None` lets the daemon's default (`~/.local/state/temper/workspace`)
    /// apply by omitting the key.
    pub workspace: Option<PathBuf>,
    /// The injected environment snapshot used to resolve default file targets
    /// (the snapshot `src/bin` captured; no `std::env` is read here).
    pub env: EnvMap,
    /// The injected base directories (HOME / XDG_*) for default-target discovery.
    pub paths: PathResolver,
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
///
/// `env` / `paths` are the snapshot the composition root (`src/bin`) captured;
/// this reads no `std::env`.
pub fn main(args: Vec<String>, env: &EnvMap, paths: &PathResolver) -> ExitCode {
    let parsed = match parse_init_args(args) {
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

    let mut opts = InitOptions {
        options: parsed.options,
        force: parsed.force,
        existing_repo: parsed.existing_repo,
        apply: parsed.apply,
        yes: parsed.yes,
        topology: parsed.topology,
        overrides: parsed.overrides,
        non_interactive: parsed.non_interactive,
        workspace: parsed.workspace,
        env: env.clone(),
        paths: paths.clone(),
    };

    // Resolve env-only secret overrides. They are only honoured when
    // --non-interactive is set; collect_answers enforces that gate.
    if let Some(pw) = env.get("TEMPER_INIT_ADMIN_PASSWORD") {
        opts.overrides.admin_password = Some(pw);
    }
    if let Some(key) = env.get("TEMPER_INIT_PROVIDER_KEY") {
        opts.overrides.provider_key = Some(key);
    }
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
/// local files, optionally provision the forge, and summarize.
///
/// The provisioning step is the only one that touches a forge; it runs only when
/// [`InitOptions::apply`] is true and goes through `provisioner`, so a
/// [`ScriptedPrompter`](temper_cli_common::ScriptedPrompter) plus a stub
/// [`Provisioner`] exercises both the local-only and apply paths offline.
pub fn run_init(
    p: &mut dyn temper_cli_common::Prompter,
    provisioner: &mut dyn Provisioner,
    opts: &InitOptions,
) -> Result<(), InitError> {
    if opts.apply && opts.non_interactive && !opts.yes {
        return Err(InitError::Unsupported(
            "--non-interactive --apply requires --yes to confirm forge-side provisioning"
                .to_string(),
        ));
    }

    // 1. Collect answers (prompts only).
    let answers = collect_answers(p, &opts.overrides, opts.non_interactive)?;

    // 2. Build the artifacts (pure) and preflight every local target up front.
    let artifacts = build_artifacts(&answers, opts)?;
    preflight_clobber(&artifacts, opts.force)?;

    // 3. Write config.toml, workflow.json, and the fresh webhook secret.
    write_artifacts(&artifacts, opts.force)?;

    // 4. Write local credentials before any optional forge mutation. These
    // contain the operator-supplied secrets (admin password + provider key) so
    // a later apply can mint and persist forge tokens.
    write::write_local_credentials(&answers, &artifacts, opts.force)?;

    let mut applied = false;
    let mut outcome = None;
    if opts.apply {
        let confirmed = opts.yes
            || p.confirm(
                &format!(
                    "Provision {}/{} on {} and register {}?",
                    answers.repo_owner,
                    answers.repo_name,
                    answers.forge_url,
                    answers.webhook_url()
                ),
                false,
            )?;
        if confirmed {
            // 5. Provision the forge idempotently (admin token, plan,
            // orchestration). `temper init` configures integration only; it
            // requests no repository seed commits, so user/project files are
            // left untouched.
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
            let provisioned = provisioner
                .provision(&request)
                .map_err(InitError::Provision)?;

            // Replace the just-written local credentials with the applied
            // version that includes minted admin/role/bot tokens. Force is safe
            // here: preflight already proved we are not clobbering an unrelated
            // file (or the caller passed --force), and this path is our own
            // local-write-then-apply sequence.
            write::write_provisioned_credentials(&answers, &artifacts, &provisioned, true)?;
            applied = true;
            outcome = Some(provisioned);
        }
    }

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
    if let Some(outcome) = outcome {
        p.note(&format!(
            "Provisioned {}/{} with {} role(s) and the `{}` automation bot.",
            outcome.provisioned.owner,
            outcome.provisioned.name,
            outcome.provisioned.roles.len(),
            outcome.provisioned.automation.user,
        ));
    } else if opts.apply {
        p.note("Skipped forge provisioning at operator confirmation.");
    } else {
        p.note("Skipped forge provisioning (pass --apply to provision users, labels, and webhooks).");
    }
    p.note("");
    if applied {
        p.note("Now run `temper serve standalone` to start the engine, worker, and agent.");
    } else {
        p.note("Run `temper init --apply` (or `temper apply`) before starting the engine.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::USAGE;

    #[test]
    fn usage_documents_workspace_and_apply_flags() {
        assert!(USAGE.contains("--workspace"), "{USAGE}");
        assert!(USAGE.contains("Per-job worker workspace root"), "{USAGE}");
        assert!(USAGE.contains("--apply"), "{USAGE}");
        assert!(USAGE.contains("--yes"), "{USAGE}");
    }
}
