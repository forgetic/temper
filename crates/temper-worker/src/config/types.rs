use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use temper_protocol_activity::{AgentActivityCapturePolicyV1, CaptureModeV1};
use temper_protocol_worker::WorkerAuth;

use crate::workspace::RoleGitIdentity;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySpec {
    pub repo: String,
    pub role: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerConfig {
    pub daemon_url: String,
    pub worker_id: String,
    /// Selected target-era worker pool name, when this worker was shaped from a
    /// `[[worker.pools]]` policy.
    pub worker_pool: Option<String>,
    /// Selected pool bearer credential, if that pool requires authentication.
    /// The value's `Debug` output is redacted and it is sent only as transport
    /// metadata (HTTP `Authorization: Bearer …` or equivalent in-process auth),
    /// never inside worker-protocol JSON.
    pub worker_auth: Option<WorkerAuth>,
    pub capabilities: Vec<CapabilitySpec>,
    /// Role id -> git identity (user, email, push token) for the worker's
    /// per-role checkouts/pushes.
    ///
    /// This is the worker subsystem's single config input: the coding executor's
    /// identities come from here (the worker factory builds its
    /// [`CodingExecutorConfig`](crate::CodingExecutorConfig) from this map),
    /// rather than being threaded into the executor separately. The push token is
    /// the already-exposed `String` the git auth header needs — secrets are
    /// unwrapped by the adapter that builds this config (the engine-side
    /// `Resolved` keeps them as `SecretString` until that I/O boundary).
    pub role_identities: BTreeMap<String, RoleGitIdentity>,
    /// How many jobs this worker runs at once.
    ///
    /// The design point is **one job at a time** (the default, and what the
    /// examples and dogfood deploy set): a worker claims a ticket, works it to
    /// completion, then claims the next — Forgejo-runner-shaped. A worker spends
    /// most of a job blocked on LLM latency, so to run more jobs in parallel the
    /// intended path is **more worker processes** (dozens is fine on one host),
    /// not more concurrency inside one worker. Values >1 are still honored — the
    /// capacity bookkeeping in [`crate::worker_machine::WorkerMachine`] is
    /// invariant-checked and fuzzed for any value — but they are not the design
    /// point. If a single worker ever genuinely needs several *top-level* agents,
    /// the cleaner move is a per-job fan-out (tag completions with a job id and
    /// route to a child machine) rather than relying on this knob.
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    /// Worker-owned job supervision limits.
    pub liveness_limits: WorkerLivenessLimits,
    /// Private durable root for terminal result/outbox state.
    pub result_root: PathBuf,
    /// Effective capture policy and durable worker spool root.
    pub agent_traces: WorkerAgentTraceConfig,
    pub executor: ExecutorSelection,
}

/// Worker-owned trace collector/spool configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerAgentTraceConfig {
    pub policy: AgentActivityCapturePolicyV1,
    pub spool_root: Option<PathBuf>,
}

impl Default for WorkerAgentTraceConfig {
    fn default() -> Self {
        let policy = AgentActivityCapturePolicyV1 {
            capture: CaptureModeV1::Off,
            ..Default::default()
        };
        Self {
            policy,
            spool_root: None,
        }
    }
}

/// Worker-owned job liveness and process-termination limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerLivenessLimits {
    pub max_no_progress: Duration,
    pub max_run: Option<Duration>,
    pub graceful_cancellation_grace: Duration,
    pub forced_termination_grace: Duration,
}

impl Default for WorkerLivenessLimits {
    fn default() -> Self {
        Self {
            max_no_progress: Duration::from_secs(900),
            max_run: None,
            graceful_cancellation_grace: Duration::from_secs(10),
            forced_termination_grace: Duration::from_secs(5),
        }
    }
}

/// Creates the durable result root and restricts it to the worker account.
pub fn prepare_result_root(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("create worker result root {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                format!(
                    "set private permissions on worker result root {}: {error}",
                    path.display()
                )
            },
        )?;
    }
    Ok(())
}

/// Default backoff before re-polling after the daemon returned no work, the
/// long-poll timed out, a transport error occurred, or the worker was at
/// capacity. Small so a freed slot is picked up promptly, but non-zero so an
/// idle or erroring worker does not hot-loop the daemon. (The steady-state pace
/// when work is flowing is set by the long-poll `max_wait_ms`, not this.)
pub const DEFAULT_POLL_BACKOFF: Duration = Duration::from_millis(500);

/// Identity + cadence knobs the pure [`WorkerMachine`](crate::worker_machine::WorkerMachine)
/// needs: a projection of [`WorkerConfig`] without the daemon URL, executor
/// selection, or any transport.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerParams {
    pub worker_id: String,
    pub worker_pool: Option<String>,
    pub capabilities: Vec<CapabilitySpec>,
    pub max_concurrent_jobs: u32,
    pub poll_wait: Duration,
    pub heartbeat_interval: Duration,
    pub poll_backoff: Duration,
    pub liveness_limits: WorkerLivenessLimits,
    pub result_root: PathBuf,
}

impl WorkerParams {
    /// Projects a [`WorkerConfig`] into the machine's parameters, applying the
    /// default poll backoff.
    pub fn from_config(config: &WorkerConfig) -> Self {
        Self {
            worker_id: config.worker_id.clone(),
            worker_pool: config.worker_pool.clone(),
            capabilities: config.capabilities.clone(),
            max_concurrent_jobs: config.max_concurrent_jobs,
            poll_wait: config.poll_wait,
            heartbeat_interval: config.heartbeat_interval,
            poll_backoff: DEFAULT_POLL_BACKOFF,
            liveness_limits: config.liveness_limits,
            result_root: config.result_root.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutorSelection {
    Stub,
    Coding(CodingSurface),
}

/// The coding-executor surface: the workspace/git wiring plus how one agent
/// turn is produced ([`AgentSurface`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingSurface {
    pub workspace_root: PathBuf,
    pub git_base_url: String,
    pub agent: AgentSurface,
}

/// How the coding executor produces an agent turn.
///
/// Both surfaces resolve to a **command the worker spawns out-of-process** over
/// the `smith-agent-protocol`. `--agent-command` selects the program: the
/// literal `anvil-native` selects the native anvil agent surface, which spawns
/// `anvil-agent` (overridable via `--agent-program`); any other value is
/// spawned verbatim (the examples' deterministic `greeting` stand-in, or an
/// operator-provided coder). Trailing `--agent-arg` values are the agent's
/// flags: for the anvil-native surface they are parsed here and re-rendered
/// onto the agent command (`--agent-program` / `--provider` / `--model` /
/// `--max-iterations` / `--capture-dir` / `--subagents`); for an external
/// command they are passed through verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentSurface {
    /// The native anvil coding agent, spawned out-of-process as `anvil-agent`.
    AnvilNative(AnvilNativeAgentSurface),
    /// An external program spawned per job (program first, then args).
    ExternalCommand(Vec<String>),
}

/// Parsed configuration for the anvil-native agent surface — the flags the
/// out-of-process `anvil-agent` binary parses for itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnvilNativeAgentSurface {
    /// The agent program the worker spawns. Defaults to [`TEMPER_AGENT_PROGRAM`]
    /// (`anvil-agent`, resolved on `PATH`); override with an absolute path via
    /// `--agent-program` when it is not on `PATH`.
    pub agent_program: String,
    pub provider: AgentProviderChoice,
    pub model: Option<String>,
    pub capture_dir: Option<PathBuf>,
    pub max_iterations: Option<usize>,
    /// Enable the in-workspace `investigate` sub-agent tool (off by default).
    pub enable_subagents: bool,
}

/// Which provider the agent authenticates against. Mirrors the agent's
/// `--provider` flag values but is parsed in the worker (which links no agent
/// code); the worker renders it back to `--provider` in
/// [`AnvilNativeAgentSurface::into_command`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderChoice {
    DeepSeek,
    ChatGpt,
    Anthropic,
}

// `Run(WorkerConfig)` is far larger than `Help`, but `ParseOutcome` is produced
// exactly once at process start and immediately destructured — the size
// difference never matters, and boxing would only obscure the config flow.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Help,
    Run(WorkerConfig),
}
