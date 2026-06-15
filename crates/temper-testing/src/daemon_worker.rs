//! Minimal wire-protocol test worker that stands in for `smith-worker`.
//!
//! The daemon-topology e2e tests need a worker-tier process that speaks the
//! Worker/Daemon Wire Protocol v1 with full fidelity (`register` -> `poll` ->
//! `assign` -> `result`) and performs a *real* git push as the assigned role's
//! git identity - without dragging `temper-runner`, the fake agents, or any
//! Forge client into the worker tier. This module is that stand-in: a
//! deterministic, protocol-faithful coding worker whose "implementation" is one
//! deterministic change file per job.
//!
//! Protocol discipline matches production `smith-worker`:
//!
//! - The worker never calls the Forge API. Its only outputs are the pushed
//!   branch and the structured `result` message; the daemon owns PR creation.
//! - Issue-targeted jobs with the enriched standard payload succeed; payloads
//!   missing the enrichment (or non-issue artifacts, which the daemon's
//!   `pr_ci_failed` feed can produce) fail with `protocol` class, mirroring
//!   `smith-worker`'s `CodingExecutor`.
//! - Git failures are reported as `transient` failures so the daemon can
//!   re-enqueue.
//!
//! Git identity and token come from the environment ([`GIT_USER_ENV`] /
//! [`GIT_TOKEN_ENV`]), never argv, and the token is redacted from every error
//! message. The token is optional so hermetic tests can push over `file://`
//! remotes without any credential.
//!
//! The commit the worker pushes carries two load-bearing message fragments:
//!
//! - With `--ci-sentinel present` the subject line includes
//!   [`CI_PASS_MARKER`], so the provisioned e2e CI workflow (which gates on the
//!   head commit's message) passes immediately. With `deferred` the marker is
//!   omitted and CI fails until something else (the red->green e2e variant)
//!   pushes a marker-bearing commit.
//! - A `Closes #<issue>` trailer referencing the source issue, so merging the
//!   implementation PR closes the issue through the real provider's native
//!   close-on-merge keyword handling - the daemon topology has no role that
//!   closes source issues, and the e2e asserts this real wiring.

mod args;
mod job;
mod protocol;

pub use args::{CiSentinel, DaemonWorkerConfig, ParseOutcome, USAGE, parse_args};
pub use protocol::run;

pub use crate::forgejo_server::provision::CI_PASS_MARKER;

/// Environment variable carrying the git author/committer login.
pub const GIT_USER_ENV: &str = "TEMPER_E2E_GIT_USER";
/// Environment variable carrying the git push token (optional).
pub const GIT_TOKEN_ENV: &str = "TEMPER_E2E_GIT_TOKEN";

/// Git author/committer identity plus optional push token, read from env.
#[derive(Clone)]
pub struct GitIdentity {
    pub user: String,
    pub email: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for GitIdentity {
    /// Redacts the token so a `{:?}` can never leak it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitIdentity")
            .field("user", &self.user)
            .field("email", &self.email)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl GitIdentity {
    /// Reads the identity from [`GIT_USER_ENV`] / [`GIT_TOKEN_ENV`]. The email
    /// is derived the same way the e2e fixture provisions role users.
    pub fn from_env() -> Result<Self, String> {
        let user = std::env::var(GIT_USER_ENV)
            .ok()
            .map(|user| user.trim().to_string())
            .filter(|user| !user.is_empty())
            .ok_or_else(|| format!("missing required environment variable {GIT_USER_ENV}"))?;
        let token = std::env::var(GIT_TOKEN_ENV)
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        Ok(Self {
            email: format!("{user}@example.invalid"),
            user,
            token,
        })
    }
}
