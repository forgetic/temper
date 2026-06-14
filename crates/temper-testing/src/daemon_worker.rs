//! Minimal wire-protocol test worker that stands in for `smith-worker`.
//!
//! The daemon-topology e2e tests need a worker-tier process that speaks the
//! Worker/Daemon Wire Protocol v1 with full fidelity (`register` → `poll` →
//! `assign` → `result`) and performs a *real* git push as the assigned role's
//! git identity — without dragging `temper-runner`, the fake agents, or any
//! Forge client into the worker tier. This module is that stand-in: a
//! deterministic, protocol-faithful coding worker whose "implementation" is one
//! deterministic change file per job.
//!
//! Protocol discipline matches production `smith-worker`:
//!
//! - The worker never calls the Forge API. Its only outputs are the pushed
//!   branch and the structured `result` message; the daemon owns PR creation.
//! - Issue-targeted jobs with the enriched standard payload succeed; payloads
//!   missing the enrichment (or non-issue artifacts, which the daemon's
//!   `pr_ci_failed` feed can produce) fail with `protocol` class, mirroring
//!   `smith-worker`'s `CodingExecutor`.
//! - Git failures are reported as `transient` failures so the daemon can
//!   re-enqueue.
//!
//! Git identity and token come from the environment ([`GIT_USER_ENV`] /
//! [`GIT_TOKEN_ENV`]), never argv, and the token is redacted from every error
//! message. The token is optional so hermetic tests can push over `file://`
//! remotes without any credential.
//!
//! The commit the worker pushes carries two load-bearing message fragments:
//!
//! - With `--ci-sentinel present` the subject line includes
//!   [`CI_PASS_MARKER`], so the provisioned e2e CI workflow (which gates on the
//!   head commit's message) passes immediately. With `deferred` the marker is
//!   omitted and CI fails until something else (the red→green e2e variant)
//!   pushes a marker-bearing commit.
//! - A `Closes #<issue>` trailer referencing the source issue, so merging the
//!   implementation PR closes the issue through the real provider's native
//!   close-on-merge keyword handling — the daemon topology has no role that
//!   closes source issues, and the e2e asserts this real wiring.

use std::path::{Path, PathBuf};
use std::time::Duration;

use temper_worker_protocol::{
    Assign, Branch, Capability, Capacity, ErrorCode, Failure, FailureClass, JobContext, JobResult,
    Poll, Register, RepoOutcome, ResultStatus, WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

pub use crate::forgejo_server::provision::CI_PASS_MARKER;

/// Environment variable carrying the git author/committer login.
pub const GIT_USER_ENV: &str = "TEMPER_E2E_GIT_USER";
/// Environment variable carrying the git push token (optional).
pub const GIT_TOKEN_ENV: &str = "TEMPER_E2E_GIT_TOKEN";

/// Directory (off the repo root) the deterministic change file is written to.
/// Distinct from the `.temper-pr-prep`/`.temper-ci` bookkeeping paths so the
/// worker's diff is a "product" change as far as the fixture is concerned.
const CHANGE_DIR: &str = "temper-daemon-worker";

/// Default long-poll wait. Short enough that the stop-file is honored promptly.
const DEFAULT_POLL_WAIT_MS: u64 = 2_000;

/// Command-line usage for the deterministic daemon test worker.
pub const USAGE: &str = concat!(
    "temper-testing-daemon-worker --daemon-url <http://host:port> ",
    "--worker-id <id> --capability <owner/name:role> [--capability ...] ",
    "--git-base-url <url> --workspace-root <dir> --stop-file <path> ",
    "[--ci-sentinel present|deferred] [--poll-wait-ms <n>]\n",
    "  Git identity comes from TEMPER_E2E_GIT_USER (required) and ",
    "TEMPER_E2E_GIT_TOKEN (optional; omit for file:// remotes)"
);

/// Whether the worker's commit message carries the CI pass marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CiSentinel {
    /// Subject line includes [`CI_PASS_MARKER`]; CI passes on the pushed head.
    Present,
    /// Marker omitted; CI fails until a marker-bearing commit lands later.
    Deferred,
}

/// Git author/committer identity plus optional push token, read from env.
#[derive(Clone)]
pub struct GitIdentity {
    pub user: String,
    pub email: String,
    pub token: Option<String>,
}

impl std::fmt::Debug for GitIdentity {
    /// Redacts the token so a `{:?}` can never leak it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitIdentity")
            .field("user", &self.user)
            .field("email", &self.email)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl GitIdentity {
    /// Reads the identity from [`GIT_USER_ENV`] / [`GIT_TOKEN_ENV`]. The email
    /// is derived the same way the e2e fixture provisions role users.
    pub fn from_env() -> Result<Self, String> {
        let user = std::env::var(GIT_USER_ENV)
            .ok()
            .map(|user| user.trim().to_string())
            .filter(|user| !user.is_empty())
            .ok_or_else(|| format!("missing required environment variable {GIT_USER_ENV}"))?;
        let token = std::env::var(GIT_TOKEN_ENV)
            .ok()
            .map(|token| token.trim().to_string())
            .filter(|token| !token.is_empty());
        Ok(Self {
            email: format!("{user}@example.invalid"),
            user,
            token,
        })
    }
}

/// Fully parsed daemon test worker configuration.
#[derive(Clone, Debug)]
pub struct DaemonWorkerConfig {
    pub daemon_url: String,
    pub worker_id: String,
    pub capabilities: Vec<Capability>,
    pub git_base_url: String,
    pub workspace_root: PathBuf,
    pub stop_file: PathBuf,
    pub ci_sentinel: CiSentinel,
    pub poll_wait_ms: u64,
}

/// Result of parsing command-line arguments.
#[derive(Clone, Debug)]
pub enum ParseOutcome {
    Help,
    Run(DaemonWorkerConfig),
}

/// Parses the daemon test worker command-line surface.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<ParseOutcome, String> {
    let mut daemon_url = None;
    let mut worker_id = None;
    let mut capabilities = Vec::new();
    let mut git_base_url = None;
    let mut workspace_root = None;
    let mut stop_file = None;
    let mut ci_sentinel = CiSentinel::Present;
    let mut poll_wait_ms = DEFAULT_POLL_WAIT_MS;

    let mut iter = args.into_iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--daemon-url" => daemon_url = Some(value_for(&flag, &mut iter)?),
            "--worker-id" => worker_id = Some(value_for(&flag, &mut iter)?),
            "--capability" => capabilities.push(parse_capability(&value_for(&flag, &mut iter)?)?),
            "--git-base-url" => git_base_url = Some(value_for(&flag, &mut iter)?),
            "--workspace-root" => {
                workspace_root = Some(PathBuf::from(value_for(&flag, &mut iter)?))
            }
            "--stop-file" => stop_file = Some(PathBuf::from(value_for(&flag, &mut iter)?)),
            "--ci-sentinel" => {
                ci_sentinel = match value_for(&flag, &mut iter)?.as_str() {
                    "present" => CiSentinel::Present,
                    "deferred" => CiSentinel::Deferred,
                    other => {
                        return Err(format!(
                            "--ci-sentinel must be 'present' or 'deferred', got '{other}'"
                        ));
                    }
                }
            }
            "--poll-wait-ms" => {
                let raw = value_for(&flag, &mut iter)?;
                poll_wait_ms = raw
                    .parse::<u64>()
                    .map_err(|error| format!("--poll-wait-ms must be an integer: {error}"))?;
                if poll_wait_ms == 0 {
                    return Err("--poll-wait-ms must be positive".to_string());
                }
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }

    if capabilities.is_empty() {
        return Err("missing required --capability".to_string());
    }
    Ok(ParseOutcome::Run(DaemonWorkerConfig {
        daemon_url: daemon_url.ok_or("missing required --daemon-url")?,
        worker_id: worker_id.ok_or("missing required --worker-id")?,
        capabilities,
        git_base_url: git_base_url.ok_or("missing required --git-base-url")?,
        workspace_root: workspace_root.ok_or("missing required --workspace-root")?,
        stop_file: stop_file.ok_or("missing required --stop-file")?,
        ci_sentinel,
        poll_wait_ms,
    }))
}

fn value_for(flag: &str, iter: &mut impl Iterator<Item = String>) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_capability(raw: &str) -> Result<Capability, String> {
    let (repo, role) = raw
        .split_once(':')
        .ok_or_else(|| format!("--capability must be owner/name:role, got '{raw}'"))?;
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("--capability repo must be owner/name, got '{raw}'"))?;
    if owner.is_empty() || name.is_empty() || name.contains('/') || role.is_empty() {
        return Err(format!("--capability must be owner/name:role, got '{raw}'"));
    }
    Ok(Capability {
        role: role.to_string(),
        repo: repo.to_string(),
    })
}

/// Runs the worker loop until the stop-file appears.
///
/// `register` → `poll` → on `assign` execute the job and send `result` →
/// re-poll. Transport errors are logged and retried (the e2e driver bounds the
/// run with the stop-file and its own convergence timeout).
pub async fn run(
    cx: &temper_engine_io::Cx,
    config: &DaemonWorkerConfig,
    identity: &GitIdentity,
) -> Result<(), String> {
    let client = temper_engine_io::http::JsonClient::new();
    let endpoint = format!("{}/v1/message", config.daemon_url.trim_end_matches('/'));

    let register = WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: config.worker_id.clone(),
        capabilities: config.capabilities.clone(),
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    });
    match send(&client, &endpoint, &register).await? {
        Some(WorkerProtocolMessage::Error(error)) => {
            return Err(format!(
                "daemon rejected registration: {:?}: {}",
                error.code, error.message
            ));
        }
        _ => eprintln!(
            "temper-testing-daemon-worker: registered worker_id={}",
            config.worker_id
        ),
    }

    while !config.stop_file.exists() {
        let poll = WorkerProtocolMessage::Poll(Poll {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: config.worker_id.clone(),
            free_capacity: 1,
            max_wait_ms: Some(config.poll_wait_ms),
        });
        let response = match send(&client, &endpoint, &poll).await {
            Ok(response) => response,
            Err(error) => {
                eprintln!("temper-testing-daemon-worker: poll failed: {error}");
                temper_engine_io::runtime::sleep_for(cx, Duration::from_millis(200)).await;
                continue;
            }
        };
        match response {
            Some(WorkerProtocolMessage::Assign(assign)) => {
                eprintln!(
                    "temper-testing-daemon-worker: assigned job_id={} repo={} role={} artifact={}/{}",
                    assign.job_id,
                    assign.repo,
                    assign.role,
                    assign.artifact.kind,
                    assign.artifact.item
                );
                let result = execute_job(cx, config, identity, &assign).await;
                eprintln!(
                    "temper-testing-daemon-worker: job_id={} finished status={:?} failure={:?}",
                    assign.job_id,
                    result.status,
                    result.failure.as_ref().map(|failure| failure.class)
                );
                let message = WorkerProtocolMessage::Result(result);
                match send(&client, &endpoint, &message).await {
                    Ok(Some(WorkerProtocolMessage::Release(release))) => eprintln!(
                        "temper-testing-daemon-worker: job_id={} released disposition={:?}",
                        release.job_id, release.disposition
                    ),
                    Ok(other) => eprintln!(
                        "temper-testing-daemon-worker: unexpected result response: {other:?}"
                    ),
                    Err(error) => {
                        eprintln!("temper-testing-daemon-worker: result send failed: {error}")
                    }
                }
            }
            Some(WorkerProtocolMessage::Error(error)) if error.code == ErrorCode::PollTimeout => {}
            Some(other) => {
                eprintln!("temper-testing-daemon-worker: unexpected poll response: {other:?}")
            }
            None => {}
        }
    }

    eprintln!(
        "temper-testing-daemon-worker: stop file {} present; exiting",
        config.stop_file.display()
    );
    Ok(())
}

async fn send(
    client: &temper_engine_io::http::JsonClient,
    endpoint: &str,
    message: &WorkerProtocolMessage,
) -> Result<Option<WorkerProtocolMessage>, String> {
    let payload = serde_json::to_value(message)
        .map_err(|error| format!("serializing protocol message failed: {error}"))?;
    let response = client
        .send("POST", endpoint, None, Some(&payload))
        .await
        .map_err(|error| format!("request to {endpoint} failed: {error}"))?;
    if response.status == 204 || response.body.is_empty() {
        return Ok(None);
    }
    if response.status != 200 {
        return Err(format!(
            "daemon returned HTTP {} from {endpoint}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }
    serde_json::from_slice(&response.body)
        .map(Some)
        .map_err(|error| format!("daemon response was not valid protocol JSON: {error}"))
}

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

async fn execute_job(
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
            status: ResultStatus::Success,
            repos,
            verdict: None,
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
            status: ResultStatus::Failure,
            repos: Vec::new(),
            verdict: None,
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
    if assign.artifact.kind != "issue" {
        // Mirrors smith-worker: only issue-targeted coding jobs are
        // implemented; PR-targeted feeds (e.g. pr_ci_failed) fail as protocol.
        return Err(JobError::protocol(format!(
            "daemon test worker only implements issue-targeted coding jobs, got artifact kind '{}'",
            assign.artifact.kind
        )));
    }
    let context: JobContext = serde_json::from_value(assign.job_payload.clone())
        .map_err(|error| JobError::protocol(format!("job payload is not a JobContext: {error}")))?;
    let manifest = context
        .workspace
        .ok_or_else(|| JobError::protocol("job payload is missing enriched workspace manifest"))?;
    let issue_number = context.artifact.as_ref().map(|artifact| artifact.number);
    let coordination_key = manifest.coordination_key.clone();

    // One commit/push per writable repo (ADR 0023). This deterministic worker
    // does not build, so it needs no sibling layout: each writable repo is
    // checked out independently and produces a RepoOutcome → one PR.
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

        workspace.prepare(&base_branch, &branch_name).await?;

        let change_path = workspace
            .path
            .join(CHANGE_DIR)
            .join(format!("{}.txt", branch_name.replace('/', "-")));
        if let Some(parent) = change_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                JobError::transient(format!("creating change dir failed: {error}"))
            })?;
        }
        std::fs::write(
            &change_path,
            format!(
                "Deterministic daemon test-worker change.\njob_id: {}\nrepo: {}\nbranch: {branch_name}\n",
                assign.job_id, repo_spec.repo
            ),
        )
        .map_err(|error| JobError::transient(format!("writing change file failed: {error}")))?;

        let mut message = format!("Implement {coordination_key}");
        if config.ci_sentinel == CiSentinel::Present {
            message.push_str(&format!(" {CI_PASS_MARKER}"));
        }
        message.push_str(&format!(
            "\n\nDeterministic test-worker change for job {} ({}).",
            assign.job_id, repo_spec.repo
        ));
        // Native provider close-on-merge only works within the coordinating
        // issue's own repo, so the `Closes #n` trailer goes on the primary
        // repo's commit only (the first manifest entry).
        if index == 0
            && let Some(number) = issue_number
        {
            message.push_str(&format!("\n\nCloses #{number}"));
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

    Ok((
        outcomes,
        format!("deterministic test-worker change for {coordination_key}"),
    ))
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
        if include_remote_auth && let Some(token) = self.identity.token.as_deref() {
            git.arg("-c")
                .arg(format!("http.extraheader=AUTHORIZATION: token {token}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    fn required() -> Vec<String> {
        strings(&[
            "--daemon-url",
            "http://127.0.0.1:8080",
            "--worker-id",
            "w1",
            "--capability",
            "acme/service:engineer",
            "--git-base-url",
            "http://127.0.0.1:3000",
            "--workspace-root",
            "/tmp/ws",
            "--stop-file",
            "/tmp/stop",
        ])
    }

    fn run_config(args: Vec<String>) -> DaemonWorkerConfig {
        match parse_args(args).expect("arguments parse") {
            ParseOutcome::Run(config) => config,
            ParseOutcome::Help => panic!("expected run config"),
        }
    }

    #[test]
    fn parses_required_flags_with_defaults() {
        let config = run_config(required());

        assert_eq!(config.daemon_url, "http://127.0.0.1:8080");
        assert_eq!(config.worker_id, "w1");
        assert_eq!(
            config.capabilities,
            vec![Capability {
                role: "engineer".to_string(),
                repo: "acme/service".to_string(),
            }]
        );
        assert_eq!(config.git_base_url, "http://127.0.0.1:3000");
        assert_eq!(config.workspace_root, PathBuf::from("/tmp/ws"));
        assert_eq!(config.stop_file, PathBuf::from("/tmp/stop"));
        assert_eq!(config.ci_sentinel, CiSentinel::Present);
        assert_eq!(config.poll_wait_ms, DEFAULT_POLL_WAIT_MS);
    }

    #[test]
    fn parses_deferred_sentinel_and_poll_wait() {
        let mut args = required();
        args.extend(strings(&[
            "--ci-sentinel",
            "deferred",
            "--poll-wait-ms",
            "500",
        ]));

        let config = run_config(args);

        assert_eq!(config.ci_sentinel, CiSentinel::Deferred);
        assert_eq!(config.poll_wait_ms, 500);
    }

    #[test]
    fn rejects_malformed_capabilities() {
        for raw in ["acme/service", "acme:engineer", "acme/:engineer", "a/b:"] {
            let mut args = required();
            args.extend(strings(&["--capability", raw]));
            let error = parse_args(args).unwrap_err();
            assert!(error.contains("--capability"), "error for {raw:?}: {error}");
        }
    }

    #[test]
    fn missing_required_flag_is_rejected() {
        let args = strings(&["--daemon-url", "http://127.0.0.1:8080"]);
        let error = parse_args(args).unwrap_err();
        assert!(error.contains("--capability"));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let mut args = required();
        args.push("--mystery".to_string());
        let error = parse_args(args).unwrap_err();
        assert!(error.contains("--mystery"));
    }

    #[test]
    fn help_flag_short_circuits() {
        assert!(matches!(
            parse_args(strings(&["--help"])).expect("help parses"),
            ParseOutcome::Help
        ));
    }
}
