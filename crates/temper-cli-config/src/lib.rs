// SPDX-License-Identifier: MPL-2.0

//! `temper config` — guided or programmatic configuration.
//!
//! - `validate` — load the config + credentials, resolve them, and report any
//!   problems (and advisory notes) without starting anything.
//! - `show` — print the effective resolved deployment, with secrets redacted.
//! - `init` — write starter `config.toml` + `credentials.toml` templates.
//!
//! This crate owns only argv parsing, terminal output, and exit codes; the
//! config schema, resolution, and writing all live in [`temper_config`], and the
//! shared file-writing/exit-code helpers in [`temper_cli_common`].

use std::path::PathBuf;
use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, PathResolver, WriteOutcome, resolve_targets, restrict_600, run,
    write_new_file,
};
use temper_config::{
    ConfigError, Finding, LoadInputs, LoadedPaths, ProviderCredential, Resolved, WebUiCreds,
    config_template, credentials_template, lint, load_explicit,
};

/// Everything `temper config` needs, with no ambient environment access.
///
/// `env` / `paths` are the snapshot the composition root (`src/bin`) captured;
/// nothing in this crate reads `std::env`.
pub struct ConfigInputs<'a> {
    /// The program arguments after `config` (the subcommand + its flags).
    pub args: Vec<String>,
    /// The injected environment snapshot (used only for `$HOME` / `$XDG_*`
    /// path expansion; no environment variable selects the config files).
    pub env: &'a EnvMap,
    /// The injected base directories (HOME / XDG_*) for default-location discovery.
    pub paths: &'a PathResolver,
}

/// Loads + resolves a deployment for `validate` / `show`, honoring the same
/// hermeticity rule the daemon uses: an explicit `--config` / `--secrets`
/// suppresses default `~/.config/temper` discovery. An explicit config root may
/// load its sibling `credentials.toml`, but the operator's global credentials
/// global credentials never ambiently layer in behind an explicit deployment.
fn load_for(
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let explicit = options.config.is_some() || options.credentials.is_some();
    let empty = PathResolver::default();
    let paths: &PathResolver = if explicit { &empty } else { paths };
    load_explicit(&LoadInputs {
        explicit_config: options.config.clone(),
        explicit_credentials: options.credentials.clone(),
        env,
        paths,
    })
}

pub const USAGE: &str = "\
Guided or programmatic configuration.

Usage: temper config <COMMAND> [OPTIONS]

Commands:
  validate  Load and validate the config + credentials, reporting any problems
  show      Print the effective resolved configuration (secrets redacted)
  init      Write starter config.toml + credentials.toml templates

Options:
  --config  <PATH>  Path to configuration file or bundle directory
  --secrets <PATH>  Explicit secret source directory or credentials.toml
  --force           (init) overwrite existing files
  -h, --help        Print help";

pub fn main(inputs: ConfigInputs) -> ExitCode {
    let ConfigInputs { args, env, paths } = inputs;
    let Some((action, rest)) = args.split_first() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    match action.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "validate" => run("temper config", validate(rest, env, paths)),
        "show" => run("temper config", show(rest, env, paths)),
        "init" => run("temper config", init(rest, env, paths)),
        other => {
            eprintln!("temper config: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

/// Parses the `--config` / `--secrets` flags (and, when `allow_force`, the
/// `--force` flag) from a config-subcommand's args.
fn parse_options(args: &[String], allow_force: bool) -> Result<(LoadOptions, bool), String> {
    let mut options = LoadOptions::default();
    let mut force = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                options.config = Some(PathBuf::from(temper_cli_common::next_value(
                    &mut iter, "--config",
                )?))
            }
            "--secrets" => {
                options.credentials = Some(PathBuf::from(temper_cli_common::next_value(
                    &mut iter, arg,
                )?))
            }
            "--force" if allow_force => force = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok((options, force))
}

fn validate(args: &[String], env: &EnvMap, paths: &PathResolver) -> Result<ExitCode, String> {
    let (options, _) = parse_options(args, false)?;
    let (resolved, loaded) = load_for(&options, env, paths).map_err(|error| error.to_string())?;
    if let Some(path) = &loaded.config {
        println!("config:      {}", path.display());
    } else {
        println!("config:      (none — defaults + environment)");
    }
    if let Some(path) = &loaded.credentials {
        println!("credentials: {}", path.display());
    } else {
        println!("credentials: (none — environment)");
    }
    println!();

    let findings = lint(&resolved);
    if findings.is_empty() {
        println!("OK — no problems found.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut has_error = false;
    for Finding { error, message } in &findings {
        if *error {
            has_error = true;
            println!("error: {message}");
        } else {
            println!("note:  {message}");
        }
    }
    if has_error {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn show(args: &[String], env: &EnvMap, paths: &PathResolver) -> Result<ExitCode, String> {
    let (options, _) = parse_options(args, false)?;
    let (resolved, _loaded) = load_for(&options, env, paths).map_err(|error| error.to_string())?;
    print!("{}", render(&resolved));
    Ok(ExitCode::SUCCESS)
}

fn init(args: &[String], env: &EnvMap, paths: &PathResolver) -> Result<ExitCode, String> {
    let (options, force) = parse_options(args, true)?;
    let targets = resolve_targets(&options, env, paths)?;

    let _ = write_new_file(&targets.config, &config_template(), force)?;
    match write_new_file(&targets.credentials, &credentials_template(), force)? {
        WriteOutcome::Created | WriteOutcome::Overwritten => restrict_600(&targets.credentials)?,
    }

    println!("Wrote {}", targets.config.display());
    println!("Wrote {} (chmod 600)", targets.credentials.display());
    println!("\nEdit both, then run `temper config validate`.");
    Ok(ExitCode::SUCCESS)
}

/// Renders the resolved deployment for `config show`, redacting every secret.
fn render(resolved: &Resolved) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let present = |value: &Option<_>| if value.is_some() { "set" } else { "unset" };

    let _ = writeln!(out, "[forge]");
    let _ = writeln!(
        out,
        "  url          = {}",
        resolved.forge.url.as_deref().unwrap_or("(unset)")
    );
    let _ = writeln!(
        out,
        "  admin_token  = {}",
        present(&resolved.forge.admin_token)
    );
    let _ = writeln!(
        out,
        "  web_ui       = {}",
        match &resolved.forge.web_ui {
            Some(WebUiCreds { username, .. }) => format!("set (user {username})"),
            None => "unset".to_string(),
        }
    );
    let roles_with_identity: Vec<&String> = resolved.forge.role_identities.keys().collect();
    let _ = writeln!(out, "  role tokens  = {}", resolved.forge.role_tokens.len());
    let _ = writeln!(
        out,
        "  identities   = [{}]",
        roles_with_identity
            .iter()
            .map(|role| role.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let _ = writeln!(out, "\n[engine]");
    let _ = writeln!(out, "  bind         = {}", resolved.engine.bind);
    let _ = writeln!(
        out,
        "  repos        = [{}]",
        resolved
            .engine
            .repos
            .iter()
            .map(|repo| repo.display())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        out,
        "  roles        = [{}]",
        resolved.engine.roles.join(", ")
    );
    let _ = writeln!(
        out,
        "  workflow     = {}",
        resolved
            .engine
            .workflow_file
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(bundled reference-delivery)".to_string())
    );
    let _ = writeln!(out, "  poll_cadence = {:?}", resolved.engine.poll_cadence);
    let _ = writeln!(
        out,
        "  mechanical   = {}",
        match resolved.engine.mechanical_cadence {
            Some(cadence) => format!("{cadence:?}"),
            None => "disabled".to_string(),
        }
    );
    let _ = writeln!(out, "  lease_ttl    = {:?}", resolved.engine.lease_ttl);
    let _ = writeln!(out, "  daemon_id    = {}", resolved.engine.daemon_id);
    let _ = writeln!(
        out,
        "  webhook      = {}",
        resolved
            .engine
            .webhook_secret_file
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );

    let _ = writeln!(out, "\n[worker]");
    let _ = writeln!(out, "  worker_id    = {}", resolved.worker.worker_id);
    let _ = writeln!(out, "  daemon_url   = {}", resolved.worker.daemon_url);
    let _ = writeln!(
        out,
        "  workspace    = {}",
        resolved.worker.workspace_root.display()
    );
    let _ = writeln!(
        out,
        "  capabilities = [{}]",
        resolved
            .worker
            .capabilities
            .iter()
            .map(|c| format!("{}:{}", c.repo, c.role))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let _ = writeln!(out, "\n[agent]");
    let _ = writeln!(
        out,
        "  provider     = {}",
        resolved.agent.provider.kind.as_str()
    );
    let _ = writeln!(
        out,
        "  main model   = {}",
        resolved
            .agent
            .provider
            .main_model
            .as_deref()
            .unwrap_or("(default)")
    );
    let _ = writeln!(
        out,
        "  investigate  = {}",
        resolved
            .agent
            .provider
            .investigate_model
            .as_deref()
            .unwrap_or("(default)")
    );
    let _ = writeln!(
        out,
        "  base_url     = {}",
        resolved
            .agent
            .provider
            .base_url
            .as_deref()
            .unwrap_or("(default)")
    );
    let _ = writeln!(
        out,
        "  credential   = {}",
        match &resolved.agent.provider.credential {
            ProviderCredential::OAuthInline { .. } => "oauth (inline)",
            ProviderCredential::OAuthFile(_) => "oauth (file)",
            ProviderCredential::ApiKey(_) => "api-key",
            ProviderCredential::Ambient => "ambient (env / ~/.pi/agent/auth.json)",
        }
    );
    let _ = writeln!(out, "  max_iters    = {}", resolved.agent.max_iterations);
    let _ = writeln!(out, "  subagents    = {}", resolved.agent.enable_subagents);

    out
}
