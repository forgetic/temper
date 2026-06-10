//! Library backing the `temper-testing-worker` binary.
//!
//! The binary is a thin entry point; the parsing, worker construction, and run
//! loop live here so the default test suite can exercise the wiring without
//! spawning a process. This fake worker backs the legacy fleet runtime that
//! stays deployed until the daemon-topology cutover (the multi-process e2e
//! rehearsals that drove it were replaced by `tests/daemon_forgejo_e2e.rs`).

pub mod args;
mod args_parse;
pub mod forgejo;
mod forgejo_drive;
mod forgejo_engineer;
mod multi_ci;
pub mod run;

pub use args::{
    parse, parse_with_env, AgentsKind, ArchitectKind, ArgsError, Backend, BackendKind,
    CiPolicyKind, CiSentinelKind, ClockKind, ForgejoArgs, ParseOutcome, ProfileKind, ReviewerKind,
    RoleBehavior, WorkerArgs, WorkerKind, FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV,
    FORGEJO_USERNAME_ENV, USAGE, WORKFLOW_FILE_ENV,
};
pub use run::{run, RunError};
