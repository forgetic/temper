//! Hermetic contract test for the deterministic daemon test worker.
//!
//! Boots an in-process `temper_engine::Daemon` transport on an ephemeral local
//! port, enqueues an implementation job followed by its enriched
//! `pull_request_writable` CI-repair job, and spawns the real
//! `temper-testing-daemon-worker` binary against a local bare git repository
//! served over `file://` as `--git-base-url` (mirroring smith-worker's
//! bare-origin executor tests). Asserts the first marker-less head is followed
//! by one marker-bearing repair commit on the same branch and both assignments
//! publish `result(success)` with their pushed heads.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use temper_engine::{Daemon, InFlightJob, ResultApplier};
use temper_protocol_worker::{
    Artifact, JobArtifactSnapshot, JobContext, JobResult, PullRequestFreshness, ResultStatus,
    WorkspaceManifest,
};
use temper_testing::daemon_worker::{CI_PASS_MARKER, GIT_TOKEN_ENV, GIT_USER_ENV};
use temper_testing::forgejo_runtime::RunWorkspace;

const RESULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Applier seam that records every applied (job, result) pair for the test.
struct RecordingApplier {
    tx: temper_engine_io::CqSender<(InFlightJob, JobResult)>,
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) -> temper_engine::ApplyOutcome {
        let _ = self.tx.send((job, result));
        temper_engine::ApplyOutcome::Applied
    }
}

/// Kills the worker child on drop so a failing assert cannot leak a process.
struct WorkerGuard {
    child: Child,
    log: PathBuf,
}

impl WorkerGuard {
    fn logs(&self) -> String {
        std::fs::read_to_string(&self.log)
            .unwrap_or_else(|error| format!("<could not read {}: {error}>", self.log.display()))
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn daemon_worker_repairs_ci_failed_pull_request_head() {
    temper_engine_io::block_on_with(move |cx, handle| async move {
        let workspace = RunWorkspace::new("temper-daemon-worker-hermetic");

        // A seeded bare origin reachable over file:// (no credentials needed).
        let git_root = workspace.dir("git/acme");
        let origin = git_root.join("service.git");
        git(&["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, workspace.path());

        // In-process daemon transport with a recording applier seam.
        let (tx, mut rx) = temper_engine_io::channel();
        let daemon =
            Daemon::with_applier(Arc::new(handle.clone()), Arc::new(RecordingApplier { tx }));
        let server = temper_engine::serve(
            &handle,
            &daemon,
            "127.0.0.1:0".parse().expect("loopback addr"),
        )
        .await
        .expect("ephemeral daemon server binds");
        let addr = server.local_addr();

        // One enriched issue job, exactly what the daemon's scan feed enqueues.
        let context = JobContext {
            trace_context: None,
            artifact_context: None,
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            workspace: Some(WorkspaceManifest::single(
                "acme/service",
                "service",
                "main",
                "main",
                "agent/pr-for-code-7",
                "pr-for-code-7",
            )),
            artifact: Some(JobArtifactSnapshot {
                number: 7,
                title: "Implement the thing".to_string(),
                body: "Detailed issue body".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                state: "Open".to_string(),
            }),
            action: Some("open_pr".to_string()),
            checkout_capability: Some("writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: None,
            pull_request_freshness: None,
        };
        daemon
            .enqueue_job(
                "acme/service/issue-7/engineer/code_ready",
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(7),
                    kind: "issue".to_string(),
                },
                serde_json::to_value(&context).expect("JobContext serializes"),
            )
            .await;

        let stop_file = workspace.join("stop");
        let log = workspace.join("worker.log");
        let mut worker = spawn_worker(workspace.path(), addr, &stop_file, &log);

        let (job, result) = skein::time::timeout(
        temper_engine_io::runtime::timer_now(&cx),
        RESULT_TIMEOUT,
        Box::pin(rx.recv()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "daemon did not observe a worker result within {RESULT_TIMEOUT:?}\n--- worker log ---\n{}",
            worker.logs()
        )
    })
    .expect("applier channel stays open");

        assert_eq!(job.job_id, "acme/service/issue-7/engineer/code_ready");
        assert_eq!(job.role, "engineer");
        assert_eq!(
            result.status,
            ResultStatus::Success,
            "worker log:\n{}",
            worker.logs()
        );
        assert_eq!(
            result.repos.len(),
            1,
            "single-repo job produces one repo outcome"
        );
        let branch = result.repos[0].branch.clone();
        assert_eq!(result.repos[0].repo, "acme/service");
        assert_eq!(branch.name, "agent/pr-for-code-7");

        // The branch really exists in the origin at the reported head.
        let pushed_sha = git_output(&[
            "-C",
            path_str(&origin),
            "rev-parse",
            "refs/heads/agent/pr-for-code-7",
        ]);
        assert_eq!(pushed_sha, branch.head_sha);

        // Deterministic implementation commit: deferred CI marker and the native
        // close-on-merge keyword for the source issue.
        let message = git_output(&[
            "-C",
            path_str(&origin),
            "log",
            "-1",
            "--format=%B",
            "refs/heads/agent/pr-for-code-7",
        ]);
        assert!(
            message.starts_with("Implement pr-for-code-7"),
            "unexpected commit subject: {message}"
        );
        assert!(
            !message.contains(CI_PASS_MARKER),
            "deferred implementation head unexpectedly carries the CI marker: {message}"
        );
        assert!(
            message.contains("Closes #7"),
            "commit message is missing the issue close keyword: {message}"
        );
        let author = git_output(&[
            "-C",
            path_str(&origin),
            "log",
            "-1",
            "--format=%an <%ae>|%cn <%ce>",
            "refs/heads/agent/pr-for-code-7",
        ]);
        assert_eq!(
            author,
            "engineer <engineer@example.invalid>|engineer <engineer@example.invalid>"
        );

        // The dedicated CI path assigns the same worker a writable repair of
        // the existing PR head. The fixture worker must push one new marker-
        // bearing commit to that branch instead of opening another branch.
        let repair_context = JobContext {
            trace_context: None,
            artifact_context: None,
            role: "engineer".to_string(),
            repo: "acme/service".to_string(),
            queue: "pr_ci_failed".to_string(),
            artifact_kind: "implementation_pr".to_string(),
            workspace: Some(WorkspaceManifest::single(
                "acme/service",
                "service",
                "main",
                "main",
                branch.name.clone(),
                "pr-for-code-7",
            )),
            artifact: Some(JobArtifactSnapshot {
                number: 8,
                title: "Implementation PR".to_string(),
                body: "Current implementation report".to_string(),
                labels: vec!["implementation".to_string()],
                state: "Open".to_string(),
            }),
            action: Some("address_ci_failure".to_string()),
            checkout_capability: Some("pull_request_writable".to_string()),
            allowed_verdicts: Vec::new(),
            verdict_contracts: Default::default(),
            source_metadata: Default::default(),
            guidance: Some(temper_protocol_worker::JobGuidance {
                role_guidance: Some("Repair the terminal failed CI head.".to_string()),
                ..Default::default()
            }),
            pull_request_freshness: Some(PullRequestFreshness {
                repository_id: "repo-acme-service".to_string(),
                repo: "acme/service".to_string(),
                role: "engineer".to_string(),
                queue: "pr_ci_failed".to_string(),
                action: "address_ci_failure".to_string(),
                number: 8,
                pull_request_id: "pr-8".to_string(),
                head_sha: Some(branch.head_sha.clone()),
                queue_condition: Some("ci_failed".to_string()),
                queue_labels: Vec::new(),
            }),
        };
        daemon
            .enqueue_job(
                "acme/service/pull_request-8/engineer/pr_ci_failed",
                "engineer",
                "acme/service",
                Artifact {
                    item: json!(8),
                    kind: "pull_request".to_string(),
                },
                serde_json::to_value(&repair_context).expect("repair JobContext serializes"),
            )
            .await;

        let (repair_job, repair) = skein::time::timeout(
            temper_engine_io::runtime::timer_now(&cx),
            RESULT_TIMEOUT,
            Box::pin(rx.recv()),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "daemon did not observe the CI repair result within {RESULT_TIMEOUT:?}\n--- worker log ---\n{}",
                worker.logs()
            )
        })
        .expect("applier channel stays open");
        assert_eq!(
            repair_job.job_id,
            "acme/service/pull_request-8/engineer/pr_ci_failed"
        );
        assert_eq!(repair.status, ResultStatus::Success, "{}", worker.logs());
        assert_eq!(repair.repos.len(), 1);
        assert_eq!(repair.repos[0].branch.name, branch.name);
        assert_ne!(repair.repos[0].branch.head_sha, branch.head_sha);
        let repaired_message = git_output(&[
            "-C",
            path_str(&origin),
            "log",
            "-1",
            "--format=%B",
            "refs/heads/agent/pr-for-code-7",
        ]);
        assert!(
            repaired_message.starts_with(&format!("Repair pr-for-code-7 {CI_PASS_MARKER}")),
            "unexpected repair commit: {repaired_message}"
        );

        // The stop-file ends the loop and the worker exits cleanly.
        std::fs::write(&stop_file, b"stop").expect("stop file writes");
        let status = skein::runtime::spawn_blocking(move || worker.child.wait())
            .await
            .expect("worker child waits");
        assert!(status.success(), "worker exited with {status:?}");
    })
}

fn spawn_worker(
    workspace: &Path,
    daemon_addr: std::net::SocketAddr,
    stop_file: &Path,
    log: &Path,
) -> WorkerGuard {
    let log_file = std::fs::File::create(log).expect("worker log creates");
    let child = Command::new(env!("CARGO_BIN_EXE_temper-testing-daemon-worker"))
        .arg("--daemon-url")
        .arg(format!("http://{daemon_addr}"))
        .arg("--worker-id")
        .arg("hermetic-worker-1")
        .arg("--capability")
        .arg("acme/service:engineer")
        .arg("--git-base-url")
        .arg(format!("file://{}/git", path_str(workspace)))
        .arg("--workspace-root")
        .arg(workspace.join("worker-root"))
        .arg("--stop-file")
        .arg(stop_file)
        .arg("--ci-sentinel")
        .arg("deferred")
        .arg("--poll-wait-ms")
        .arg("500")
        .env(GIT_USER_ENV, "engineer")
        .env_remove(GIT_TOKEN_ENV)
        .stdout(Stdio::from(
            log_file.try_clone().expect("worker log clones"),
        ))
        .stderr(Stdio::from(log_file))
        .spawn()
        .expect("daemon test worker spawns");
    WorkerGuard {
        child,
        log: log.to_path_buf(),
    }
}

fn seed_origin(origin: &Path, temp: &Path) {
    let seed = temp.join("seed");
    git(&["init", "-b", "main", path_str(&seed)]);
    std::fs::write(seed.join("README.md"), "# seed\n").expect("seed file writes");
    git(&[
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.invalid",
        "add",
        "README.md",
    ]);
    git(&[
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.invalid",
        "commit",
        "-m",
        "initial commit",
    ]);
    git(&["-C", path_str(&seed), "push", path_str(origin), "main"]);
}

fn git(args: &[&str]) {
    git_output(args);
}

fn git_output(args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .output()
        .expect("git command runs");
    assert!(
        output.status.success(),
        "git {args:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string()
}

fn path_str(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| panic!("non-utf8 path: {path:?}"))
}
