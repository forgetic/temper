use std::collections::BTreeMap;

use super::*;

pub struct Fixture {
    temp: TempDir,
    pub origin: PathBuf,
    pub workspace_root: PathBuf,
    pub pull_request_head_sha: String,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().expect("create temp dir");
        let git_root = temp.path().join("git");
        fs::create_dir_all(git_root.join("acme")).expect("create git root");
        let origin = git_root.join("acme/service.git");
        git(["init", "--bare", path_str(&origin)]);
        let pull_request_head_sha = seed_origin(&origin, temp.path());
        git([
            "-C",
            path_str(&origin),
            "symbolic-ref",
            "HEAD",
            "refs/heads/main",
        ]);

        Self {
            workspace_root: temp.path().join("workspaces"),
            temp,
            origin,
            pull_request_head_sha,
        }
    }

    pub fn executor<R: AgentRunner + 'static>(
        &self,
        runner: R,
        include_identity: bool,
    ) -> CodingExecutor<R> {
        let mut role_identities = BTreeMap::new();
        if include_identity {
            for (role, user, email) in [
                ("engineer", "Smith Engineer", "smith-engineer@example.test"),
                (
                    "architect",
                    "Smith Architect",
                    "smith-architect@example.test",
                ),
                ("reviewer", "Smith Reviewer", "smith-reviewer@example.test"),
                ("tester", "Smith Tester", "smith-tester@example.test"),
            ] {
                role_identities.insert(
                    role.to_string(),
                    RoleGitIdentity {
                        user: user.to_string(),
                        email: email.to_string(),
                        token: "test-token".to_string(),
                    },
                );
            }
        }

        CodingExecutor::new(
            CodingExecutorConfig {
                workspace_root: self.workspace_root.clone(),
                git_base_url: format!("file://{}/git", path_str(self.temp.path())),
                role_identities,
            },
            Arc::new(runner),
        )
    }

    pub fn seed_pr_head_branch(&self, branch: &str) -> String {
        git([
            "-C",
            path_str(&self.origin),
            "update-ref",
            &format!("refs/heads/{branch}"),
            self.pull_request_head_sha.as_str(),
        ]);
        self.pull_request_head_sha.clone()
    }

    pub fn seed_conflicting_pr_head_branch(&self, branch: &str) -> (String, String) {
        let seed = self.temp.path().join("conflict-seed");
        git(["clone", path_str(&self.origin), path_str(&seed)]);
        git(["-C", path_str(&seed), "checkout", "main"]);
        fs::write(seed.join("conflict.txt"), "shared base\n").expect("write base conflict file");
        git([
            "-C",
            path_str(&seed),
            "-c",
            "user.name=Seed User",
            "-c",
            "user.email=seed@example.test",
            "add",
            "conflict.txt",
        ]);
        git([
            "-C",
            path_str(&seed),
            "-c",
            "user.name=Seed User",
            "-c",
            "user.email=seed@example.test",
            "commit",
            "-m",
            "add conflict base",
        ]);
        git(["-C", path_str(&seed), "push", "origin", "main"]);

        git(["-C", path_str(&seed), "checkout", "-b", branch]);
        fs::write(seed.join("conflict.txt"), "pull request side\n")
            .expect("write PR side conflict file");
        git([
            "-C",
            path_str(&seed),
            "-c",
            "user.name=Seed User",
            "-c",
            "user.email=seed@example.test",
            "commit",
            "-am",
            "edit conflict file on PR head",
        ]);
        let pull_request_head = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
        git([
            "-C",
            path_str(&seed),
            "push",
            "origin",
            &format!("HEAD:refs/heads/{branch}"),
        ]);

        git(["-C", path_str(&seed), "checkout", "main"]);
        fs::write(seed.join("conflict.txt"), "main side\n").expect("write main side conflict file");
        git([
            "-C",
            path_str(&seed),
            "-c",
            "user.name=Seed User",
            "-c",
            "user.email=seed@example.test",
            "commit",
            "-am",
            "edit conflict file on main",
        ]);
        let main_head = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
        git(["-C", path_str(&seed), "push", "origin", "main"]);

        (pull_request_head, main_head)
    }
}

pub fn assign(branch_hint: &str, correlation_key: &str) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: format!("acme/service/issue-7/engineer/{correlation_key}"),
        attempt_id: Some(format!("attempt-{correlation_key}")),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: job_context(branch_hint, correlation_key).to_payload(),
    }
}

pub fn pr_assign(
    branch_hint: &str,
    correlation_key: &str,
    context_builder: fn(&str, &str) -> TestJobContext,
) -> Assign {
    let context = context_builder(branch_hint, correlation_key);
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: format!("acme/service/pull-7/reviewer/{correlation_key}"),
        attempt_id: Some(format!("attempt-{correlation_key}")),
        role: "reviewer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "pull_request".to_string(),
        },
        job_payload: context.to_payload(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestJobContext {
    role: String,
    repo: String,
    queue: String,
    artifact_kind: String,
    default_branch: String,
    base_branch: String,
    branch_hint: String,
    correlation_key: String,
    access: String,
    pub artifact: Option<TestJobArtifactSnapshot>,
    pub action: Option<String>,
    checkout_capability: Option<String>,
    pub allowed_verdicts: Vec<String>,
    pub verdict_contracts: temper_verdict::VerdictContracts,
    pub source_metadata: temper_verdict::SourceMetadata,
    pub pull_request_freshness: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TestJobArtifactSnapshot {
    number: u64,
    title: String,
    body: String,
    labels: Vec<String>,
    state: String,
}

impl TestJobContext {
    pub fn with_base_branch(mut self, base_branch: &str) -> Self {
        self.base_branch = base_branch.to_string();
        self
    }

    /// Serialize into the v2 `JobContext` wire shape (the workspace manifest
    /// carries the per-repo branch data; this is exactly what the daemon emits
    /// and the coding executor parses).
    pub fn to_payload(&self) -> serde_json::Value {
        json!({
            "role": self.role,
            "repo": self.repo,
            "queue": self.queue,
            "artifact_kind": self.artifact_kind,
            "artifact": self.artifact,
            "action": self.action,
            "checkout_capability": self.checkout_capability,
            "allowed_verdicts": self.allowed_verdicts,
            "verdict_contracts": self.verdict_contracts,
            "source_metadata": self.source_metadata,
            "pull_request_freshness": self.pull_request_freshness,
            "workspace": {
                "coordination_key": self.correlation_key,
                "repos": [{
                    "repo": self.repo,
                    "dir": self.repo.split('/').next_back().unwrap_or(&self.repo),
                    "access": self.access,
                    "default_branch": self.default_branch,
                    "base_branch": self.base_branch,
                    "branch_hint": self.branch_hint,
                }],
            },
        })
    }
}

pub fn job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    TestJobContext {
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        queue: "code_ready".to_string(),
        artifact_kind: "code".to_string(),
        default_branch: "main".to_string(),
        base_branch: "main".to_string(),
        branch_hint: branch_hint.to_string(),
        correlation_key: correlation_key.to_string(),
        access: "writable".to_string(),
        artifact: Some(TestJobArtifactSnapshot {
            number: 7,
            title: "Implement the thing".to_string(),
            body: "Detailed issue body".to_string(),
            labels: vec!["code".to_string(), "ready".to_string()],
            state: "Open".to_string(),
        }),
        action: Some("open_pr".to_string()),
        checkout_capability: Some("writable".to_string()),
        allowed_verdicts: vec![],
        verdict_contracts: Default::default(),
        source_metadata: Default::default(),
        pull_request_freshness: None,
    }
}

pub fn read_only_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.role = "architect".to_string();
    context.queue = "design_review".to_string();
    context.artifact_kind = "triage".to_string();
    context.action = Some("triage_intake".to_string());
    context.checkout_capability = Some("read_only".to_string());
    context.allowed_verdicts = vec!["ready_code".to_string(), "needs_design".to_string()];
    context
}

pub fn native_validation_job_context(
    branch_hint: &str,
    correlation_key: &str,
    source_branch: &str,
) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.role = "tester".to_string();
    context.queue = "plan_needs_validation".to_string();
    context.artifact_kind = "plan".to_string();
    context.action = Some("validate_plan".to_string());
    context.checkout_capability = Some("read_only".to_string());
    context.access = "read_only".to_string();
    context.allowed_verdicts = vec!["validated".to_string(), "needs_followup".to_string()];
    context.base_branch = source_branch.to_string();
    context.source_metadata = BTreeMap::from([
        ("target_branch".to_string(), source_branch.to_string()),
        (
            "validation_binding_id".to_string(),
            "validate_exact_feature_head".to_string(),
        ),
        (
            "validation_idempotency_key".to_string(),
            "validator:{binding_id}:plan:{issue_number}:head:{exact_head_sha}".to_string(),
        ),
        (
            "validation_feature".to_string(),
            "acme/service#778".to_string(),
        ),
        ("validation_plan".to_string(), "acme/service#7".to_string()),
        (
            "validation_source_branch".to_string(),
            source_branch.to_string(),
        ),
    ]);
    context
}

pub fn writable_job_context_with_allowed_verdicts(
    branch_hint: &str,
    correlation_key: &str,
    allowed_verdicts: &[&str],
) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.action = Some("open_pr".to_string());
    context.checkout_capability = Some("writable".to_string());
    context.allowed_verdicts = allowed_verdicts
        .iter()
        .map(|verdict| (*verdict).to_string())
        .collect();
    context
}

pub fn pr_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.role = "reviewer".to_string();
    context.queue = "pr_needs_review".to_string();
    context.artifact_kind = "implementation_pr".to_string();
    context.action = Some("review_pr".to_string());
    context.checkout_capability = Some("pull_request_read_only".to_string());
    context.allowed_verdicts = vec![
        "approve".to_string(),
        "changes".to_string(),
        "escalate".to_string(),
    ];
    context
}

pub fn pr_fix_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = job_context(branch_hint, correlation_key);
    context.queue = "pr_ci_failed".to_string();
    context.artifact_kind = "implementation_pr".to_string();
    context.action = Some("address_ci_failure".to_string());
    context.checkout_capability = Some("pull_request_writable".to_string());
    context.pull_request_freshness = Some(json!({
        "repository_id": "repo-1",
        "repo": "acme/service",
        "role": "engineer",
        "queue": "pr_ci_failed",
        "action": "address_ci_failure",
        "number": 7,
        "pull_request_id": "pr-7",
        "head_sha": "assigned-head",
        "queue_condition": "ci_failed"
    }));
    context
}

pub fn pr_merge_conflict_job_context(branch_hint: &str, correlation_key: &str) -> TestJobContext {
    let mut context = pr_fix_job_context(branch_hint, correlation_key);
    context.queue = "pr_merge_conflict".to_string();
    context.action = Some("resolve_merge_conflict".to_string());
    context.pull_request_freshness = Some(json!({
        "repository_id": "repo-1",
        "repo": "acme/service",
        "role": "engineer",
        "queue": "pr_merge_conflict",
        "action": "resolve_merge_conflict",
        "number": 7,
        "pull_request_id": "pr-7",
        "head_sha": "assigned-head",
        "queue_labels": ["merge-conflict"]
    }));
    context
}

pub fn pr_fix_assign(branch_hint: &str, correlation_key: &str) -> Assign {
    let context = pr_fix_job_context(branch_hint, correlation_key);
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: "acme/service/pull_request-7/engineer/pr_ci_failed".to_string(),
        attempt_id: Some(format!("attempt-{correlation_key}")),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "pull_request".to_string(),
        },
        job_payload: context.to_payload(),
    }
}

pub fn pr_merge_conflict_assign(branch_hint: &str, correlation_key: &str) -> Assign {
    let context = pr_merge_conflict_job_context(branch_hint, correlation_key);
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: "acme/service/pull_request-7/engineer/pr_merge_conflict".to_string(),
        attempt_id: Some(format!("attempt-{correlation_key}")),
        role: "engineer".to_string(),
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "pull_request".to_string(),
        },
        job_payload: context.to_payload(),
    }
}

pub fn assign_with_context(correlation_key: &str, context: TestJobContext) -> Assign {
    let role = context.role.clone();
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        trace_context: None,
        job_id: format!("acme/service/issue-7/{role}/{correlation_key}"),
        attempt_id: Some(format!("attempt-{correlation_key}")),
        role,
        repo: "acme/service".to_string(),
        artifact: Artifact {
            item: json!(7),
            kind: "issue".to_string(),
        },
        job_payload: context.to_payload(),
    }
}

fn seed_origin(origin: &Path, temp: &Path) -> String {
    let seed = temp.join("seed");
    git(["init", "-b", "main", path_str(&seed)]);
    fs::write(seed.join("README.md"), "# seed\n").expect("write seed file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "README.md",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "initial commit",
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

    git(["-C", path_str(&seed), "checkout", "-b", "review-head"]);
    fs::write(seed.join("pr-change.txt"), "pull request change\n").expect("write PR file");
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "add",
        "pr-change.txt",
    ]);
    git([
        "-C",
        path_str(&seed),
        "-c",
        "user.name=Seed User",
        "-c",
        "user.email=seed@example.test",
        "commit",
        "-m",
        "pull request change",
    ]);
    let pull_request_head_sha = git_output(["-C", path_str(&seed), "rev-parse", "HEAD"]);
    git([
        "-C",
        path_str(&seed),
        "push",
        "origin",
        "HEAD:refs/temper/seed/pr-7",
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "refs/pull/7/head",
        pull_request_head_sha.as_str(),
    ]);
    git([
        "-C",
        path_str(origin),
        "update-ref",
        "-d",
        "refs/temper/seed/pr-7",
    ]);
    pull_request_head_sha
}
