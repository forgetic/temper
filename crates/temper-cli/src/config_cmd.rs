// SPDX-License-Identifier: MPL-2.0

//! `temper config` — guided or programmatic configuration.
//!
//! - `validate` — load the config + credentials, resolve them, and report any
//!   problems (and advisory notes) without starting anything.
//! - `show` — print the effective resolved deployment, with secrets redacted.
//! - `init` — write starter `config.toml` + `credentials.toml` templates.

use std::path::PathBuf;
use std::process::ExitCode;

use temper_config::{
    EX_USAGE, Finding, LoadOptions, ProviderCredential, Resolved, WebUiCreds, config_path,
    config_template, credentials_path, credentials_template, lint, load,
};

const USAGE: &str = "\
Guided or programmatic configuration.

Usage: temper config <COMMAND> [OPTIONS]

Commands:
  validate  Load and validate the config + credentials, reporting any problems
  show      Print the effective resolved configuration (secrets redacted)
  init      Write starter config.toml + credentials.toml templates

Options:
  --config      <FILE>  Path to configuration file
  --credentials <FILE>  Path to credentials file
  --force               (init) overwrite existing files
  -h, --help            Print help";

pub fn main(args: std::env::Args) -> ExitCode {
    let args: Vec<String> = args.collect();
    let Some((action, rest)) = args.split_first() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    match action.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "validate" => run(validate(rest)),
        "show" => run(show(rest)),
        "init" => run(init(rest)),
        other => {
            eprintln!("temper config: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

fn run(result: Result<ExitCode, String>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("temper config: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Parses the `--config` / `--credentials` flags (and, when `allow_force`, the
/// `--force` flag) from a config-subcommand's args.
fn parse_options(args: &[String], allow_force: bool) -> Result<(LoadOptions, bool), String> {
    let mut options = LoadOptions::default();
    let mut force = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => options.config = Some(PathBuf::from(next(&mut iter, "--config")?)),
            "--credentials" => {
                options.credentials = Some(PathBuf::from(next(&mut iter, "--credentials")?))
            }
            "--force" if allow_force => force = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok((options, force))
}

fn next<'a>(iter: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    iter.next()
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn validate(args: &[String]) -> Result<ExitCode, String> {
    let (options, _) = parse_options(args, false)?;
    let (resolved, paths) = load(&options).map_err(|error| error.to_string())?;
    if let Some(path) = &paths.config {
        println!("config:      {}", path.display());
    } else {
        println!("config:      (none — defaults + environment)");
    }
    if let Some(path) = &paths.credentials {
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

fn show(args: &[String]) -> Result<ExitCode, String> {
    let (options, _) = parse_options(args, false)?;
    let (resolved, _paths) = load(&options).map_err(|error| error.to_string())?;
    print!("{}", render(&resolved));
    Ok(ExitCode::SUCCESS)
}

fn init(args: &[String]) -> Result<ExitCode, String> {
    let (options, force) = parse_options(args, true)?;
    let config_target = config_path(options.config).ok_or_else(|| {
        "cannot determine a default config path (no HOME); pass --config".to_string()
    })?;
    let credentials_target = credentials_path(options.credentials).ok_or_else(|| {
        "cannot determine a default credentials path (no HOME); pass --credentials".to_string()
    })?;

    write_file(&config_target, &config_template(), force)?;
    write_file(&credentials_target, &credentials_template(), force)?;
    restrict(&credentials_target)?;

    println!("Wrote {}", config_target.display());
    println!("Wrote {} (chmod 600)", credentials_target.display());
    println!("\nEdit both, then run `temper config validate`.");
    Ok(ExitCode::SUCCESS)
}

fn write_file(path: &std::path::Path, contents: &str, force: bool) -> Result<(), String> {
    if path.exists() && !force {
        return Err(format!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(unix)]
fn restrict(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) -> Result<(), String> {
    Ok(())
}

/// Renders the resolved deployment for `config show`, redacting every secret.
fn render(resolved: &Resolved) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let present = |value: &Option<String>| if value.is_some() { "set" } else { "unset" };

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
