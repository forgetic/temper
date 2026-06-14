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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::json;
use temper_agent_protocol::{StepProgress, StepState};
use temper_worker::config::CapabilitySpec;
use temper_worker::{
    CodingExecutor, CodingExecutorConfig, ExecutorSelection, JobExecutor, OutOfProcessRunner,
    ProgressSink, RoleGitIdentity, WorkerConfig, run_worker_with_transport,
};
use temper_worker_protocol::{Artifact, Assign, ResultStatus, WORKER_PROTOCOL_VERSION};

#[path = "support/real_daemon.rs"]
mod real_daemon;
use real_daemon::DaemonHarness;

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
        temper_worker_protocol::FailureClass::Transient
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

fn fake_agent_bin() -> String {
    env!("CARGO_BIN_EXE_temper-fake-agent").to_string()
}

// ---------------------------------------------------------------------------
// Worker config + identities.
// ---------------------------------------------------------------------------

fn worker_config() -> WorkerConfig {
    WorkerConfig {
        daemon_url: "http://placeholder".to_string(),
        worker_id: "coding-worker-e2e".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "acme/service".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: 1,
        poll_wait: std::time::Duration::from_millis(50),
        heartbeat_interval: std::time::Duration::from_millis(50),
        // `run_worker` takes the executor we construct directly, so the config's
        // `executor` field is unused here (it only matters to the binary's arg
        // parsing); leave it as the stub shape.
        executor: ExecutorSelection::Stub,
    }
}

fn role_identities() -> BTreeMap<String, RoleGitIdentity> {
    let mut m = BTreeMap::new();
    m.insert(
        "engineer".to_string(),
        RoleGitIdentity {
            user: "Smith Engineer".to_string(),
            email: "engineer@example.test".to_string(),
            token: "test-token".to_string(),
        },
    );
    m
}

/// Run one coding job end-to-end against the **real** daemon: enqueue `assign`,
/// run a real worker (with the given `executor`) on the same runtime through the
/// daemon's in-process carrier, and return the result the daemon applied. The
/// worker is spawned detached; the call returns once the result is applied.
fn run_until_result<E>(
    assign: Assign,
    config: WorkerConfig,
    executor: Arc<E>,
) -> temper_worker_protocol::JobResult
where
    E: JobExecutor + Send + Sync + 'static,
{
    temper_engine_io::block_on_with(move |_cx, handle| async move {
        let mut harness = DaemonHarness::start(&handle);
        harness.enqueue(&assign).await;

        let transport = harness.transport();
        let worker_handle = handle.clone();
        handle.spawn(async move {
            let _ = run_worker_with_transport(worker_handle, config, executor, transport).await;
        });

        harness.await_result().await
    })
}

/// A full coding-job Assign payload (the enriched v2 `JobContext` the executor
/// requires: per-repo branch data lives in the workspace manifest).
fn coding_assign(_fixture: &GitFixture) -> Assign {
    let job_context = json!({
        "role": "engineer",
        "repo": "acme/service",
        "queue": "code_ready",
        "artifact_kind": "code",
        "artifact": {
            "number": 7,
            "title": "Add a greeting file",
            "body": "Create GREETING.md.",
            "labels": ["code", "ready"],
            "state": "Open"
        },
        "action": "open_pr",
        "checkout_capability": "writable",
        "allowed_verdicts": [],
        "workspace": {
            "coordination_key": "pr-for-code-7",
            "repos": [{
                "repo": "acme/service",
                "dir": "service",
                "access": "writable",
                "default_branch": "main",
                "base_branch": "main",
                "branch_hint": "agent/pr-for-code-7"
            }]
        }
    });
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: "acme/service/issue-7/engineer/pr-for-code-7".to_string(),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: job_context,
    }
}

/// A coordinated multi-repo coding-job Assign: a two-repo writable workspace
/// manifest (ADR 0023), both on the shared `coord-for-code-7` branch.
fn coordinated_assign(_fixture: &GitFixture) -> Assign {
    let job_context = json!({
        "role": "engineer",
        "repo": "acme/service",
        "queue": "code_ready",
        "artifact_kind": "code",
        "artifact": {
            "number": 7,
            "title": "Cross-repo greeting",
            "body": "Add GREETING.md to both repos.",
            "labels": ["code", "ready", "coordinated"],
            "state": "Open"
        },
        "workspace": {
            "coordination_key": "coord-for-code-7",
            "repos": [
                {
                    "repo": "acme/service",
                    "dir": "service",
                    "access": "writable",
                    "default_branch": "main",
                    "base_branch": "main",
                    "branch_hint": "agent/coord-for-code-7"
                },
                {
                    "repo": "acme/lib",
                    "dir": "lib",
                    "access": "writable",
                    "default_branch": "main",
                    "base_branch": "main",
                    "branch_hint": "agent/coord-for-code-7"
                }
            ]
        },
        "action": "open_pr",
        "checkout_capability": "writable",
        "allowed_verdicts": []
    });
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: "acme/service/issue-7/engineer/coord-for-code-7".to_string(),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: job_context,
    }
}

fn multi_repo_worker_config() -> WorkerConfig {
    WorkerConfig {
        capabilities: vec![
            CapabilitySpec {
                repo: "acme/service".to_string(),
                role: "engineer".to_string(),
            },
            CapabilitySpec {
                repo: "acme/lib".to_string(),
                role: "engineer".to_string(),
            },
        ],
        ..worker_config()
    }
}

// ---------------------------------------------------------------------------
// Git fixture (bare origin seeded with main, file:// remotes).
// ---------------------------------------------------------------------------

struct GitFixture {
    temp: tempfile::TempDir,
    git_root: PathBuf,
    workspace_root: PathBuf,
}

impl GitFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp dir");
        let git_root = temp.path().join("git");
        fs::create_dir_all(git_root.join("acme")).expect("git root");
        // Two seeded bare origins so a coordinated job can push a branch to each.
        for repo in ["acme/service", "acme/lib"] {
            let origin = git_root.join(format!("{repo}.git"));
            git(["init", "--bare", path_str(&origin)]);
            seed_origin(
                &origin,
                &temp.path().join(format!("seed-{}", repo.replace('/', "-"))),
            );
        }
        let workspace_root = temp.path().join("workspaces");
        Self {
            temp,
            git_root,
            workspace_root,
        }
    }

    fn git_base_url(&self) -> String {
        format!("file://{}/git", path_str(self.temp.path()))
    }

    fn origin_of(&self, repo: &str) -> PathBuf {
        self.git_root.join(format!("{repo}.git"))
    }

    fn origin_rev(&self, refname: &str) -> String {
        self.rev_of("acme/service", refname)
    }

    fn rev_of(&self, repo: &str, refname: &str) -> String {
        git_output(["-C", path_str(&self.origin_of(repo)), "rev-parse", refname])
    }

    fn origin_show(&self, spec: &str) -> String {
        self.show_of("acme/service", spec)
    }

    fn show_of(&self, repo: &str, spec: &str) -> String {
        git_output(["-C", path_str(&self.origin_of(repo)), "show", spec])
    }

    fn origin_log_format(&self, refname: &str, fmt: &str) -> String {
        self.log_format_of("acme/service", refname, fmt)
    }

    fn log_format_of(&self, repo: &str, refname: &str, fmt: &str) -> String {
        git_output([
            "-C",
            path_str(&self.origin_of(repo)),
            "log",
            "-1",
            &format!("--format={fmt}"),
            refname,
        ])
    }
}

fn seed_origin(origin: &Path, temp: &Path) {
    let seed = temp.join("seed");
    git(["init", "-b", "main", path_str(&seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("seed file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial",
    ]);
    git([
        "-C",
        path_str(&seed),
        "remote",
        "add",
        "origin",
        path_str(origin),
    ]);
    git(["-C", path_str(&seed), "push", "origin", "main"]);
}

fn git<const N: usize>(args: [&str; N]) {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_output<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new("git").args(args).output().expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim_end_matches('\n')
        .to_string()
}

fn path_str(p: &Path) -> &str {
    p.as_os_str().to_str().expect("utf8 path")
}
