// SPDX-License-Identifier: MPL-2.0

//! `temper config` — configuration inspection and compatibility helpers.
//!
//! - `show` — print the effective resolved deployment, with secrets redacted.
//! - `paths` — print the config, secret, state, workspace, and workflow paths
//!   Temper will use.
//! - `schema` — print the canonical JSON Schema for `config.toml`.
//! - `validate` — compatibility path for top-level `temper check`: load the
//!   config + credentials, resolve them, and report any problems (and advisory
//!   notes) without starting anything.
//! - `init` — legacy bare-template writer. Public onboarding should use
//!   top-level `temper init`, which writes a complete deployment bundle.
//!
//! This crate owns only argv parsing, terminal output, and exit codes; the
//! config schema, resolution, and writing all live in [`temper_config`], and the
//! shared file-writing/exit-code helpers in [`temper_cli_common`].

mod check;
mod paths;
mod schema;

use std::process::ExitCode;

use temper_cli_common::{
    EX_USAGE, EnvMap, LoadOptions, OutputFormat, PathResolver, WriteOutcome, resolve_targets,
    restrict_600, run, write_new_file,
};
use temper_config::{
    ConfigError, LoadInputs, LoadedPaths, ProviderCredential, Resolved, SecretReference,
    WebUiCreds, config_template, credentials_template, lint, load_explicit_with_secret_validation,
};

pub use check::{CheckInputs, check};

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
    load_for_with_secret_validation(options, env, paths, true)
}

pub(crate) fn load_for_with_secret_validation(
    options: &LoadOptions,
    env: &EnvMap,
    paths: &PathResolver,
    validate_secret_references: bool,
) -> Result<(Resolved, LoadedPaths), ConfigError> {
    let explicit = options.config.is_some() || options.credentials.is_some();
    let empty = PathResolver::default();
    let paths: &PathResolver = if explicit { &empty } else { paths };
    load_explicit_with_secret_validation(
        &LoadInputs {
            explicit_config: options.config.clone(),
            explicit_credentials: options.credentials.clone(),
            env,
            paths,
        },
        validate_secret_references,
    )
}

pub const USAGE: &str = "\
Configuration inspection utilities.

Usage: temper [GLOBAL OPTIONS] config <COMMAND> [OPTIONS]

Commands:
  show      Print the effective resolved configuration (secrets redacted)
  paths     Print resolved config, secret, state, workspace, and workflow paths
  schema    Print the canonical JSON Schema for config.toml

Compatibility commands:
  validate  Legacy offline validation path; prefer top-level `temper check`
  init      Legacy bare-template writer; prefer top-level `temper init`

Options:
  --force     (compat init) overwrite existing files
  -h, --help  Print help

Onboarding flow: `temper init` -> `temper check` -> `temper plan` ->
`temper apply` -> `temper serve`.

Global options:
  --format <human|json>  `temper check` and `config paths` output format; schema always emits JSON; accepted before the command only";

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
    check::print_validation_human(&loaded, &findings);
    if check::has_error_findings(&findings) {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
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

fn render_secret_reference(reference: Option<&SecretReference>) -> String {
    match reference {
        Some(reference) => format!(
            "{} ({})",
            reference.name,
            if reference.available {
                "available"
            } else {
                "missing"
            }
        ),
        None => "(unset)".to_string(),
    }
}

fn render_capture_mode(mode: temper_config::CaptureModeV1) -> &'static str {
    match mode {
        temper_config::CaptureModeV1::Off => "off",
        temper_config::CaptureModeV1::Metadata => "metadata",
        temper_config::CaptureModeV1::Transcript => "transcript",
        temper_config::CaptureModeV1::Diagnostic => "diagnostic",
    }
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
    let _ = writeln!(
        out,
        "  forge_token  = {}",
        render_secret_reference(resolved.engine.forge_token.as_ref())
    );
    let _ = writeln!(out, "  daemon_id    = {}", resolved.engine.daemon_id);
    let _ = writeln!(
        out,
        "  webhook_secret = {}",
        render_secret_reference(resolved.engine.webhook_secret.as_ref())
    );
    let _ = writeln!(
        out,
        "  webhook_file = {}",
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
        "  result_root  = {}",
        resolved.worker.result_root.display()
    );
    let _ = writeln!(
        out,
        "  max_no_progress_secs = {}",
        resolved.worker.liveness_limits.max_no_progress.as_secs()
    );
    let _ = writeln!(
        out,
        "  max_run_secs = {}",
        resolved
            .worker
            .liveness_limits
            .max_run
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    let _ = writeln!(
        out,
        "  cancellation_graces_secs = {} graceful + {} forced",
        resolved
            .worker
            .liveness_limits
            .graceful_cancellation_grace
            .as_secs(),
        resolved
            .worker
            .liveness_limits
            .forced_termination_grace
            .as_secs()
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
    let _ = writeln!(out, "  pools        = {}", resolved.worker.pools.len());
    for pool in &resolved.worker.pools {
        let repos = pool
            .repos
            .iter()
            .map(|repo| repo.display())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "    - {}: roles=[{}], repos=[{}], max_jobs={}, agent_profile={}, worker_token={}",
            pool.name,
            pool.roles.join(", "),
            repos,
            pool.max_concurrent_jobs
                .map(|jobs| jobs.to_string())
                .unwrap_or_else(|| "(unset)".to_string()),
            pool.agent_profile.as_deref().unwrap_or("(unset)"),
            render_secret_reference(pool.worker_token.as_ref())
        );
    }

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

    let traces = &resolved.observability.agent_traces;
    let _ = writeln!(out, "\n[observability.agent_traces]");
    let _ = writeln!(
        out,
        "  capture       = {}",
        render_capture_mode(traces.policy.capture)
    );
    let _ = writeln!(out, "  retention_days = {}", traces.policy.retention_days);
    let _ = writeln!(out, "  max_run_bytes = {}", traces.policy.max_run_bytes);
    let _ = writeln!(
        out,
        "  capture_thinking = {}",
        traces.policy.capture_thinking
    );
    let _ = writeln!(
        out,
        "  read_token    = {}",
        render_secret_reference(traces.read_token.as_ref())
    );
    let _ = writeln!(
        out,
        "  transcript_queries = {}",
        if traces.transcript_queries_enabled() {
            "enabled"
        } else {
            "disabled"
        }
    );
    let _ = writeln!(
        out,
        "  engine_journal = {}",
        traces
            .engine_journal_root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(unavailable; tracing disabled)".to_string())
    );
    let _ = writeln!(
        out,
        "  worker_spool = {}",
        traces
            .worker_spool_root
            .as_deref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "(unavailable; tracing disabled)".to_string())
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
    let _ = writeln!(
        out,
        "  deadlines_secs = tool {}, model_connect {}, model_idle {}",
        resolved.agent.operation_limits.tool_timeout.as_secs(),
        resolved
            .agent
            .operation_limits
            .model_connect_timeout
            .as_secs(),
        resolved.agent.operation_limits.model_idle_timeout.as_secs()
    );
    let _ = writeln!(out, "  profiles     = {}", resolved.agent.profiles.len());
    for (name, profile) in &resolved.agent.profiles {
        let command = if profile.command.is_empty() {
            "(unset)".to_string()
        } else {
            format!("[{}]", profile.command.join(" "))
        };
        let _ = writeln!(
            out,
            "    - {}: command={}, provider={}, model={}, investigate={}, url={}, max_iters={}, subagents={}, deadlines_secs=tool {}/connect {}/idle {}, credential={}",
            name,
            command,
            profile
                .provider
                .map(|provider| provider.as_str())
                .unwrap_or("(unset)"),
            profile.model.as_deref().unwrap_or("(unset)"),
            profile.investigate_model.as_deref().unwrap_or("(unset)"),
            profile.provider_url.as_deref().unwrap_or("(unset)"),
            profile
                .max_iterations
                .map(|iterations| iterations.to_string())
                .unwrap_or_else(|| "(unset)".to_string()),
            profile
                .subagents
                .map(|enabled| enabled.to_string())
                .unwrap_or_else(|| "(unset)".to_string()),
            profile.operation_limits.tool_timeout.as_secs(),
            profile.operation_limits.model_connect_timeout.as_secs(),
            profile.operation_limits.model_idle_timeout.as_secs(),
            render_secret_reference(profile.credential.as_ref())
        );
    }

    out
}
