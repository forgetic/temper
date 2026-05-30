//! Library backing the `harness-testing-worker` binary.
//!
//! The binary is a thin entry point; the parsing, worker construction, and run
//! loop live here so the default test suite can exercise the wiring without
//! spawning a process. See `docs/explanation/multiprocess-e2e-roadmap.md`
//! (Phase 3) for the role of this fake worker in the multi-process rehearsal.

pub mod args;
pub mod run;

pub use args::{
    parse, ArchitectKind, ArgsError, CiPolicyKind, ClockKind, ParseOutcome, ReviewerKind,
    RoleBehavior, WorkerArgs, WorkerKind, USAGE,
};
pub use run::{run, RunError};
