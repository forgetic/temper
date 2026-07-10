// SPDX-License-Identifier: MPL-2.0

//! `temper init` — the interactive first-run command.
//!
//! It walks an operator through the minimum needed to stand up a deployment,
//! writes the on-disk artifacts, and only provisions the forge when explicitly
//! asked via `--apply`:
//!
//! 1. **Collect answers** ([`collect_answers`]) — forge URL, workflow,
//!    webhook address, admin user/password, repository/provider selection, and
//!    any provider secret the selection needs. No disk or network I/O.
//! 2. **Preflight** ([`preflight_clobber`]) — check *all* target paths up front
//!    so the flow never writes file I then aborts at file III.
//! 3. **Write** `config.toml`, `workflow.yaml` (the selected builtin or
//!    normalized custom workflow YAML), and a freshly generated
//!    `webhook-secret` (chmod 600).
//! 4. **Write** a local `credentials.toml` (chmod 600) with the operator-supplied
//!    admin password and any provider key.
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

mod answers_file;
mod apply;
mod args;
mod collect;
mod plan;
mod provisioner;
mod write;

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvLookup, EnvMap, LoadOptions, PathResolver, TerminalPrompter};

use answers_file::{AnswersFile, load_answers_file};
use args::{ParsedInitArgs, parse_init_args};

pub use apply::{
    APPLY_USAGE, ApplyCredentialMode, ApplyOptions, apply_main, apply_main_with_options, run_apply,
};
pub use args::{InitOverrides, InitTopology, RepoSelection};
pub use collect::{Answers, collect_answers};
pub use plan::{PLAN_USAGE, PlanOptions, plan_main_with_options, run_plan};
pub use provisioner::{
    ApplyPlanOutcome, ApplyPlanRequest, ApplyProvisioner, ForgejoProvisioner, ProvisionOutcome,
    ProvisionRequest, Provisioner,
};
pub use write::{InitArtifacts, build_artifacts, preflight_clobber, write_artifacts};

/// `temper init [OPTIONS]` usage.
pub const USAGE: &str = r#"Interactively configure a temper deployment.

Walks you through your forge URL, admin credentials, and LLM provider choice, then
writes config.toml + workflow.yaml + a webhook secret + credentials.toml.
Forge-side provisioning (repo/users/labels/webhook registration) only runs when
--apply is set; --yes skips that apply confirmation.

Usage: temper [GLOBAL OPTIONS] init [OPTIONS]

Options:
  --force                       Overwrite existing local files
  --apply                       After writing local files, provision the forge
  --yes                         With --apply, skip the provisioning confirmation
  --existing-repo               Supported compatibility behavior: require the
                                repo to already exist when provisioning
  --topology      <standalone|distributed>
                                Topology to collect for the initialized bundle
  --repo          <owner/name>  Managed repository to provision (repeatable)
  --workflow      <builtin|PATH>  Builtin workflow name or JSON/YAML workflow file
  --forge         <URL>         Forgejo URL; skips the Forge URL prompt
  --bind          <ADDR>        Daemon bind / webhook advertise address override
  --workspace     <PATH>        Top-level worker workspace root to write
  --provider      <anthropic|chatgpt|deepseek|none>
                                LLM provider profile to configure
  --provider-url  <URL>         Base URL override for the provider
  --answers       <FILE>        TOML answers file; implies --non-interactive
  --non-interactive             Run without prompts; all required values must
                                be supplied via flags, --answers, or environment
  --admin-user   <VALUE>        Forgejo admin username; skips the admin prompt
  -h, --help                    Print help

Environment variables (only honoured with --non-interactive or --answers):
  TEMPER_INIT_ADMIN_PASSWORD    Forgejo admin password (wins over --answers)
  TEMPER_INIT_PROVIDER_KEY      DeepSeek provider API key (wins over --answers)

Answers file (TOML, used by --answers and implies --non-interactive):
  schema_version = 1
  topology = "standalone"          # or "distributed"
  forge_url = "http://localhost:3000"
  workflow = "basic-delivery"      # builtin or JSON/YAML path
  webhook_addr = "http://127.0.0.1:8314"
  admin_user = "root"
  admin_password = "..."           # secret; env TEMPER_INIT_ADMIN_PASSWORD wins
  provider = "deepseek"            # anthropic|chatgpt|deepseek|none
  provider_key = "..."             # secret for deepseek; env TEMPER_INIT_PROVIDER_KEY wins
  provider_url = "http://localhost:9999/v1"
  repos = ["owner/name", "owner/other"]

The answers file cannot set --apply; pass --apply explicitly to provision."#;

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
    /// The topology selected by `--topology` / answers file.
    pub topology: InitTopology,
    /// Init answers selected by flags, answers file, and non-interactive env secrets.
    pub overrides: InitOverrides,
    /// Run without prompts; all required values must be supplied via flags,
    /// answers file, or environment variables.
    pub non_interactive: bool,
    /// The top-level worker workspace root written into `[worker] workspace`.
    /// `None` lets the daemon's default (`~/.local/state/temper/workspace`)
    /// apply by omitting the key; workers create per-job scoped roots below it.
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
    main_with_options(args, env, paths, LoadOptions::default())
}

pub fn main_with_options(
    args: Vec<String>,
    env: &EnvMap,
    paths: &PathResolver,
    options: LoadOptions,
) -> ExitCode {
    let parsed = match parse_init_args(args, options) {
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

    let opts = match init_options_from_parsed(parsed, env, paths) {
        Ok(opts) => opts,
        Err(error) => {
            eprintln!("temper init: {error}\n\n{USAGE}");
            return ExitCode::from(EX_USAGE);
        }
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

fn init_options_from_parsed(
    parsed: ParsedInitArgs,
    env: &EnvMap,
    paths: &PathResolver,
) -> Result<InitOptions, String> {
    let ParsedInitArgs {
        help: _,
        options,
        force,
        existing_repo,
        apply,
        yes,
        topology: flag_topology,
        overrides: flag_overrides,
        workspace,
        non_interactive,
        answers,
    } = parsed;

    let mut topology = InitTopology::Standalone;
    let mut overrides = InitOverrides::default();
    let mut answer_file_seen = false;

    if let Some(path) = answers {
        let AnswersFile {
            topology: answer_topology,
            overrides: answer_overrides,
        } = load_answers_file(&path)?;
        answer_file_seen = true;
        if let Some(answer_topology) = answer_topology {
            topology = answer_topology;
        }
        overrides = answer_overrides;
    }

    if let Some(flag_topology) = flag_topology {
        topology = flag_topology;
    }
    overlay_overrides(&mut overrides, flag_overrides);

    // Resolve env-only secret overrides after answers and flags so a CI or
    // shell-provided secret wins without rewriting the reproducible answers file.
    // They are only consumed by collection when non-interactive is active.
    if let Some(pw) = env.non_empty("TEMPER_INIT_ADMIN_PASSWORD") {
        overrides.admin_password = Some(pw);
    }
    if let Some(key) = env.non_empty("TEMPER_INIT_PROVIDER_KEY") {
        overrides.provider_key = Some(key);
    }

    Ok(InitOptions {
        options,
        force,
        existing_repo,
        apply,
        yes,
        topology,
        overrides,
        non_interactive: non_interactive || answer_file_seen,
        workspace,
        env: env.clone(),
        paths: paths.clone(),
    })
}

fn overlay_overrides(base: &mut InitOverrides, overlay: InitOverrides) {
    if let Some(value) = overlay.forge_url {
        base.forge_url = Some(value);
    }
    if let Some(value) = overlay.bind {
        base.bind = Some(value);
    }
    if !overlay.repos.is_empty() {
        base.repos = overlay.repos;
    }
    if let Some(value) = overlay.workflow {
        base.workflow = Some(value);
    }
    if let Some(value) = overlay.provider {
        base.provider = Some(value);
    }
    if let Some(value) = overlay.admin_user {
        base.admin_user = Some(value);
    }
    if let Some(value) = overlay.admin_password {
        base.admin_password = Some(value);
    }
    if let Some(value) = overlay.provider_key {
        base.provider_key = Some(value);
    }
    if let Some(value) = overlay.provider_url {
        base.provider_url = Some(value);
    }
}

fn multiple_repo_apply_error(count: usize) -> InitError {
    InitError::Unsupported(format!(
        "--apply currently supports exactly one repository, found {count}; omit --apply or pass a single --repo"
    ))
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

    if opts.apply && opts.overrides.repos.len() > 1 {
        return Err(multiple_repo_apply_error(opts.overrides.repos.len()));
    }

    // 1. Collect answers (prompts only).
    let answers = collect_answers(p, &opts.overrides, opts.non_interactive)?;
    if opts.apply && answers.repos.len() != 1 {
        return Err(multiple_repo_apply_error(answers.repos.len()));
    }

    // 2. Build the artifacts (pure) and preflight every local target up front.
    let artifacts = build_artifacts(&answers, opts)?;
    preflight_clobber(&artifacts, opts.force)?;

    // 3. Write config.toml, workflow.yaml, and the fresh webhook secret.
    write_artifacts(&artifacts, opts.force)?;

    // 4. Write local credentials before any optional forge mutation. These
    // contain the operator-supplied secrets (admin password and any provider
    // key) so a later apply can mint and persist forge tokens.
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
                workflow_path: Some(artifacts.workflow_path.clone()),
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
        p.note(
            "Skipped forge provisioning (pass --apply to provision users, labels, and webhooks).",
        );
    }
    p.note("");
    if applied {
        p.note("Now run `temper serve standalone` to start the engine, worker, and agent.");
    } else if answers.repos.len() == 1 {
        p.note("Run `temper apply` before starting the engine.");
    } else {
        p.note(
            "Provision each repository before starting the engine; `temper apply` currently supports one repository.",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use temper_cli_common::{EnvMap, LoadOptions, PathResolver};

    use super::{USAGE, init_options_from_parsed};
    use crate::args::{InitTopology, parse_init_args};

    #[test]
    fn usage_documents_workspace_apply_answers_and_selection_flags() {
        assert!(USAGE.contains("--workspace"), "{USAGE}");
        assert!(USAGE.contains("Top-level worker workspace root"), "{USAGE}");
        assert!(USAGE.contains("--apply"), "{USAGE}");
        assert!(USAGE.contains("--yes"), "{USAGE}");
        assert!(USAGE.contains("--admin-user"), "{USAGE}");
        assert!(USAGE.contains("skips the admin prompt"), "{USAGE}");
        assert!(USAGE.contains("--answers"), "{USAGE}");
        assert!(USAGE.contains("schema_version = 1"), "{USAGE}");
        assert!(USAGE.contains("standalone|distributed"), "{USAGE}");
        assert!(USAGE.contains("anthropic|chatgpt|deepseek|none"), "{USAGE}");
        assert!(USAGE.contains("repeatable"), "{USAGE}");
        assert!(USAGE.contains("cannot set --apply"), "{USAGE}");
        assert!(
            !USAGE.contains("admin username (only non-interactive)"),
            "{USAGE}"
        );
        assert!(!USAGE.contains("  --config"), "{USAGE}");
        assert!(!USAGE.contains("  --secrets"), "{USAGE}");
    }

    #[test]
    fn answers_file_implies_non_interactive_and_flags_env_win() {
        let dir = tempfile::tempdir().expect("tempdir");
        let answers = dir.path().join("answers.toml");
        std::fs::write(
            &answers,
            r#"
schema_version = 1
topology = "distributed"
forge_url = "http://answers-forge.local:3000"
admin_user = "answers-admin"
admin_password = "answers-pw"
provider = "deepseek"
provider_key = "answers-key"
repos = ["answers/repo"]
"#,
        )
        .expect("answers file");

        let parsed = parse_init_args(
            vec![
                "--answers".to_string(),
                answers.display().to_string(),
                "--topology".to_string(),
                "standalone".to_string(),
                "--forge".to_string(),
                "http://flag-forge.local:3000".to_string(),
                "--repo".to_string(),
                "flag/repo".to_string(),
                "--provider".to_string(),
                "chatgpt".to_string(),
            ],
            LoadOptions::default(),
        )
        .expect("parse");
        let mut env = EnvMap::new();
        env.insert("TEMPER_INIT_ADMIN_PASSWORD", "env-pw");
        env.insert("TEMPER_INIT_PROVIDER_KEY", "env-provider-key");

        let opts = init_options_from_parsed(parsed, &env, &PathResolver::default())
            .expect("compose options");

        assert!(opts.non_interactive, "--answers implies non-interactive");
        assert_eq!(opts.topology, InitTopology::Standalone);
        assert_eq!(
            opts.overrides.forge_url.as_deref(),
            Some("http://flag-forge.local:3000")
        );
        assert_eq!(opts.overrides.repos[0].path(), "flag/repo");
        assert_eq!(opts.overrides.provider.as_deref(), Some("chatgpt"));
        assert_eq!(opts.overrides.admin_user.as_deref(), Some("answers-admin"));
        assert_eq!(opts.overrides.admin_password.as_deref(), Some("env-pw"));
        assert_eq!(
            opts.overrides.provider_key.as_deref(),
            Some("env-provider-key")
        );
    }

    #[test]
    fn answers_file_load_error_is_usage_error() {
        let parsed = parse_init_args(
            vec!["--answers".to_string(), "missing.toml".to_string()],
            LoadOptions::default(),
        )
        .expect("parse");

        let err = init_options_from_parsed(parsed, &EnvMap::new(), &PathResolver::default())
            .expect_err("missing answers file rejected");

        assert!(err.contains("read answers file"), "{err}");
    }

    #[test]
    fn answers_file_path_is_preserved_as_pathbuf() {
        let parsed = parse_init_args(
            vec!["--answers".to_string(), "answers.toml".to_string()],
            LoadOptions::default(),
        )
        .expect("parse");
        assert_eq!(parsed.answers, Some(PathBuf::from("answers.toml")));
    }
}
