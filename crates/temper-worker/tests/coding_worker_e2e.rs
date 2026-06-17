//! Hermetic full-worker end-to-end test of the **out-of-process agent boundary**.
//!
//! A real `smith-worker` runs a real coding job by spawning a real agent
//! **process** (`smith-fake-agent`, a deterministic protocol speaker — no LLM,
//! no git), with NO external Forgejo or runner. This exercises the orchestration
//! path and the `smith-agent-protocol` boundary; the agent's own LLM loop is
//! tested in the `anvil` repo against jig.
//!
//! Topology, all in one process:
//! - the **real daemon**, reached through its in-process carrier, speaks the
//!   worker/daemon wire protocol: accepts register, assigns one real coding job
//!   (a full `WireJobContext` payload), then accepts the result;
//! - a **real `smith-worker`** runs on its own skein runtime thread with an
//!   [`OutOfProcessRunner`] pointed at the `smith-fake-agent` binary;
//! - a **recording [`ProgressSink`]** captures every step-progress marker the
//!   worker relayed from the agent's stdout;
//! - git remotes are local `file://` bare repos seeded with an initial commit.
//!
//! This drives the entire production path: worker register → poll → assign →
//! `CodingExecutor` prepares the checkout → spawns `smith-fake-agent` over the
//! protocol → the agent writes the product file + emits step-progress → the
//! worker relays progress, commits and pushes the branch to the `file://` origin
//! → reports Success. Assertions verify the branch landed with the agent's file,
//! the worker reported Success, and the step-progress checkpoints were relayed.
//!
//! A second test injects an agent crash *after* it emits progress but *before* it
//! writes the result — the crash-recovery scenario: the worker reports a
//! transient failure (re-dispatchable) and the already-emitted progress markers
//! were still relayed.
//!
//! Hermetic and fast; runs by default.

use std::sync::{Arc, Mutex};

use temper_protocol_agent::{StepProgress, StepState};
use temper_protocol_worker::ResultStatus;
use temper_worker::{CodingExecutor, CodingExecutorConfig, OutOfProcessRunner, ProgressSink};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

#[path = "coding_worker_e2e/support.rs"]
mod support;
use support::*;

/// Records every step-progress marker the worker relays, so the test can assert
/// the agent→worker→sink path fired.
#[derive(Clone, Default)]
struct RecordingProgressSink {
    markers: Arc<Mutex<Vec<StepProgress>>>,
}

impl ProgressSink for RecordingProgressSink {
    fn report(&self, progress: StepProgress) {
        self.markers.lock().expect("markers lock").push(progress);
    }
}

impl RecordingProgressSink {
    fn snapshot(&self) -> Vec<StepProgress> {
        self.markers.lock().expect("markers lock").clone()
    }
}

#[test]
fn worker_runs_a_real_coding_job_through_the_out_of_process_agent() {
    let fixture = GitFixture::new();

    // The out-of-process runner spawns the deterministic fake agent binary,
    // which writes GREETING.md and emits two step-progress markers.
    let runner = Arc::new(OutOfProcessRunner::new(vec![fake_agent_bin()]));
    let executor_config = CodingExecutorConfig {
        workspace_root: fixture.workspace_root.clone(),
        git_base_url: fixture.git_base_url(),
        role_identities: role_identities(),
    };
    let sink = RecordingProgressSink::default();
    let executor = Arc::new(
        CodingExecutor::new(executor_config, runner).with_progress_sink(Arc::new(sink.clone())),
    );

    let result = run_until_result(coding_assign(&fixture), worker_config(), executor);

    // The worker reported Success with a branch.
    assert_eq!(result.status, ResultStatus::Success, "result: {result:?}");
    assert_eq!(
        result.repos.len(),
        1,
        "coding job produces one per-repo outcome"
    );
    let branch = &result.repos[0].branch;
    assert_eq!(branch.name, "agent/pr-for-code-7");
    assert_eq!(branch.head_sha.len(), 40, "head sha looks like a real sha");

    // The branch landed on origin with the agent's product file.
    let pushed_sha = fixture.origin_rev("refs/heads/agent/pr-for-code-7");
    assert_eq!(
        pushed_sha, branch.head_sha,
        "the reported sha is what was pushed"
    );
    let greeting = fixture.origin_show("refs/heads/agent/pr-for-code-7:GREETING.md");
    assert_eq!(
        greeting, "hello from the fake agent",
        "the agent's product file was committed and pushed"
    );
    // The commit message carries the correlation key + Closes trailer.
    assert_eq!(
        fixture.origin_log_format("refs/heads/agent/pr-for-code-7", "%s"),
        "Implement pr-for-code-7"
    );
    assert_eq!(
        fixture.origin_log_format("refs/heads/agent/pr-for-code-7", "%b"),
        "Closes #7"
    );

    // The worker relayed the agent's step-progress checkpoints (the crash-recovery
    // channel): a Started marker and a Done marker, both stamped with the job's
    // correlation key.
    let markers = sink.snapshot();
    assert_eq!(
        markers.len(),
        2,
        "expected two step-progress markers: {markers:?}"
    );
    assert!(markers.iter().all(|m| m.correlation_key == "pr-for-code-7"));
    assert_eq!(markers[0].state, StepState::Started);
    assert_eq!(markers[1].state, StepState::Done);
}

/// The headline ADR 0023 demonstration: one engineer assignment assembles a
/// two-repo workspace, the agent edits both, and the worker pushes a branch to
/// *each* repo — reporting one `RepoOutcome` per repo for the daemon to turn
/// into one PR each.
#[test]
fn worker_runs_a_coordinated_multi_repo_job_and_pushes_each_writable_repo() {
    let fixture = GitFixture::new();

    let runner = Arc::new(OutOfProcessRunner::new(vec![fake_agent_bin()]));
    let executor_config = CodingExecutorConfig {
        workspace_root: fixture.workspace_root.clone(),
        git_base_url: fixture.git_base_url(),
        role_identities: role_identities(),
    };
    let sink = RecordingProgressSink::default();
    let executor = Arc::new(
        CodingExecutor::new(executor_config, runner).with_progress_sink(Arc::new(sink.clone())),
    );

    let result = run_until_result(
        coordinated_assign(&fixture),
        multi_repo_worker_config(),
        executor,
    );

    assert_eq!(result.status, ResultStatus::Success, "result: {result:?}");

    // Two writable repos that each produced a diff → one outcome per repo.
    let mut reported: Vec<String> = result
        .repos
        .iter()
        .map(|outcome| outcome.repo.clone())
        .collect();
    reported.sort();
    assert_eq!(
        reported,
        vec!["acme/lib".to_string(), "acme/service".to_string()],
        "coordinated job reports one outcome per writable repo: {:?}",
        result.repos
    );

    // Each repo's shared coordination branch landed with the agent's product
    // file at exactly the reported head sha.
    let branch = "agent/coord-for-code-7";
    for repo in ["acme/service", "acme/lib"] {
        let greeting = fixture.show_of(repo, &format!("refs/heads/{branch}:GREETING.md"));
        assert_eq!(
            greeting, "hello from the fake agent",
            "{repo} branch carries the agent's product file"
        );
        let outcome = result
            .repos
            .iter()
            .find(|outcome| outcome.repo == repo)
            .expect("an outcome for each repo");
        assert_eq!(
            fixture.rev_of(repo, &format!("refs/heads/{branch}")),
            outcome.branch.head_sha,
            "the reported sha for {repo} is what was pushed"
        );
    }

    // Only the primary repo's commit closes the coordinating issue (cross-repo
    // close-on-merge does not exist); the secondary omits the trailer.
    assert_eq!(
        fixture.log_format_of("acme/service", &format!("refs/heads/{branch}"), "%b"),
        "Closes #7"
    );
    assert_eq!(
        fixture.log_format_of("acme/lib", &format!("refs/heads/{branch}"), "%b"),
        ""
    );
}

#[test]
fn worker_reports_transient_failure_when_agent_crashes_after_progress() {
    let fixture = GitFixture::new();

    // The fake agent emits progress, then exits non-zero before writing a result
    // — a crash mid-task. (A future slice has the agent push its partial work
    // first so the next agent resumes; here we assert the worker's handling: the
    // emitted markers were relayed, and the job is a re-dispatchable transient.)
    // The crash knob is a command arg, not an env var, so concurrent test
    // threads cannot race on it.
    let runner = Arc::new(OutOfProcessRunner::new(vec![
        fake_agent_bin(),
        "--crash-after-progress".to_string(),
    ]));
    let executor_config = CodingExecutorConfig {
        workspace_root: fixture.workspace_root.clone(),
        git_base_url: fixture.git_base_url(),
        role_identities: role_identities(),
    };
    let sink = RecordingProgressSink::default();
    let executor = Arc::new(
        CodingExecutor::new(executor_config, runner).with_progress_sink(Arc::new(sink.clone())),
    );

    let result = run_until_result(coding_assign(&fixture), worker_config(), executor);

    // The worker reported a transient failure (the crash is re-dispatchable).
    assert_eq!(result.status, ResultStatus::Failure, "result: {result:?}");
    let failure = result.failure.expect("failure carries detail");
    assert_eq!(
        failure.class,
        temper_protocol_worker::FailureClass::Transient
    );

    // The progress the agent emitted *before* crashing was still relayed — the
    // recovery channel survives the crash.
    let markers = sink.snapshot();
    assert!(
        markers.iter().any(|m| m.state == StepState::Started),
        "the pre-crash Started marker must have been relayed: {markers:?}"
    );
}

// ---------------------------------------------------------------------------
// The fake agent binary path (built by cargo as a [[bin]] of this crate).
// ---------------------------------------------------------------------------
