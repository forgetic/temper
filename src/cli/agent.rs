//! `temper-agent` — the out-of-process coding agent the orchestration worker spawns.
//!
//! This binary *is* the worker ↔ agent process boundary (plane 1). It speaks the
//! `temper-agent-protocol`:
//!
//! 1. read the [`WorkspaceContext`] JSON from the file named by `CONTEXT_ENV`;
//! 2. run the native sans-IO coding loop in the current directory (the prepared
//!    checkout the worker handed us as cwd);
//! 3. emit [`StepProgress`] records as line-delimited JSON on **stdout** — each
//!    a crash-recovery checkpoint the worker relays to the forge. On writable
//!    jobs the run **checkpoints**: before each model turn it commits + pushes
//!    whatever the previous turn changed and emits a marker carrying the
//!    pushed sha (the marker never claims more than the branch holds). A
//!    re-dispatched agent finds those commits on the prepared branch, resumes
//!    its step numbering from them, and tells the model what already landed;
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use temper_agent_runtime::{
    AuthChoice, CodingAgentError, DEFAULT_MAX_ITERATIONS, ProviderConfig,
    run_coding_agent_native_with_hooks,
};
use temper_agent_protocol::{
    CONTEXT_ENV, PROTOCOL_VERSION, RESULT_ENV, StepProgress, StepState, WorkspaceContext,
    WorkspaceResult,
};

pub fn main<I>(args: I) -> ExitCode
where
    I: Iterator<Item = String>,
{
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // stderr carries diagnostics; stdout is reserved for the framed
            // step-progress stream so the worker can parse it cleanly.
            eprintln!("temper-agent: {error}");
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

fn run<I>(args: I) -> Result<(), String>
where
    I: Iterator<Item = String>,
{
    let options = Options::parse(args)?;

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

    // Writable jobs checkpoint: commit + push at turn boundaries, resume from
    // prior checkpoints found on the prepared branch. Read-only jobs never
    // mutate the tree, so they run hook-less.
    let writable = context
        .checkout
        .as_deref()
        .map(|capability| capability == "writable")
        .unwrap_or(true);
    let checkpointer = writable.then(|| Arc::new(Checkpointer::new(&cwd, &context)));
    let resume = checkpointer.as_deref().and_then(Checkpointer::detect_resume);
    if let Some(resume) = &resume {
        emit(&StepProgress {
            correlation_key: context.correlation_key.clone(),
            step: resume.last_step + 1,
            status: format!("resume {} run from pushed checkpoints", context.work_item.role),
            state: StepState::Started,
            pushed_sha: Some(resume.head_sha.clone()),
            note: Some(format!("{} checkpoint commit(s) on the branch", resume.commits)),
        });
    }
    if let Some(checkpointer) = &checkpointer {
        checkpointer.start_after(resume.as_ref().map(|resume| resume.last_step + 1));
    }
    let resume_note = resume.as_ref().map(|resume| {
        format!(
            "A previous run of this task was interrupted after pushing {} checkpoint              commit(s); the working tree already reflects them:\n{}\nContinue from              that state — do not redo work those commits already contain.",
            resume.commits, resume.log
        )
    });

    let result = {
        // Clone the values the run consumes so the originals survive for the
        // terminal marker below; the closure moves only these clones.
        let run_context = context.clone();
        let run_checkpointer = checkpointer.clone();
        let run_cwd = cwd.clone();
        temper_agent_io_engine::block_on_with(move |_cx, handle| async move {
            run_coding_agent_native_with_hooks(
                handle,
                &provider,
                &run_context,
                &run_cwd,
                options.max_iterations,
                options.config_dir.as_deref(),
                options.enable_subagents,
                resume_note.as_deref(),
                run_checkpointer.map(|hook| hook as Arc<dyn temper_agent_core::TurnHook>),
            )
            .await
        })
    };

    let result = match result {
        Ok(value) => value,
        Err(error) => return Err(describe_agent_error(&error)),
    };

    // The terminal marker takes the next free step index (after any
    // checkpoints the run pushed).
    let final_step = checkpointer
        .as_ref()
        .map(|checkpointer| checkpointer.next_step())
        .unwrap_or(2);
    emit(&StepProgress {
        correlation_key: context.correlation_key.clone(),
        step: final_step,
        status: format!("finish {} run", context.work_item.role),
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
        Err(error) => eprintln!("temper-agent: serialize step-progress: {error}"),
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
///
/// A model-unavailability rejection (a suspended model alias, or a tier that
/// does not grant the configured model) is flagged with an explicit
/// `model-unavailable:` prefix so it stands out in logs as a configuration
/// problem the `Display` text already explains how to fix — not a transient
/// network blip to be retried blindly.
fn describe_agent_error(error: &CodingAgentError) -> String {
    match error {
        CodingAgentError::ModelUnavailable { .. } => {
            format!("model-unavailable: {error}")
        }
        _ => format!("coding agent failed: {error}"),
    }
}

/// A prior run's pushed checkpoints, recovered from the prepared branch.
struct ResumeState {
    /// Commits beyond the base branch.
    commits: u32,
    /// The highest step index encoded in checkpoint commit subjects (or the
    /// commit count + 1 when none parse), so new markers stay monotonic.
    last_step: u32,
    /// Current branch head (what the checkpoints pushed).
    head_sha: String,
    /// One subject line per checkpoint commit, oldest first.
    log: String,
}

/// Commits + pushes the working tree at turn boundaries and emits the
/// matching step-progress markers (phase 6b).
///
/// Awaited by the agent shell immediately before each model call — the
/// previous turn's tool batch has fully drained, so the tree is quiescent.
/// Failures are logged to stderr and swallowed: a checkpoint is an
/// opportunistic crash-recovery point, never a reason to fail the run. The
/// marker is emitted only after the push succeeds (it never claims more than
/// the branch holds).
struct Checkpointer {
    cwd: PathBuf,
    branch: String,
    base_branch: String,
    correlation_key: String,
    /// Per-role git identity + push token from the worker-provided env
    /// (`TEMPER_FORGEJO_{USER,EMAIL,TOKEN}_<ROLE>`); absent values degrade
    /// gracefully (generic identity, credential-less push).
    user: Option<String>,
    email: Option<String>,
    token: Option<String>,
    /// The next step index to hand out (1 = the Started marker).
    step: AtomicU32,
}

impl Checkpointer {
    fn new(cwd: &Path, context: &WorkspaceContext) -> Self {
        let role_key = context.work_item.role.to_uppercase().replace('-', "_");
        let env = |prefix: &str| {
            std::env::var(format!("{prefix}_{role_key}"))
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        Self {
            cwd: cwd.to_path_buf(),
            branch: context.branch_hint.clone(),
            base_branch: context.base_branch.clone(),
            correlation_key: context.correlation_key.clone(),
            user: env("TEMPER_FORGEJO_USER"),
            email: env("TEMPER_FORGEJO_EMAIL"),
            token: env("TEMPER_FORGEJO_TOKEN"),
            step: AtomicU32::new(2),
        }
    }

    /// Continue step numbering after a resumed run's markers.
    fn start_after(&self, next: Option<u32>) {
        if let Some(next) = next {
            self.step.store(next + 1, Ordering::SeqCst);
        }
    }

    /// The next unused step index (the terminal Done marker takes it).
    fn next_step(&self) -> u32 {
        self.step.load(Ordering::SeqCst)
    }

    /// Inspects the prepared branch for checkpoints a previous run pushed.
    fn detect_resume(&self) -> Option<ResumeState> {
        let range = format!("origin/{}..HEAD", self.base_branch);
        let count: u32 = self
            .git(&["rev-list", "--count", &range])
            .ok()?
            .trim()
            .parse()
            .ok()?;
        if count == 0 {
            return None;
        }
        let log = self
            .git(&["log", "--reverse", "--format=%s", &range])
            .ok()?
            .trim()
            .to_string();
        let head_sha = self.git(&["rev-parse", "HEAD"]).ok()?.trim().to_string();
        let last_step = log
            .lines()
            .filter_map(parse_checkpoint_step)
            .max()
            .unwrap_or(count + 1);
        Some(ResumeState {
            commits: count,
            last_step,
            head_sha,
            log,
        })
    }

    /// One commit+push checkpoint; returns the pushed sha, or `None` when the
    /// tree was clean.
    fn checkpoint_sync(&self, step: u32, turn: usize) -> Result<Option<String>, String> {
        self.git(&["add", "-A"])?;
        // Exit 0 = nothing staged.
        if self.git(&["diff", "--cached", "--quiet"]).is_ok() {
            return Ok(None);
        }
        let user = self.user.as_deref().unwrap_or("temper-agent");
        let email = self.email.as_deref().unwrap_or("temper-agent@localhost");
        self.git(&[
            "-c",
            &format!("user.name={user}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-m",
            &format!("checkpoint(step {step}): work in progress (turn {turn})"),
        ])?;
        let push_ref = format!("HEAD:refs/heads/{}", self.branch);
        match self.token.as_deref() {
            Some(token) => self.git(&[
                "-c",
                &format!("http.extraheader=AUTHORIZATION: token {token}"),
                "push",
                "origin",
                &push_ref,
            ])?,
            None => self.git(&["push", "origin", &push_ref])?,
        };
        Ok(Some(self.git(&["rev-parse", "HEAD"])?.trim().to_string()))
    }

    /// Runs git in the checkout, returning stdout. Errors carry the command
    /// and stderr with the push token redacted.
    fn git(&self, args: &[&str]) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .output()
            .map_err(|error| format!("spawn git: {error}"))?;
        if !output.status.success() {
            let mut detail = format!(
                "git {} failed ({}): {}",
                args.first().copied().unwrap_or(""),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            if let Some(token) = self.token.as_deref() {
                detail = detail.replace(token, "<redacted>");
            }
            return Err(detail);
        }
        String::from_utf8(output.stdout).map_err(|error| format!("git output not UTF-8: {error}"))
    }
}

#[async_trait::async_trait]
impl temper_agent_core::TurnHook for Checkpointer {
    async fn before_model_call(&self, turn: usize) {
        // Turn 0 is the first model call: nothing has run yet.
        if turn == 0 {
            return;
        }
        let step = self.step.fetch_add(1, Ordering::SeqCst);
        let this = CheckpointJob {
            cwd: self.cwd.clone(),
            branch: self.branch.clone(),
            base_branch: self.base_branch.clone(),
            correlation_key: self.correlation_key.clone(),
            user: self.user.clone(),
            email: self.email.clone(),
            token: self.token.clone(),
        };
        let outcome =
            skein::runtime::spawn_blocking(move || this.into_checkpointer().checkpoint_sync(step, turn))
                .await;
        match outcome {
            Ok(Some(sha)) => emit(&StepProgress {
                correlation_key: self.correlation_key.clone(),
                step,
                status: format!("push checkpoint (turn {turn})"),
                state: StepState::Done,
                pushed_sha: Some(sha),
                note: None,
            }),
            Ok(None) => {
                // Clean tree: hand the unused step index back when possible
                // (best effort — a concurrent bump just leaves a gap).
                let _ = self.step.compare_exchange(
                    step + 1,
                    step,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            Err(error) => {
                eprintln!("temper-agent: checkpoint skipped: {error}");
            }
        }
    }
}

/// The owned snapshot a blocking checkpoint runs from.
struct CheckpointJob {
    cwd: PathBuf,
    branch: String,
    base_branch: String,
    correlation_key: String,
    user: Option<String>,
    email: Option<String>,
    token: Option<String>,
}

impl CheckpointJob {
    fn into_checkpointer(self) -> Checkpointer {
        Checkpointer {
            cwd: self.cwd,
            branch: self.branch,
            base_branch: self.base_branch,
            correlation_key: self.correlation_key,
            user: self.user,
            email: self.email,
            token: self.token,
            step: AtomicU32::new(0),
        }
    }
}

/// Parses `checkpoint(step N): …` commit subjects.
fn parse_checkpoint_step(subject: &str) -> Option<u32> {
    subject
        .strip_prefix("checkpoint(step ")?
        .split_once(')')?
        .0
        .parse()
        .ok()
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

const USAGE: &str = "temper-agent [--auth <deepseek|chatgpt-oauth|anthropic-oauth>] [--auth-file <path>] [--codex-model <id>] [--max-iterations <n>] [--config-dir <path>] [--enable-subagents]\n  reads context from $TEMPER_CODING_WORKSPACE_CONTEXT, runs in cwd, writes result to $TEMPER_CODING_WORKSPACE_RESULT, emits step-progress JSON lines on stdout";

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

#[cfg(test)]
mod tests {
    use super::parse_checkpoint_step;

    #[test]
    fn parses_checkpoint_subjects() {
        assert_eq!(
            parse_checkpoint_step("checkpoint(step 3): work in progress (turn 2)"),
            Some(3)
        );
        assert_eq!(parse_checkpoint_step("checkpoint(step 12): x"), Some(12));
        assert_eq!(parse_checkpoint_step("Implement pr-for-code-7"), None);
        assert_eq!(parse_checkpoint_step("checkpoint(step ): x"), None);
    }
}
