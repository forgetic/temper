//! End-to-end tests for [`CodingExecutor`]: workspace prep, the agent turn (an
//! in-process [`FakeAgentRunner`] standing in for the out-of-process agent), and
//! the result -> [`JobOutcome`] mapping (commit/push on the writable head path,
//! verdict routing otherwise).
//!
//! These exercise the executor's workspace/git/result-mapping logic, which is
//! independent of *how* the agent turn is produced. The real out-of-process
//! boundary is covered separately by `coding_worker_e2e.rs`.

#[path = "coding_executor/context.rs"]
mod context;
#[path = "coding_executor/failures.rs"]
mod failures;
#[path = "coding_executor/pr_repair.rs"]
mod pr_repair;
#[path = "coding_executor/read_only.rs"]
mod read_only;
#[path = "coding_executor/review.rs"]
mod review;
#[path = "coding_executor/support/mod.rs"]
mod support;
#[path = "coding_executor/target_branch.rs"]
mod target_branch;
#[path = "coding_executor/writable.rs"]
mod writable;
