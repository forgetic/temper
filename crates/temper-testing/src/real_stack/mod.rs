//! Hermetic real-stack fixture builders for integration tests.
//!
//! This module composes production Temper components without a live Forgejo or
//! host Actions runner: a real [`temper_engine::Daemon`] with the Forge-backed
//! result applier, a real [`temper_worker::WorkerMachine`] /
//! [`temper_worker::WorkerShell`] loop reached through
//! [`ResultTappingTransport`], the real
//! [`temper_worker::CodingExecutor`], and the native coding agent pointed at a
//! Jig fake LLM. Product repositories are local `file://` bare git remotes and
//! Forge state lives in [`temper_forge_memory::MemoryForge`].
//!
//! The API intentionally stays narrow: tests describe one primary repo, one
//! ready issue, one or more worker role identities, and the fake model's edit
//! plan; the fixture handles daemon/worker/agent wiring and exposes the
//! resulting worker result, pull requests, Forge handle, and pushed git branches
//! for assertions.
//!
//! ```no_run
//! use temper_testing::real_stack::{
//!     FakeModelResponse, HermeticIssueSpec, HermeticRealStackBuilder,
//!     HermeticRepoSpec,
//! };
//! use temper_protocol_worker::ResultStatus;
//!
//! temper_engine_io::block_on_with(|cx, handle| async move {
//!     let mut stack = HermeticRealStackBuilder::new()
//!         .repo(HermeticRepoSpec::new("acme", "service"))
//!         .issue(HermeticIssueSpec::ready_code(
//!             "Create notes",
//!             "Add NOTES.md with deterministic content.",
//!         ))
//!         .fake_model_response(FakeModelResponse::write_file(
//!             "service/NOTES.md",
//!             "notes\n",
//!             "Added NOTES.md.",
//!         ))
//!         .build(&handle)
//!         .await
//!         .expect("fixture builds");
//!
//!     let run = stack
//!         .run_open_pr_job(&cx, &handle)
//!         .await
//!         .expect("worker runs one coding job");
//!     assert_eq!(run.job_result.status, ResultStatus::Success);
//! });
//! ```

mod acceptance;
mod artifact_context;
mod builder;
mod clock;
mod git;
mod pause;
mod runner;
mod stack;
mod types;

pub use jig_core::{Reply, Script, StopReason, Turn};

pub use builder::HermeticRealStackBuilder;
pub use clock::MutableWallClock;
pub use pause::{PauseHooks, PausePermit, PausePoint, ReachedPause};
pub use runner::NativeJigAgentRunner;
pub use stack::{
    HermeticComponentHandles, HermeticDurableWorld, HermeticRealStack, HermeticRunResult,
    ResultTappingTransport,
};
pub use types::{
    FakeModelResponse, FakeModelWrite, HermeticIssueSpec, HermeticRepoSpec, WorkerRoleSpec,
};

pub(crate) const DEFAULT_NOW: &str = "2026-05-29T00:00:00Z";
pub(crate) const DEFAULT_WORKER_ID: &str = "hermetic-real-stack-worker";
pub(crate) const DEFAULT_MAX_ITERATIONS: usize = 6;
