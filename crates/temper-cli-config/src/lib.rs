// SPDX-License-Identifier: MPL-2.0

//! `temper config` — guided or programmatic configuration.
//!
//! - `validate` — compatibility path for top-level `temper check`: load the
//!   config + credentials, resolve them, and report any problems (and advisory
//!   notes) without starting anything.
//! - `show` — print the effective resolved deployment, with secrets redacted.
//! - `paths` — print the config, secret, state, workspace, and workflow paths
//!   Temper will use.
//! - `schema` — print the canonical JSON Schema for `config.toml`.
//! - `init` — write starter `config.toml` + `credentials.toml` templates.
//!
//! This crate owns only argv parsing, terminal output, and exit codes; the
//! config schema, resolution, and writing all live in [`temper_config`], and the
//! shared file-writing/exit-code helpers in [`temper_cli_common`].

mod paths;
mod schema;

use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver, WriteOutcome, resolve_targets,
    restrict_600, run, write_new_file,
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
    /// Global file-location options parsed before `config`.
    pub options: LoadOptions,
    /// Global output format parsed before `config`.
    pub format: OutputFormat,
    /// The injected environment snapshot (used for path expansion and for
    /// systemd `CREDENTIALS_DIRECTORY` credentials discovery).
    pub env: &'a EnvMap,
    /// The injected base directories (HOME / XDG_*) for default-location discovery.
    pub paths: &'a PathResolver,
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
    /// The injected base directories (HOME / XDG_*) for default-location discovery.
    pub paths: &'a PathResolver,
}

/// Loads + resolves a deployment for `validate` / `show`, honoring the same
/// hermeticity rule the daemon uses: an explicit `--config` / `--secrets`
/// suppresses default `~/.config/temper` discovery. `CREDENTIALS_DIRECTORY` from
/// the injected env may still supply credentials when `--secrets` is absent;
/// otherwise an explicit config root may load its sibling `credentials.toml`,
/// but the operator's global credentials never ambiently layer in behind an
/// explicit deployment.
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

Usage: temper [GLOBAL OPTIONS] config <COMMAND> [OPTIONS]

Commands:
  validate  Compatibility path for `temper check` (load and validate offline)
  show      Print the effective resolved configuration (secrets redacted)
  paths     Print resolved config, secret, state, workspace, and workflow paths
  schema    Print the canonical JSON Schema for config.toml
  init      Write starter config.toml + credentials.toml templates

Options:
  --force     (init) overwrite existing files
  -h, --help  Print help

Prefer `temper check` for validation; `temper config validate` remains for compatibility.

Global options:
  --format <human|json>  `temper check` and `config paths` output format; schema always emits JSON; accepted before the command only";

pub const CHECK_USAGE: &str = "\
Validate the resolved Temper config and credentials offline.

Usage: temper [GLOBAL OPTIONS] check [OPTIONS]

Options:
  -h, --help  Print help

Global options:
  -c, --config <DIR|FILE>   Path to configuration file or bundle directory
      --secrets <DIR|FILE>  Explicit secret source directory or credentials.toml
      --format <human|json> Output format (default: human)";

pub fn main(inputs: ConfigInputs) -> ExitCode {
    let ConfigInputs {
        args,
        options,
        format,
        env,
        paths,
    } = inputs;
    let Some((action, rest)) = args.split_first() else {
        println!("{USAGE}");
        return ExitCode::from(EX_USAGE);
    };
    match action.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        "validate" => run("temper config", validate(rest, &options, env, paths)),
        "show" => run("temper config", show(rest, &options, env, paths)),
        "paths" => run(
            "temper config",
            paths::command(rest, &options, format, env, paths),
        ),
        "schema" => run("temper config", schema::command(rest)),
        "init" => run("temper config", init(rest, &options, env, paths)),
        other => {
            eprintln!("temper config: unknown command `{other}`\n\n{USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

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
        Ok(CheckAction::Run) => {
            let report = validation_report(&options, env, paths);
            match format {
                OutputFormat::Human => print_validation_human(&report.loaded, &report.findings),
                OutputFormat::Json => {
                    if let Err(error) = print_validation_json(&report.loaded, &report.findings) {
                        eprintln!("temper check: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if report.load_failed || has_error_findings(&report.findings) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("temper check: {error}\n\n{CHECK_USAGE}");
            ExitCode::from(EX_USAGE)
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum CheckAction {
    Run,
    Help,
}

fn parse_check_args(args: &[String]) -> Result<CheckAction, String> {
    match args {
        [] => Ok(CheckAction::Run),
        [arg] if matches!(arg.as_str(), "-h" | "--help" | "help") => Ok(CheckAction::Help),
        [arg, ..] => Err(format!("unexpected argument `{arg}`")),
    }
}

/// Parses config-subcommand-local flags. File-location flags are global-only and
/// are supplied via [`ConfigInputs::options`].
fn parse_options(args: &[String], allow_force: bool) -> Result<bool, String> {
    let mut force = false;
    for arg in args {
        match arg.as_str() {
            "--force" if allow_force => force = true,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(force)
}

fn validate(
    args: &[String],
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
) -> Result<ExitCode, String> {
    parse_options(args, false)?;
    let (resolved, loaded) = load_for(options, env, paths).map_err(|error| error.to_string())?;
    let findings = lint(&resolved);
    print_validation_human(&loaded, &findings);
    if has_error_findings(&findings) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
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
) -> ValidationReport {
    match load_for(options, env, paths) {
        Ok((resolved, loaded)) => ValidationReport {
            loaded,
            findings: lint(&resolved),
            load_failed: false,
        },
        Err(error) => ValidationReport {
            loaded: LoadedPaths::default(),
            findings: vec![Finding {
                error: true,
                message: error.to_string(),
            }],
            load_failed: true,
        },
    }
}

fn print_validation_human(loaded: &LoadedPaths, findings: &[Finding]) {
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

    if findings.is_empty() {
        println!("OK — no problems found.");
        return;
    }
    for Finding { error, message } in findings {
        if *error {
            println!("error: {message}");
        } else {
            println!("note:  {message}");
        }
    }
}

fn print_validation_json(loaded: &LoadedPaths, findings: &[Finding]) -> Result<(), String> {
    let status = if has_error_findings(findings) {
        "error"
    } else {
        "ok"
    };
    let config_path = loaded
        .config
        .as_ref()
        .map(|path| path.display().to_string());
    let credentials_path = loaded
        .credentials
        .as_ref()
        .map(|path| path.display().to_string());
    let findings = findings
        .iter()
        .map(|finding| {
            serde_json::json!({
                "severity": if finding.error { "error" } else { "note" },
                "message": &finding.message,
            })
        })
        .collect::<Vec<_>>();
    let report = serde_json::json!({
        "status": status,
        "result": status,
        "config_path": config_path.clone(),
        "credentials_path": credentials_path.clone(),
        "paths": {
            "config": config_path,
            "credentials": credentials_path,
        },
        "findings": findings,
    });
    let text = serde_json::to_string_pretty(&report)
        .map_err(|error| format!("serialize validation report: {error}"))?;
    println!("{text}");
    Ok(())
}

fn has_error_findings(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| finding.error)
}

fn show(
    args: &[String],
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
) -> Result<ExitCode, String> {
    parse_options(args, false)?;
    let (resolved, _loaded) = load_for(options, env, paths).map_err(|error| error.to_string())?;
    print!("{}", render(&resolved));
    Ok(ExitCode::SUCCESS)
}

fn init(
    args: &[String],
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
) -> Result<ExitCode, String> {
    let force = parse_options(args, true)?;
    let targets = resolve_targets(options, env, paths)?;

    let _ = write_new_file(&targets.config, &config_template(), force)?;
    match write_new_file(&targets.credentials, &credentials_template(), force)? {
        WriteOutcome::Created | WriteOutcome::Overwritten => restrict_600(&targets.credentials)?,
    }

    println!("Wrote {}", targets.config.display());
    println!("Wrote {} (chmod 600)", targets.credentials.display());
    println!("\nEdit both, then run `temper check`.");
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

    let _ = writeln!(out, "\n[deployment]");
    let _ = writeln!(
        out,
        "  name         = {}",
        resolved.deployment.name.as_deref().unwrap_or("(unset)")
    );
    let _ = writeln!(
        out,
        "  topology     = {}",
        resolved
            .deployment
            .topology
            .map(|topology| topology.as_str())
            .unwrap_or("(unset)")
    );

    let _ = writeln!(out, "\n[paths]");
    let _ = writeln!(
        out,
        "  state_dir    = {}",
        resolved
            .paths
            .state_dir
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unavailable)".to_string())
    );
    let _ = writeln!(
        out,
        "  workspace_dir = {}",
        resolved.paths.workspace_dir.display()
    );
    let _ = writeln!(
        out,
        "  workflow_file = {}",
        resolved
            .paths
            .workflow_file
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(bundled reference-delivery)".to_string())
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
