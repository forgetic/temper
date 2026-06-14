//! Hermetic contract test for the deterministic daemon test worker.
//!
//! Boots an in-process `temper_engine::Daemon` transport on an ephemeral local
//! port, enqueues one enriched issue job, and spawns the real
//! `temper-testing-daemon-worker` binary against it with a local bare git
//! repository served over `file://` as `--git-base-url` (mirroring
//! smith-worker's bare-origin executor tests). Asserts the worker registers,
//! receives the assignment, pushes the hinted branch with the expected
//! deterministic commit, and that the daemon's applier seam observes
//! `result(success)` carrying the pushed head.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use temper_engine::{Daemon, InFlightJob, ResultApplier};
use temper_testing::daemon_worker::{CI_PASS_MARKER, GIT_TOKEN_ENV, GIT_USER_ENV};
use temper_testing::forgejo_runtime::RunWorkspace;
use temper_worker_protocol::{
    Artifact, JobArtifactSnapshot, JobContext, JobResult, ResultStatus, WorkspaceManifest,
};

const RESULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Applier seam that records every applied (job, result) pair for the test.
struct RecordingApplier {
    tx: temper_io_engine::CqSender<(InFlightJob, JobResult)>,
}

#[async_trait::async_trait]
impl ResultApplier for RecordingApplier {
    async fn apply(&self, job: InFlightJob, result: JobResult) {
        let _ = self.tx.send((job, result));
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
fn daemon_worker_pushes_branch_and_daemon_sees_success() {
    temper_io_engine::block_on_with(move |cx, handle| async move {
        let workspace = RunWorkspace::new("temper-daemon-worker-hermetic");

        // A seeded bare origin reachable over file:// (no credentials needed).
        let git_root = workspace.dir("git/acme");
        let origin = git_root.join("service.git");
        git(&["init", "--bare", path_str(&origin)]);
        seed_origin(&origin, workspace.path());

        // In-process daemon transport with a recording applier seam.
        let (tx, mut rx) = temper_io_engine::channel();
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
        temper_io_engine::runtime::timer_now(&cx),
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

        // Deterministic commit: role identity, CI sentinel, and the native
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
            message.starts_with(&format!("Implement pr-for-code-7 {CI_PASS_MARKER}")),
            "unexpected commit subject: {message}"
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
