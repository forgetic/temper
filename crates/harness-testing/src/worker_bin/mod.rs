//! Library backing the `harness-testing-worker` binary.
//!
//! The binary is a thin entry point; the parsing, worker construction, and run
//! loop live here so the default test suite can exercise the wiring without
//! spawning a process. See `docs/explanation/multiprocess-e2e-roadmap.md`
//! (Phase 3) for the role of this fake worker in the multi-process rehearsal.

pub mod args;
mod args_parse;
pub mod forgejo;
mod forgejo_engineer;
pub mod run;

pub use args::{
    parse, parse_with_env, ArchitectKind, ArgsError, Backend, BackendKind, CiPolicyKind,
    CiSentinelKind, ClockKind, ForgejoArgs, ParseOutcome, ReviewerKind, RoleBehavior, WorkerArgs,
    WorkerKind, FORGEJO_PASSWORD_ENV, FORGEJO_TOKEN_ENV, FORGEJO_USERNAME_ENV, USAGE,
};
pub use run::{run, RunError};
