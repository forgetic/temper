//! Parsed argument *types* for the `temper-testing-worker` binary.
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

/// Which workflow's fake agent set a `role` worker registers (`--profile`).
///
/// The agent set must match the workflow shape the worker drives (the bundled
/// reference-delivery default, or a `--workflow` selection). `reference` uses the
/// full architect/engineer/reviewer/owner/human fakes; `basic` uses the
/// basic-delivery architect/engineer pair (no reviewer/owner/human), whose
/// transitions differ structurally (a single `triage_intake_to_code` / `open_pr`
/// rather than the reference fan-out + `claim_code`/`request_review` sequence).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ProfileKind {
    /// The reference-delivery fakes (the default; preserves existing behavior).
    #[default]
    Reference,
    /// The basic-delivery fakes (architect + engineer only).
    Basic,
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
/// wiring in `temper-runner/tests/end_to_end.rs` so the same scenarios converge
/// across both topologies.
///
/// `ci_sentinel` is Forgejo-only and only the engineer reads it; it does not
/// affect the filesystem topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct RoleBehavior {
    /// Which workflow's fake agent set to register (`--profile`).
    pub profile: ProfileKind,
    /// Architect variant (reference profile only).
    pub architect: ArchitectKind,
    /// Reviewer variant (reference profile only).
    pub reviewer: ReviewerKind,
    /// Forgejo engineer CI-sentinel policy (`--ci-sentinel`).
    pub ci_sentinel: CiSentinelKind,
}

/// Which agent registry a `role` worker populates (`--agents`).
///
/// `fake` uses deterministic behavior-only fakes. The old in-process real-LLM
/// option has moved out of Temper; use Smith's process-responder e2e for real
/// provider coverage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum AgentsKind {
    /// Deterministic fake agents (the default).
    #[default]
    Fake,
}

impl AgentsKind {
    /// Human-readable flag value, for error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentsKind::Fake => "fake",
        }
    }
}

/// Which clock a poll-loop worker drives its ticks from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum ClockKind {
    /// Deterministic [`ManualClock`](temper_runner::ManualClock) seeded near the
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
pub const FORGEJO_TOKEN_ENV: &str = "TEMPER_FORGEJO_TOKEN";

/// Environment variable carrying the web-UI login username for CI reads.
///
/// Optional: only the CI-reading role(s) need it (the Phase 3b password/web-UI
/// CI read path logs in with username + password). Other roles act through the
/// token alone.
pub const FORGEJO_USERNAME_ENV: &str = "TEMPER_FORGEJO_USERNAME";

/// Environment variable carrying the web-UI login password for CI reads.
///
/// Optional, paired with [`FORGEJO_USERNAME_ENV`]; same rationale as the token —
/// passed via env, never argv.
pub const FORGEJO_PASSWORD_ENV: &str = "TEMPER_FORGEJO_PASSWORD";

/// Environment variable carrying the workflow document path (`--workflow`).
///
/// Mirrors the production worker's `TEMPER_WORKFLOW_FILE` (see
/// `temper-worker`'s `WORKFLOW_FILE_ENV`). The flag wins when both are set; when
/// neither is set the worker uses the bundled reference-delivery workflow,
/// reproducing today's default behavior. Unlike the Forgejo credentials this is
/// not a secret, but it is read through the same env seam for parity.
pub const WORKFLOW_FILE_ENV: &str = "TEMPER_WORKFLOW_FILE";

/// Which Forge backend a worker process builds its handle against (`--backend`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum BackendKind {
    /// The local [`FilesystemForge`](temper_forge_filesystem::FilesystemForge)
    /// store shared across processes by path (the default, so the existing
    /// multiprocess test is untouched).
    #[default]
    Filesystem,
    /// A real Forgejo server reached over HTTP via
    /// [`ForgejoForge`](temper_forge_forgejo::ForgejoForge).
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
    /// First configured repository owner, retained for legacy single-repo tests.
    pub owner: String,
    /// First configured repository name, retained for legacy single-repo tests.
    pub name: String,
    /// Repositories this worker scans. Forge permissions, not this list, remain
    /// the write authority; this is only the worker's scan shard.
    pub repositories: Vec<temper_forge::RepositoryPath>,
    /// Poll cadence between ticks.
    pub poll_interval: Duration,
    /// Maximum Forgejo mechanical poll cadence after repeated no-action ticks.
    pub idle_poll_max_interval: Duration,
    /// Low-frequency broad audit cadence. `None` disables audit ticks.
    pub audit_interval: Option<Duration>,
    /// Sentinel file whose existence stops the run loop.
    pub stop_file: Option<PathBuf>,
    /// Maximum wall-clock seconds to run before stopping; `None` runs until the
    /// stop file appears.
    pub run_secs: Option<u64>,
    /// Clock fidelity for poll-loop ticks.
    pub clock: ClockKind,
    /// Which fake agent registry a `role` worker populates.
    pub agents: AgentsKind,
    /// Optional Unix datagram socket that authenticated webhook wakes interrupt.
    pub wake_socket: Option<PathBuf>,
    /// Optional file containing the local wake secret accepted on `wake_socket`.
    pub wake_secret_file: Option<PathBuf>,
    /// Workflow document to operate against (`--workflow` / `TEMPER_WORKFLOW_FILE`).
    /// `None` uses the bundled reference-delivery workflow, reproducing today's
    /// default behavior byte-for-byte.
    pub workflow_file: Option<PathBuf>,
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
pub use super::args_parse::{ParseOutcome, USAGE, parse, parse_with_env};
