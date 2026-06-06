//! Library backing the `temper-testing-worker` binary.
//!
//! The binary is a thin entry point; the parsing, worker construction, and run
//! loop live here so the default test suite can exercise the wiring without
//! spawning a process. See `docs/how-to/run-multiprocess-e2e.md` for the role of
//! this fake worker in the multi-process rehearsal.

pub mod args;
mod args_parse;
pub mod forgejo;
mod forgejo_drive;
mod forgejo_engineer;
mod multi_ci;
pub mod run;

pub use args::{
    parse, parse_with_env, AgentsKind, ArchitectKind, ArgsError, Backend, BackendKind,
    CiPolicyKind, CiSentinelKind, ClockKind, ForgejoArgs, ParseOutcome, ReviewerKind, RoleBehavior,
    WorkerArgs, WorkerKind, FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV, USAGE,
    WORKFLOW_FILE_ENV,
};
pub use run::{run, RunError};
