//! `anvil-agent` — the out-of-process coding agent the orchestration worker spawns.
//!
//! This binary *is* the worker ↔ agent process boundary (plane 1). It speaks the
//! `smith-agent-protocol`:
//!
//! 1. read the [`WorkspaceContext`] JSON from the file named by `CONTEXT_ENV`;
//! 2. run the native sans-IO coding loop in the current directory (the prepared
//!    checkout the worker handed us as cwd);
//! 3. emit [`StepProgress`] records as line-delimited JSON on **stdout** — each
//!    a crash-recovery checkpoint the worker relays to the forge;
//! 4. write the [`WorkspaceResult`] JSON to the file named by `RESULT_ENV`.
//!
//! The agent has git credentials only via the prepared checkout (to push), and
//! never talks to the forge API — the worker owns that. Anything real-time
//! (token deltas, steering) belongs to the out-of-band control plane, not this
//! binary's stdout.
//!
//! Auth/iteration knobs come from flags, mirroring the former in-process runner:
//! `--auth <deepseek|chatgpt-oauth|anthropic-oauth>` `--auth-file <path>`
//! `--codex-model <id>` `--max-iterations <n>` `--config-dir <path>`
//! `--enable-subagents`.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anvil_temper_agent::{
    AuthChoice, CodingAgentError, DEFAULT_MAX_ITERATIONS, ProviderConfig,
    run_coding_agent_native_with_options,
};
use smith_agent_protocol::{
    CONTEXT_ENV, PROTOCOL_VERSION, RESULT_ENV, StepProgress, StepState, WorkspaceContext,
    WorkspaceResult,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // stderr carries diagnostics; stdout is reserved for the framed
            // step-progress stream so the worker can parse it cleanly.
            eprintln!("anvil-agent: {error}");
            ExitCode::from(2)
        }
    }
}

struct Options {
    auth: AuthChoice,
    codex_model: Option<String>,
    auth_file: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    max_iterations: usize,
    enable_subagents: bool,
}

fn run() -> Result<(), String> {
    let options = Options::parse(std::env::args().skip(1))?;

    let context_path = std::env::var(CONTEXT_ENV)
        .map_err(|_| format!("missing required env var {CONTEXT_ENV} (context file path)"))?;
    let result_path = std::env::var(RESULT_ENV)
        .map_err(|_| format!("missing required env var {RESULT_ENV} (result file path)"))?;

    let context_bytes = std::fs::read(&context_path)
        .map_err(|error| format!("read context file {context_path}: {error}"))?;
    let context: WorkspaceContext = serde_json::from_slice(&context_bytes)
        .map_err(|error| format!("parse context file {context_path}: {error}"))?;

    // Emit the Started checkpoint before any preamble (auth, cwd) so the worker
    // sees the correlation/start even if credential preflight fails.
    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: 1,
        status: format!("start {} run", context.work_item.role),
        state: StepState::Started,
        pushed_sha: None,
        note: Some(format!("protocol v{PROTOCOL_VERSION}")),
    });

    let provider = ProviderConfig::from_auth(options.auth, options.codex_model, options.auth_file)
        .map_err(|error| format!("provider preflight: {error}"))?;

    // The checkout is our cwd: the worker runs us there, exactly as the legacy
    // file-protocol coder was run.
    let cwd = std::env::current_dir().map_err(|error| format!("resolve cwd: {error}"))?;

    let result = anvil_io_engine::block_on(async move {
        run_coding_agent_native_with_options(
            &provider,
            &context,
            &cwd,
            options.max_iterations,
            options.config_dir.as_deref(),
            options.enable_subagents,
        )
        .await
        .map(|result| {
            (
                result,
                context.correlation_key.clone(),
                context.work_item.role.clone(),
            )
        })
    });

    let (result, correlation_key, role) = match result {
        Ok(value) => value,
        Err(error) => return Err(describe_agent_error(&error)),
    };

    emit(&StepProgress {
        correlation_key,
        step: 2,
        status: format!("finish {role} run"),
        state: StepState::Done,
        pushed_sha: None,
        note: result.summary.clone(),
    });

    write_result(&result_path, &result)
}

/// Writes one step-progress record as a single JSON line to stdout and flushes,
/// so the worker sees checkpoints live rather than buffered to process exit.
fn emit(progress: &StepProgress) {
    match progress.to_line() {
        Ok(line) => {
            let mut stdout = std::io::stdout().lock();
            // A failed write to the worker's pipe must not abort the run; the
            // run's product (the result file + pushed commits) is what matters.
            let _ = writeln!(stdout, "{line}");
            let _ = stdout.flush();
        }
        Err(error) => eprintln!("anvil-agent: serialize step-progress: {error}"),
    }
}

fn write_result(result_path: &str, result: &WorkspaceResult) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(result).map_err(|error| format!("serialize result: {error}"))?;
    std::fs::write(result_path, bytes)
        .map_err(|error| format!("write result file {result_path}: {error}"))
}

/// Renders a coding-agent error for stderr. The worker re-derives the
/// transient/permanent class from the process exit (non-zero ⇒ transient) plus
/// a missing result file (⇒ permanent); the message here is for humans.
fn describe_agent_error(error: &CodingAgentError) -> String {
    format!("coding agent failed: {error}")
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut auth = AuthChoice::ChatGptOAuth;
        let mut codex_model = None;
        let mut auth_file = None;
        let mut config_dir = None;
        let mut max_iterations = DEFAULT_MAX_ITERATIONS;
        let mut enable_subagents = false;

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--auth" => auth = parse_auth(&value(&mut iter, "--auth")?)?,
                "--codex-model" => codex_model = Some(value(&mut iter, "--codex-model")?),
                "--auth-file" => auth_file = Some(PathBuf::from(value(&mut iter, "--auth-file")?)),
                "--config-dir" => {
                    config_dir = Some(PathBuf::from(value(&mut iter, "--config-dir")?))
                }
                "--max-iterations" => {
                    let raw = value(&mut iter, "--max-iterations")?;
                    max_iterations = raw.parse::<usize>().map_err(|_| {
                        format!("--max-iterations expects a positive integer, got `{raw}`")
                    })?;
                    if max_iterations == 0 {
                        return Err("--max-iterations must be greater than zero".to_string());
                    }
                }
                "--enable-subagents" => enable_subagents = true,
                "--help" | "-h" => return Err(USAGE.to_string()),
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }

        Ok(Self {
            auth,
            codex_model,
            auth_file,
            config_dir,
            max_iterations,
            enable_subagents,
        })
    }
}

const USAGE: &str = "anvil-agent [--auth <deepseek|chatgpt-oauth|anthropic-oauth>] [--auth-file <path>] [--codex-model <id>] [--max-iterations <n>] [--config-dir <path>] [--enable-subagents]\n  reads context from $TEMPER_CODING_WORKSPACE_CONTEXT, runs in cwd, writes result to $TEMPER_CODING_WORKSPACE_RESULT, emits step-progress JSON lines on stdout";

fn value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_auth(value: &str) -> Result<AuthChoice, String> {
    match value {
        "deepseek" => Ok(AuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unknown --auth `{other}` (expected deepseek|chatgpt-oauth|anthropic-oauth)"
        )),
    }
}
