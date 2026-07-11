// SPDX-License-Identifier: MPL-2.0

//! Collection, local generation, optional deployment apply, and summary for init.

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{EX_USAGE, EnvLookup, EnvMap, LoadOptions, PathResolver, TerminalPrompter};

use crate::answers_file::{AnswersFile, load_answers_file};
use crate::apply::{ApplyCredentialMode, ApplyOptions, run_apply};
use crate::args::{InitOverrides, InitTopology, ParsedInitArgs, parse_init_args};
use crate::collect::collect_answers;
use crate::provisioner::{ApplyProvisioner, ForgejoProvisioner};
use crate::usage::USAGE;
use crate::write::{build_artifacts, preflight_clobber, write_artifacts, write_local_credentials};

/// Everything `temper init` needs that is not gathered interactively.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub options: LoadOptions,
    pub force: bool,
    /// Require every selected repository to exist during apply.
    pub existing_repo: bool,
    pub apply: bool,
    pub yes: bool,
    pub topology: InitTopology,
    pub overrides: InitOverrides,
    pub non_interactive: bool,
    pub workspace: Option<PathBuf>,
    pub env: EnvMap,
    pub paths: PathResolver,
}

/// A failure in the `temper init` flow.
#[derive(Debug)]
pub enum InitError {
    Prompt(std::io::Error),
    Unsupported(String),
    Clobber(Vec<PathBuf>),
    Write(String),
    Provision(String),
    Path(String),
}

impl std::fmt::Display for InitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Prompt(error) => write!(f, "reading input failed: {error}"),
            Self::Unsupported(detail) | Self::Write(detail) | Self::Path(detail) => {
                write!(f, "{detail}")
            }
            Self::Clobber(paths) => write!(
                f,
                "these files already exist (pass --force to overwrite): {}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Provision(detail) => write!(f, "provisioning failed: {detail}"),
        }
    }
}

impl std::error::Error for InitError {}
impl From<std::io::Error> for InitError {
    fn from(error: std::io::Error) -> Self {
        Self::Prompt(error)
    }
}

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
    if let Err(error) = validate_apply_confirmation(&opts) {
        eprintln!("temper init: {error}\n\n{USAGE}");
        return ExitCode::from(EX_USAGE);
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
    if let Some(password) = env.non_empty("TEMPER_INIT_ADMIN_PASSWORD") {
        overrides.admin_password = Some(password);
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

fn validate_apply_confirmation(opts: &InitOptions) -> Result<(), InitError> {
    if opts.apply && opts.non_interactive && !opts.yes {
        return Err(InitError::Unsupported(
            "--non-interactive --apply requires --yes to confirm forge-side provisioning".into(),
        ));
    }
    Ok(())
}

/// Generates a complete local bundle and optionally applies that deployment.
///
/// Apply reloads the generated config and credentials through the canonical
/// deployment loader. Both paths are explicit, so an ambient
/// `CREDENTIALS_DIRECTORY` cannot shadow the bootstrap credentials.
pub fn run_init(
    p: &mut dyn temper_cli_common::Prompter,
    provisioner: &mut dyn ApplyProvisioner,
    opts: &InitOptions,
) -> Result<(), InitError> {
    validate_apply_confirmation(opts)?;
    let answers = collect_answers(p, &opts.overrides, opts.non_interactive)?;
    let artifacts = build_artifacts(&answers, opts)?;
    preflight_clobber(&artifacts, opts.force)?;
    write_artifacts(&artifacts, opts.force)?;
    write_local_credentials(&answers, &artifacts, opts.force)?;

    if opts.apply {
        run_apply(
            p,
            provisioner,
            &ApplyOptions {
                options: LoadOptions {
                    config: Some(artifacts.config_path.clone()),
                    credentials: Some(artifacts.credentials_path.clone()),
                },
                existing_repo: opts.existing_repo,
                yes: opts.yes,
                credential_mode: ApplyCredentialMode::UpdateLocalCredentials,
                env: opts.env.clone(),
                paths: opts.paths.clone(),
            },
        )?;
    }

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
    if !opts.apply {
        p.note("Skipped forge provisioning (pass --apply to apply the generated deployment).");
    }
    p.note("");
    p.note("Deployment workflow: `temper check` -> `temper plan` -> `temper apply` -> `temper serve standalone`.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::init_options_from_parsed;
    use crate::args::parse_init_args;
    use crate::{InitTopology, USAGE};
    use std::path::PathBuf;
    use temper_cli_common::{EnvMap, LoadOptions, PathResolver};

    #[test]
    fn usage_documents_workspace_apply_answers_and_selection_flags() {
        for expected in [
            "--workspace",
            "Top-level worker workspace root",
            "--apply",
            "--yes",
            "--admin-user",
            "skips the admin prompt",
            "--answers",
            "schema_version = 1",
            "standalone|distributed",
            "anthropic|chatgpt|deepseek|none",
            "repeatable",
            "cannot set --apply",
            "every",
        ] {
            assert!(USAGE.contains(expected), "missing {expected}: {USAGE}");
        }
        assert!(!USAGE.contains("  --config"), "{USAGE}");
        assert!(!USAGE.contains("  --secrets"), "{USAGE}");
    }

    #[test]
    fn answers_file_implies_non_interactive_and_flags_env_win() {
        let dir = tempfile::tempdir().expect("tempdir");
        let answers = dir.path().join("answers.toml");
        std::fs::write(&answers, "schema_version = 1\ntopology = \"distributed\"\nforge_url = \"http://answers-forge.local:3000\"\nadmin_user = \"answers-admin\"\nadmin_password = \"answers-pw\"\nprovider = \"deepseek\"\nprovider_key = \"answers-key\"\nrepos = [\"answers/repo\"]\n").expect("answers");
        let parsed = parse_init_args(
            vec![
                "--answers".into(),
                answers.display().to_string(),
                "--topology".into(),
                "standalone".into(),
                "--forge".into(),
                "http://flag-forge.local:3000".into(),
                "--repo".into(),
                "flag/repo".into(),
                "--provider".into(),
                "chatgpt".into(),
            ],
            LoadOptions::default(),
        )
        .expect("parse");
        let mut env = EnvMap::new();
        env.insert("TEMPER_INIT_ADMIN_PASSWORD", "env-pw");
        env.insert("TEMPER_INIT_PROVIDER_KEY", "env-provider-key");
        let opts = init_options_from_parsed(parsed, &env, &PathResolver::default()).expect("opts");
        assert!(opts.non_interactive);
        assert_eq!(opts.topology, InitTopology::Standalone);
        assert_eq!(
            opts.overrides.forge_url.as_deref(),
            Some("http://flag-forge.local:3000")
        );
        assert_eq!(opts.overrides.repos[0].path(), "flag/repo");
        assert_eq!(opts.overrides.provider.as_deref(), Some("chatgpt"));
        assert_eq!(opts.overrides.admin_password.as_deref(), Some("env-pw"));
        assert_eq!(
            opts.overrides.provider_key.as_deref(),
            Some("env-provider-key")
        );
    }

    #[test]
    fn answers_file_load_error_is_usage_error() {
        let parsed = parse_init_args(
            vec!["--answers".into(), "missing.toml".into()],
            LoadOptions::default(),
        )
        .expect("parse");
        let error = init_options_from_parsed(parsed, &EnvMap::new(), &PathResolver::default())
            .expect_err("error");
        assert!(error.contains("read answers file"), "{error}");
    }

    #[test]
    fn answers_file_path_is_preserved_as_pathbuf() {
        let parsed = parse_init_args(
            vec!["--answers".into(), "answers.toml".into()],
            LoadOptions::default(),
        )
        .expect("parse");
        assert_eq!(parsed.answers, Some(PathBuf::from("answers.toml")));
    }
}
