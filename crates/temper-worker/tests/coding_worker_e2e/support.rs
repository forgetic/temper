use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde_json::json;
use temper_worker::config::CapabilitySpec;
use temper_worker::{
    ExecutorSelection, JobExecutor, RoleGitIdentity, WorkerConfig, run_worker_with_transport,
};
use temper_worker_protocol::{Artifact, Assign, WORKER_PROTOCOL_VERSION};

use super::DaemonHarness;

pub fn fake_agent_bin() -> String {
    env!("CARGO_BIN_EXE_temper-fake-agent").to_string()
}

// ---------------------------------------------------------------------------
// Worker config + identities.
// ---------------------------------------------------------------------------

pub fn worker_config() -> WorkerConfig {
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

pub fn role_identities() -> BTreeMap<String, RoleGitIdentity> {
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
pub fn run_until_result<E>(
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
pub fn coding_assign(_fixture: &GitFixture) -> Assign {
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
pub fn coordinated_assign(_fixture: &GitFixture) -> Assign {
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

pub fn multi_repo_worker_config() -> WorkerConfig {
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

pub struct GitFixture {
    temp: tempfile::TempDir,
    git_root: PathBuf,
    pub workspace_root: PathBuf,
}

impl GitFixture {
    pub fn new() -> Self {
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

    pub fn git_base_url(&self) -> String {
        format!("file://{}/git", path_str(self.temp.path()))
    }

    fn origin_of(&self, repo: &str) -> PathBuf {
        self.git_root.join(format!("{repo}.git"))
    }

    pub fn origin_rev(&self, refname: &str) -> String {
        self.rev_of("acme/service", refname)
    }

    pub fn rev_of(&self, repo: &str, refname: &str) -> String {
        git_output(["-C", path_str(&self.origin_of(repo)), "rev-parse", refname])
    }

    pub fn origin_show(&self, spec: &str) -> String {
        self.show_of("acme/service", spec)
    }

    pub fn show_of(&self, repo: &str, spec: &str) -> String {
        git_output(["-C", path_str(&self.origin_of(repo)), "show", spec])
    }

    pub fn origin_log_format(&self, refname: &str, fmt: &str) -> String {
        self.log_format_of("acme/service", refname, fmt)
    }

    pub fn log_format_of(&self, repo: &str, refname: &str, fmt: &str) -> String {
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
