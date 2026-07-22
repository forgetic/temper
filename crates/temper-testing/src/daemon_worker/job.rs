use std::path::{Path, PathBuf};

use temper_protocol_worker::{
    Assign, Branch, Failure, FailureClass, JobContext, JobResult, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION,
};

use super::{CI_PASS_MARKER, CiSentinel, DaemonWorkerConfig, GitIdentity};

/// Directory (off the repo root) the deterministic change file is written to.
/// Distinct from the `.temper-pr-prep`/`.temper-ci` bookkeeping paths so the
/// worker's diff is a "product" change as far as the fixture is concerned.
const CHANGE_DIR: &str = "temper-daemon-worker";

struct JobError {
    class: FailureClass,
    message: String,
}

impl JobError {
    fn protocol(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Protocol,
            message: message.into(),
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            class: FailureClass::Transient,
            message: message.into(),
        }
    }
}

pub(super) async fn execute_job(
    cx: &temper_engine_io::Cx,
    config: &DaemonWorkerConfig,
    identity: &GitIdentity,
    assign: &Assign,
) -> JobResult {
    match run_job(cx, config, identity, assign).await {
        Ok((repos, summary)) => JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: config.worker_id.clone(),
            job_id: assign.job_id.clone(),
            attempt_id: assign.attempt_id.clone(),
            status: ResultStatus::Success,
            repos,
            verdict: None,
            title: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: Some(summary),
            details: None,
        },
        Err(error) => JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: config.worker_id.clone(),
            job_id: assign.job_id.clone(),
            attempt_id: assign.attempt_id.clone(),
            status: ResultStatus::Failure,
            repos: Vec::new(),
            verdict: None,
            title: None,
            body: None,
            children: Vec::new(),
            failure: Some(Failure {
                class: error.class,
                message: error.message,
            }),
            summary: None,
            details: None,
        },
    }
}

async fn run_job(
    cx: &temper_engine_io::Cx,
    config: &DaemonWorkerConfig,
    identity: &GitIdentity,
    assign: &Assign,
) -> Result<(Vec<RepoOutcome>, String), JobError> {
    let context: JobContext = serde_json::from_value(assign.job_payload.clone())
        .map_err(|error| JobError::protocol(format!("job payload is not a JobContext: {error}")))?;
    let pull_request_repair = match assign.artifact.kind.as_str() {
        "issue" => false,
        "pull_request"
            if context.queue == "pr_ci_failed"
                && context.action.as_deref() == Some("address_ci_failure")
                && context.checkout_capability.as_deref() == Some("pull_request_writable") =>
        {
            true
        }
        kind => {
            return Err(JobError::protocol(format!(
                "daemon test worker only implements issue coding jobs and pull_request_writable pr_ci_failed repairs, got artifact kind '{kind}', queue '{}', action {:?}",
                context.queue, context.action
            )));
        }
    };
    let manifest = context
        .workspace
        .ok_or_else(|| JobError::protocol("job payload is missing enriched workspace manifest"))?;
    let issue_number = (!pull_request_repair)
        .then(|| context.artifact.as_ref().map(|artifact| artifact.number))
        .flatten();
    let coordination_key = manifest.coordination_key.clone();

    // One commit/push per writable repo (ADR 0023). This deterministic worker
    // does not build, so it needs no sibling layout: each writable repo is
    // checked out independently and produces a RepoOutcome -> one PR.
    let mut outcomes = Vec::new();
    for (index, repo_spec) in manifest.repos.iter().enumerate() {
        if !repo_spec.is_writable() {
            continue;
        }
        let branch_name = repo_spec.branch_hint.clone().ok_or_else(|| {
            JobError::protocol(format!(
                "writable workspace repo {} is missing a branch hint",
                repo_spec.repo
            ))
        })?;
        let base_branch = if repo_spec.base_branch.trim().is_empty() {
            "main".to_string()
        } else {
            repo_spec.base_branch.clone()
        };
        let (owner, name) = repo_spec.owner_name().ok_or_else(|| {
            JobError::protocol(format!("malformed workspace repo path {}", repo_spec.repo))
        })?;

        let workspace = Workspace {
            cx,
            path: config
                .workspace_root
                .join(repo_spec.repo.replace('/', "__"))
                .join(&context.role),
            remote_url: format!(
                "{}/{}/{}.git",
                config.git_base_url.trim_end_matches('/'),
                owner,
                name
            ),
            identity,
        };

        let source_branch = if pull_request_repair {
            branch_name.as_str()
        } else {
            base_branch.as_str()
        };
        workspace.prepare(source_branch, &branch_name).await?;

        let change_path = workspace
            .path
            .join(CHANGE_DIR)
            .join(format!("{}.txt", branch_name.replace('/', "-")));
        if let Some(parent) = change_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                JobError::transient(format!("creating change dir failed: {error}"))
            })?;
        }
        let operation = if pull_request_repair {
            "CI repair"
        } else {
            "implementation"
        };
        std::fs::write(
            &change_path,
            format!(
                "Deterministic daemon test-worker {operation}.\njob_id: {}\nrepo: {}\nbranch: {branch_name}\n",
                assign.job_id, repo_spec.repo
            ),
        )
        .map_err(|error| JobError::transient(format!("writing change file failed: {error}")))?;

        let mut message = if pull_request_repair {
            format!("Repair {coordination_key} {CI_PASS_MARKER}")
        } else {
            let mut message = format!("Implement {coordination_key}");
            if config.ci_sentinel == CiSentinel::Present {
                message.push_str(&format!(" {CI_PASS_MARKER}"));
            }
            message
        };
        message.push_str(&format!(
            "\n\nDeterministic test-worker {operation} for job {} ({}).",
            assign.job_id, repo_spec.repo
        ));
        // Native provider close-on-merge only works within the coordinating
        // issue's own repo, so the `Closes #n` trailer goes on the primary
        // repo's commit only (the first manifest entry).
        if index == 0 {
            if let Some(number) = issue_number {
                message.push_str(&format!("\n\nCloses #{number}"));
            }
        }

        workspace.commit_all(&message).await?;
        let head_sha = workspace.push_branch(&branch_name).await?;

        outcomes.push(RepoOutcome {
            repo: repo_spec.repo.clone(),
            branch: Branch {
                name: branch_name,
                head_sha,
            },
        });
    }

    if outcomes.is_empty() {
        return Err(JobError::protocol(
            "workspace manifest declared no writable repositories",
        ));
    }

    let summary = if pull_request_repair {
        format!("deterministic CI repair for {coordination_key}")
    } else {
        format!("deterministic test-worker change for {coordination_key}")
    };
    Ok((outcomes, summary))
}

/// Minimal clone-or-fetch git workspace driven through the git CLI, mirroring
/// the production smith-worker workspace shape (auth via `http.extraheader`,
/// token redacted from errors).
struct Workspace<'a> {
    cx: &'a temper_engine_io::Cx,
    path: PathBuf,
    remote_url: String,
    identity: &'a GitIdentity,
}

impl Workspace<'_> {
    async fn prepare(&self, base_branch: &str, work_branch: &str) -> Result<(), JobError> {
        if self.path.exists() {
            self.git(
                Some(&self.path),
                false,
                &["remote", "set-url", "origin", self.remote_url.as_str()],
            )
            .await?;
        } else {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    JobError::transient(format!("creating workspace dir failed: {error}"))
                })?;
            }
            let path = self.path_str()?.to_string();
            self.git(
                None,
                true,
                &["clone", "--no-checkout", self.remote_url.as_str(), &path],
            )
            .await?;
        }

        let refspec = format!("+refs/heads/{base_branch}:refs/remotes/origin/{base_branch}");
        self.git(Some(&self.path), true, &["fetch", "origin", &refspec])
            .await?;
        self.git(
            Some(&self.path),
            false,
            &[
                "checkout",
                "-B",
                work_branch,
                &format!("origin/{base_branch}"),
            ],
        )
        .await?;
        Ok(())
    }

    async fn commit_all(&self, message: &str) -> Result<(), JobError> {
        self.git(Some(&self.path), false, &["add", "-A"]).await?;
        self.git(Some(&self.path), false, &["commit", "-m", message])
            .await?;
        Ok(())
    }

    async fn push_branch(&self, branch_name: &str) -> Result<String, JobError> {
        let refspec = format!("HEAD:refs/heads/{branch_name}");
        self.git(Some(&self.path), true, &["push", "origin", &refspec])
            .await?;
        let output = self
            .git(Some(&self.path), false, &["rev-parse", "HEAD"])
            .await?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn path_str(&self) -> Result<&str, JobError> {
        self.path
            .to_str()
            .ok_or_else(|| JobError::transient("workspace path is not valid UTF-8"))
    }

    async fn git(
        &self,
        current_dir: Option<&Path>,
        include_remote_auth: bool,
        args: &[&str],
    ) -> Result<skein::process::Output, JobError> {
        let mut git = skein::process::Command::new("git");
        git.env("GIT_TERMINAL_PROMPT", "0")
            .arg("-c")
            .arg(format!("user.name={}", self.identity.user))
            .arg("-c")
            .arg(format!("user.email={}", self.identity.email));
        if include_remote_auth {
            if let Some(token) = self.identity.token.as_deref() {
                git.arg("-c")
                    .arg(format!("http.extraheader=AUTHORIZATION: token {token}"));
            }
        }
        if let Some(current_dir) = current_dir {
            git.arg("-C").arg(current_dir);
        }
        git.args(args);
        let output = git
            .output_async(self.cx)
            .await
            .map_err(|error| JobError::transient(format!("spawning git failed: {error}")))?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(JobError::transient(self.redact(format!(
                "git {} failed (status {}): {}",
                args.first().copied().unwrap_or("?"),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ))))
        }
    }

    fn redact(&self, text: String) -> String {
        match self.identity.token.as_deref() {
            Some(token) if !token.is_empty() => text.replace(token, "<redacted>"),
            _ => text,
        }
    }
}
