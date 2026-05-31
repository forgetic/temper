//! Parsed argument *types* for the `harness-testing-worker` binary.
//!
//! This module owns the public value types ([`WorkerArgs`], [`WorkerKind`],
//! [`Backend`], the behavior enums, the secret env-var names) and re-exports the
//! parsing entry points ([`parse`], [`parse_with_env`]) from the sibling
//! [`super::args_parse`] module, which holds the hand-rolled, table-free flag
//! walker. The split keeps each file within the line budget.
//!
//! A heavyweight CLI framework would be the only reason to add a new dependency
//! to this crate, so the parser stays deliberately small. Keep it
//! dependency-light; if the surface grows past a handful of flags, reconsider a
//! small lockfile crate rather than hand-rolling more.

use chrono::Duration;
use std::fmt;
use std::path::PathBuf;

/// Which worker a binary invocation should run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerKind {
    /// One-shot: create the repository and upsert every workflow label.
    Provision,
    /// Per-role worker servicing a single workflow role.
    Role {
        /// Workflow role id this worker services.
        role: String,
        /// Forge user handle the worker acts as.
        user: String,
        /// Which fake agent variants populate this worker's registry.
        behavior: RoleBehavior,
    },
    /// Controller-plane mechanical reconcile/apply worker.
    Mechanical,
    /// Test-only fake CI producer.
    Ci {
        /// CI verdict policy.
        policy: CiPolicyKind,
    },
}

/// CI producer policy selectable from the command line via `--ci`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum CiPolicyKind {
    /// Every visited pull request passes (the default).
    #[default]
    Pass,
    /// Fail the first verdict per head, then pass.
    FailThenPass,
    /// Always fail.
    FixedFail,
}

/// Which fake architect variant a `role` worker registers (`--architect`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ArchitectKind {
    /// Reconciles landed PRs but leaves produced parent issues open (default).
    #[default]
    Default,
    /// Also closes a merged PR's produced parent issues, unblocking dependents.
    Closing,
}

/// Which fake reviewer variant a `role` worker registers (`--reviewer`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ReviewerKind {
    /// Approves on the first review (default).
    #[default]
    Default,
    /// Requests changes on the first review, approves on the next.
    RequestChangesThenApprove,
}

/// Whether the Forgejo engineer seeds the CI sentinel (`ci-ok`) at PR-open time
/// or withholds it until the fix commit (`--ci-sentinel`).
///
/// Forgejo-only: the filesystem path never reads this (it has no real CI). The
/// committed workflow gates the `build` job on `test -f ci-ok`, so a head with
/// the sentinel passes and one without fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum CiSentinelKind {
    /// Seed `ci-ok` on the head when the PR is opened, so its single CI run
    /// passes immediately (the default; happy path and its review/dependency
    /// variants).
    #[default]
    Present,
    /// Do **not** seed `ci-ok` at PR-open: the first head fails CI, and the
    /// engineer's `address_ci_failure` fix commit adds the sentinel to produce a
    /// second, passing head SHA (`ci_fails_then_passes`).
    Deferred,
}

/// The fake agent variants that populate a `role` worker's registry.
///
/// Only the architect and reviewer have behavior variants; every other role
/// uses its single fake. These map one-to-one onto the in-process scenario
/// wiring in `harness-runner/tests/end_to_end.rs` so the same scenarios converge
/// across both topologies (see `docs/how-to/run-multiprocess-e2e.md`).
///
/// `ci_sentinel` is Forgejo-only and only the engineer reads it; it does not
/// affect the filesystem topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct RoleBehavior {
    /// Architect variant.
    pub architect: ArchitectKind,
    /// Reviewer variant.
    pub reviewer: ReviewerKind,
    /// Forgejo engineer CI-sentinel policy (`--ci-sentinel`).
    pub ci_sentinel: CiSentinelKind,
}

/// Which agent registry a `role` worker populates (`--agents`).
///
/// `fake` (the default everywhere) uses the deterministic behavior-only fakes, so
/// the filesystem topology and the no-LLM Forgejo e2e are unchanged. `real` uses
/// the in-process DeepSeek-backed LLM agents from `harness-agents`; it reads the
/// API key at runtime (file/env) and is only exercised by the double-gated
/// real-agent e2e. The architect/reviewer behavior flags still select variants in
/// either mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum AgentsKind {
    /// Deterministic fake agents (the default).
    #[default]
    Fake,
    /// Real, in-process LLM agents (DeepSeek via `harness-agents`).
    Real,
}

impl AgentsKind {
    /// Human-readable flag value, for error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentsKind::Fake => "fake",
            AgentsKind::Real => "real",
        }
    }
}

/// Which credential the real (`--agents real`) LLM agents authenticate with
/// (`--auth`).
///
/// This is a **test/dev surface**, so it defaults to [`AgentsAuthKind::ChatGptOAuth`]
/// per the cost policy (a flat ChatGPT subscription instead of pay-per-token
/// DeepSeek). It maps onto [`harness_agents::AuthChoice`] when the registry is
/// built; it is irrelevant under `--agents fake` (no provider is constructed).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum AgentsAuthKind {
    /// ChatGPT (OpenAI Codex) OAuth subscription (the test/dev default).
    #[default]
    ChatGptOAuth,
    /// DeepSeek API key (pay-per-token fallback).
    DeepSeek,
}

impl AgentsAuthKind {
    /// Human-readable flag value, for error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentsAuthKind::ChatGptOAuth => "chatgpt-oauth",
            AgentsAuthKind::DeepSeek => "deepseek",
        }
    }
}

/// Environment variable selecting the real-agent auth mode (the launch-script
/// bridge from a config file). A `--auth` CLI flag overrides it; absent both, the
/// test/dev default ([`AgentsAuthKind::ChatGptOAuth`]) applies.
pub const AGENTS_AUTH_ENV: &str = "HARNESS_AGENTS_AUTH";

/// Which clock a poll-loop worker drives its ticks from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ClockKind {
    /// Deterministic [`ManualClock`](harness_runner::ManualClock) seeded near the
    /// filesystem backend logical-clock origin (the default).
    #[default]
    Deterministic,
    /// Wall-clock time, the mode a real provider backend would use.
    Wall,
}

/// Environment variable carrying the per-role Forgejo access token.
///
/// Secrets never travel on argv (other processes can read a command line); the
/// Phase 4 spawner sets this per child. Read only for `--backend forgejo`.
pub const FORGEJO_TOKEN_ENV: &str = "HARNESS_FORGEJO_TOKEN";

/// Environment variable carrying the web-UI login username for CI reads.
///
/// Optional: only the CI-reading role(s) need it (the Phase 3b password/web-UI
/// CI read path logs in with username + password). Other roles act through the
/// token alone.
pub const FORGEJO_USERNAME_ENV: &str = "HARNESS_FORGEJO_USERNAME";

/// Environment variable carrying the web-UI login password for CI reads.
///
/// Optional, paired with [`FORGEJO_USERNAME_ENV`]; same rationale as the token —
/// passed via env, never argv.
pub const FORGEJO_PASSWORD_ENV: &str = "HARNESS_FORGEJO_PASSWORD";

/// Which Forge backend a worker process builds its handle against (`--backend`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum BackendKind {
    /// The local [`FilesystemForge`](harness_forge_filesystem::FilesystemForge)
    /// store shared across processes by path (the default, so the existing
    /// multiprocess test is untouched).
    #[default]
    Filesystem,
    /// A real Forgejo server reached over HTTP via
    /// [`ForgejoForge`](harness_forge_forgejo::ForgejoForge).
    Forgejo,
}

impl BackendKind {
    /// Human-readable flag value, for error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Filesystem => "filesystem",
            BackendKind::Forgejo => "forgejo",
        }
    }
}

/// Forgejo connection details for `--backend forgejo`.
///
/// The base URL is the only piece that arrives on argv; every credential is read
/// from the environment ([`FORGEJO_TOKEN_ENV`], [`FORGEJO_USERNAME_ENV`],
/// [`FORGEJO_PASSWORD_ENV`]) so it never appears in a process command line. The
/// [`std::fmt::Debug`] impl redacts the secrets.
#[derive(Clone, Eq, PartialEq)]
pub struct ForgejoArgs {
    /// Forgejo base URL, e.g. `http://127.0.0.1:3000`.
    pub base_url: String,
    /// Per-role access token (from [`FORGEJO_TOKEN_ENV`]); REST identity.
    pub token: String,
    /// Optional web-UI login username (from [`FORGEJO_USERNAME_ENV`]) for the
    /// Phase 3b CI read path; `None` for roles that do not read CI.
    pub username: Option<String>,
    /// Optional web-UI login password (from [`FORGEJO_PASSWORD_ENV`]); paired
    /// with [`Self::username`].
    pub password: Option<String>,
}

impl fmt::Debug for ForgejoArgs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ForgejoArgs")
            .field("base_url", &self.base_url)
            .field("token", &Redacted(self.token.is_empty()))
            .field("username", &self.username)
            .field("password", &Redacted(self.password.is_none()))
            .finish()
    }
}

/// Debug helper that never prints a secret, only whether one is present.
struct Redacted(bool);

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 {
            formatter.write_str("<unset>")
        } else {
            formatter.write_str("<redacted>")
        }
    }
}

/// Which backend a worker builds its handle against, plus backend-specific data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Backend {
    /// Filesystem store under [`WorkerArgs::root`].
    Filesystem,
    /// Forgejo server with connection + credentials.
    Forgejo(ForgejoArgs),
}

impl Backend {
    /// The flag value this backend corresponds to.
    pub fn kind(&self) -> BackendKind {
        match self {
            Backend::Filesystem => BackendKind::Filesystem,
            Backend::Forgejo(_) => BackendKind::Forgejo,
        }
    }
}

/// Fully parsed and validated worker invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerArgs {
    /// Which worker to run.
    pub kind: WorkerKind,
    /// Which Forge backend to build a handle against.
    pub backend: Backend,
    /// Filesystem store root shared by every worker process. Used by
    /// `--backend filesystem`; ignored by `--backend forgejo`.
    pub root: PathBuf,
    /// Repository owner.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Poll cadence between ticks.
    pub poll_interval: Duration,
    /// Sentinel file whose existence stops the run loop.
    pub stop_file: Option<PathBuf>,
    /// Maximum wall-clock seconds to run before stopping; `None` runs until the
    /// stop file appears.
    pub run_secs: Option<u64>,
    /// Clock fidelity for poll-loop ticks.
    pub clock: ClockKind,
    /// Which agent registry a `role` worker populates (fake or real LLM).
    pub agents: AgentsKind,
    /// Which credential the real LLM agents authenticate with (`--agents real`).
    pub auth: AgentsAuthKind,
    /// Codex model id override for ChatGPT OAuth (`--codex-model`); `None` falls
    /// back to `HARNESS_AGENTS_CODEX_MODEL` then the built-in default.
    pub codex_model: Option<String>,
    /// Auth-file path override for ChatGPT OAuth (`--auth-file`); `None` falls
    /// back to `HARNESS_AGENTS_AUTH_FILE` then `~/.pi/agent/auth.json`.
    pub auth_file: Option<PathBuf>,
}

/// An argument-parsing failure with a user-facing message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArgsError(String);

impl ArgsError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ArgsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArgsError {}

// The parsing logic lives in the sibling `args_parse` module (split out to keep
// each file within the line budget); re-export its surface so callers continue
// to use `args::{parse, parse_with_env, ParseOutcome, USAGE}`.
pub use super::args_parse::{parse, parse_with_env, ParseOutcome, USAGE};
