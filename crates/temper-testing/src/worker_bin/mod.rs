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
    AgentsKind, ArchitectKind, ArgsError, Backend, BackendKind, CiPolicyKind, CiSentinelKind,
    ClockKind, FORGEJO_TOKEN_ENV, ForgejoArgs, ParseOutcome, ProfileKind, ReviewerKind,
    RoleBehavior, USAGE, WORKFLOW_FILE_ENV, WorkerArgs, WorkerKind, parse, parse_with_env,
};
pub use run::{RunError, run};
