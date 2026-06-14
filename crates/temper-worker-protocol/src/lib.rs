// SPDX-License-Identifier: MPL-2.0

//! Serialization-only DTOs for the Temper Worker/Daemon Wire Protocol v1.
//!
//! This crate intentionally has no Temper runtime dependencies. It provides the
//! stable JSON shapes that workers and daemons can share without coupling Smith
//! or other worker implementations to Temper runner, workflow, backend, daemon,
//! deployment, or Forge crates.
//!
//! # Multi-repo co-development jobs (ADR 0023)
//!
//! The protocol is still `v1` (pre-1.0 alpha): we revise it in place rather than
//! bumping the version, even for wire-incompatible changes.
//!
//! A coding job's checkout target is a [`WorkspaceManifest`] — an ordered set of
//! repositories laid out as siblings so their inter-repo path dependencies
//! resolve. The worker assembles them all, runs one agent turn over the combined
//! root, and reports one [`RepoOutcome`] per *writable* repository that produced
//! a diff. The daemon opens one pull request per outcome. A single-repo job is
//! the degenerate manifest of one writable primary repo.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WORKER_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum WorkerProtocolMessage {
    Register(Register),
    Poll(Poll),
    Assign(Assign),
    Heartbeat(Heartbeat),
    Result(JobResult),
    Progress(JobProgress),
    Release(Release),
    LeaseAck(LeaseAck),
    Error(ProtocolError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub role: String,
    pub repo: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capacity {
    pub max_concurrent_jobs: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Register {
    pub protocol_version: u32,
    pub worker_id: String,
    pub capabilities: Vec<Capability>,
    pub capacity: Capacity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Poll {
    pub protocol_version: u32,
    pub worker_id: String,
    pub free_capacity: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wait_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    /// String or numeric artifact identity, preserved as JSON per the protocol.
    pub item: Value,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assign {
    pub protocol_version: u32,
    pub job_id: String,
    pub role: String,
    /// Primary repository path (`owner/name`) — the home of the coordinating
    /// artifact. The full repo set to assemble travels in the job payload's
    /// [`WorkspaceManifest`].
    pub repo: String,
    pub artifact: Artifact,
    pub job_payload: Value,
}

/// Whether a workspace repository may be edited (committed, pushed, PR'd) or is
/// present only so the combined build resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoAccess {
    Writable,
    ReadOnly,
}

/// One repository in a job's [`WorkspaceManifest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRepo {
    /// Repository path, `owner/name`.
    pub repo: String,
    /// Relative directory under the workspace root where this repo is checked
    /// out. Chosen so inter-repo path dependencies resolve (e.g. `temper`,
    /// `smith`, `skein` as flat siblings).
    pub dir: String,
    pub access: RepoAccess,
    pub default_branch: String,
    pub base_branch: String,
    /// Work branch the worker pushes for a writable repo, e.g.
    /// `agent/coord-for-code-42`. Absent for read-only repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,
    /// Other repos (by `owner/name` path) whose pull request must land before
    /// this repo's — the coordinated landing order (ADR 0023). The daemon turns
    /// each into a cross-repo dependency link between the opened PRs. Empty for
    /// an independent repo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

impl WorkspaceRepo {
    pub fn is_writable(&self) -> bool {
        matches!(self.access, RepoAccess::Writable)
    }

    /// Splits `repo` into `(owner, name)`. `None` when not exactly `owner/name`.
    pub fn owner_name(&self) -> Option<(&str, &str)> {
        let mut parts = self.repo.split('/');
        let owner = parts.next()?;
        let name = parts.next()?;
        if owner.is_empty() || name.is_empty() || parts.next().is_some() {
            None
        } else {
            Some((owner, name))
        }
    }
}

/// The ordered set of repositories a coding job assembles into one workspace.
///
/// The first repo is the *primary* — the home of the coordinating artifact, off
/// which leases, progress relay, and source-issue resolution key. The
/// `coordination_key` is the stable id for the whole pull-request set (and the
/// cross-plane progress correlation id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub coordination_key: String,
    pub repos: Vec<WorkspaceRepo>,
}

impl WorkspaceManifest {
    /// A one-repo writable manifest — the degenerate single-repo job.
    pub fn single(
        repo: impl Into<String>,
        dir: impl Into<String>,
        default_branch: impl Into<String>,
        base_branch: impl Into<String>,
        branch_hint: impl Into<String>,
        coordination_key: impl Into<String>,
    ) -> Self {
        Self {
            coordination_key: coordination_key.into(),
            repos: vec![WorkspaceRepo {
                repo: repo.into(),
                dir: dir.into(),
                access: RepoAccess::Writable,
                default_branch: default_branch.into(),
                base_branch: base_branch.into(),
                branch_hint: Some(branch_hint.into()),
                depends_on: Vec::new(),
            }],
        }
    }

    /// The primary repository (home of the coordinating artifact).
    pub fn primary(&self) -> Option<&WorkspaceRepo> {
        self.repos.first()
    }

    pub fn writable(&self) -> impl Iterator<Item = &WorkspaceRepo> {
        self.repos.iter().filter(|repo| repo.is_writable())
    }
}

/// Snapshot of the Forge artifact a job acts on, taken at enqueue time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobArtifactSnapshot {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    /// Debug-formatted artifact state, e.g. `Open`.
    pub state: String,
}

/// Standard daemon-owned job payload serialized into `Assign.job_payload`.
///
/// It describes the single coordinating artifact (the issue/PR the job
/// services) plus the multi-repo [`WorkspaceManifest`] to assemble.
///
/// `artifact` and `workspace` are *enrichment* fields: the daemon maps a
/// scanned work item to a thin context first (no Forge access), then fills them
/// from Forge reads before the job is ever dispatched. They are therefore
/// `Option` on the wire DTO but **always present on a job a worker receives**;
/// the worker treats their absence as a protocol error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobContext {
    pub role: String,
    /// Primary repository path (`owner/name`) — home of the coordinating
    /// artifact. Equal to `workspace.repos[0].repo`.
    pub repo: String,
    pub queue: String,
    pub artifact_kind: String,

    /// The coordinating artifact this job services.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<JobArtifactSnapshot>,

    /// The repositories to assemble into the job's workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceManifest>,

    /// Workflow action (intent-level tool / transition id) this job services,
    /// e.g. `open_pr` or `triage_intake`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,

    /// Checkout capability the worker should prepare for the *primary* repo:
    /// `"writable"`, `"read_only"`, or `"pull_request_read_only"`. Absent means
    /// writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_capability: Option<String>,

    /// Verdict vocabulary the job's action declares (the action's `outcomes`
    /// keys). Empty for a plain coding job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_verdicts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatState {
    Running,
    Waiting,
    Finishing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobHeartbeat {
    pub job_id: String,
    pub state: HeartbeatState,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub protocol_version: u32,
    pub worker_id: String,
    pub jobs: Vec<JobHeartbeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_capacity: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    Transient,
    Permanent,
    Canceled,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub head_sha: String,
}

/// The pushed product of one writable repository in a coordinated head result.
/// The daemon opens one pull request per outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoOutcome {
    /// Repository path, `owner/name`.
    pub repo: String,
    pub branch: Branch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub class: FailureClass,
    pub message: String,
}

/// One workspace-authored child issue carried by a breakdown verdict result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobChild {
    /// Stable per-child identifier within the result (seeds the child's
    /// correlation key; referenced by sibling `depends_on`).
    pub slug: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// Slugs of sibling children in the same result that must land before
    /// this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Target repository as an `owner/name` path (the same shape the daemon's
    /// `--repo` flag parses). `None` = the job's own repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub status: ResultStatus,
    /// Per-repo head products — one per writable repo that produced a diff. The
    /// daemon opens one pull request per entry. Empty for a verdict result.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoOutcome>,
    /// Verdict chosen by a verdict job (must be one of the assignment's
    /// `allowed_verdicts`). A success result may carry a verdict and no repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// Authored body accompanying a verdict (e.g. the rewritten issue spec or
    /// the review body).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Child issues authored by a breakdown verdict (e.g. `needs_breakdown`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<JobChild>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// One agent step-progress checkpoint, relayed worker → daemon.
///
/// Carries the agent protocol's `StepProgress` fields plus the message
/// envelope. Per the agent/orchestration split's bright-line rule, this is
/// durable human-facing PR state only (step label, lifecycle phase, pushed
/// sha) — high-frequency observability belongs to the out-of-band control
/// plane and must not grow fields here.
///
/// There is deliberately no `job_id`: the workspace `correlation_key` (the
/// manifest's `coordination_key` value) is the one cross-plane identifier, and
/// the daemon resolves it to its in-flight job. Delivery is fire-and-forget and
/// the daemon applies progress idempotently keyed by
/// `(correlation_key, step, state)`, so re-delivery after worker retry or
/// daemon restart is safe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProgress {
    pub protocol_version: u32,
    pub worker_id: String,
    /// The job's coordination key (echoes `WorkspaceManifest.coordination_key`).
    pub correlation_key: String,
    /// Monotonic step index from the agent, starting at 1.
    pub step: u32,
    /// Short imperative step label, e.g. "write failing test".
    pub status: String,
    /// Lifecycle phase: `"started"` or `"done"`.
    pub state: String,
    /// Commit sha the step pushed, when it pushed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed_sha: Option<String>,
    /// Optional one-line human note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseDisposition {
    Accepted,
    Superseded,
    Reclaimed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub disposition: ReleaseDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseAckDisposition {
    Released,
    UnknownJob,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseAck {
    pub protocol_version: u32,
    pub worker_id: String,
    pub job_id: String,
    pub disposition: LeaseAckDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PollTimeout,
    ProtocolVersionMismatch,
    MalformedMessage,
    UnknownWorker,
    CapacityExceeded,
    HeartbeatMissed,
    JobTimeout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub protocol_version: u32,
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> WorkspaceManifest {
        WorkspaceManifest {
            coordination_key: "coord-for-code-42".to_string(),
            repos: vec![
                WorkspaceRepo {
                    repo: "ai/temper".to_string(),
                    dir: "temper".to_string(),
                    access: RepoAccess::Writable,
                    default_branch: "main".to_string(),
                    base_branch: "main".to_string(),
                    branch_hint: Some("agent/coord-for-code-42".to_string()),
                    depends_on: Vec::new(),
                },
                WorkspaceRepo {
                    repo: "ai/smith".to_string(),
                    dir: "smith".to_string(),
                    access: RepoAccess::Writable,
                    default_branch: "main".to_string(),
                    base_branch: "main".to_string(),
                    branch_hint: Some("agent/coord-for-code-42".to_string()),
                    // smith consumes temper's protocol crate → land after temper.
                    depends_on: vec!["ai/temper".to_string()],
                },
                WorkspaceRepo {
                    repo: "ai/skein".to_string(),
                    dir: "skein".to_string(),
                    access: RepoAccess::ReadOnly,
                    default_branch: "main".to_string(),
                    base_branch: "main".to_string(),
                    branch_hint: None,
                    depends_on: Vec::new(),
                },
            ],
        }
    }

    fn assert_round_trips(json: &str) -> WorkerProtocolMessage {
        let msg: WorkerProtocolMessage = serde_json::from_str(json).expect("fixture parses");
        let encoded = serde_json::to_string(&msg).expect("serializes");
        let again: WorkerProtocolMessage = serde_json::from_str(&encoded).expect("round-trips");
        assert_eq!(msg, again, "round-trip must be lossless");
        msg
    }

    fn protocol_version(msg: &WorkerProtocolMessage) -> u32 {
        match msg {
            WorkerProtocolMessage::Register(msg) => msg.protocol_version,
            WorkerProtocolMessage::Poll(msg) => msg.protocol_version,
            WorkerProtocolMessage::Assign(msg) => msg.protocol_version,
            WorkerProtocolMessage::Heartbeat(msg) => msg.protocol_version,
            WorkerProtocolMessage::Result(msg) => msg.protocol_version,
            WorkerProtocolMessage::Progress(msg) => msg.protocol_version,
            WorkerProtocolMessage::Release(msg) => msg.protocol_version,
            WorkerProtocolMessage::LeaseAck(msg) => msg.protocol_version,
            WorkerProtocolMessage::Error(msg) => msg.protocol_version,
        }
    }

    fn fixture_jsons() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/reference/worker-daemon-wire-protocol/examples");
        let mut fixtures = std::fs::read_dir(dir)
            .expect("read protocol fixture directory")
            .map(|entry| entry.expect("fixture entry"))
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    == Some("json")
            })
            .map(|entry| {
                let path = entry.path();
                let name = path
                    .file_name()
                    .expect("fixture has a filename")
                    .to_str()
                    .expect("fixture filename is UTF-8")
                    .to_string();
                let json = std::fs::read_to_string(path).expect("fixture is readable");
                (name, json)
            })
            .collect::<Vec<_>>();
        fixtures.sort_by(|left, right| left.0.cmp(&right.0));
        fixtures
    }

    #[test]
    fn fixtures_round_trip_and_match_variants() {
        for (filename, json) in fixture_jsons() {
            let msg = assert_round_trips(&json);
            assert_eq!(protocol_version(&msg), WORKER_PROTOCOL_VERSION);

            let expected = filename.trim_end_matches(".json");
            match (expected, msg) {
                ("register", WorkerProtocolMessage::Register(_))
                | ("poll", WorkerProtocolMessage::Poll(_))
                | ("assign", WorkerProtocolMessage::Assign(_))
                | ("heartbeat", WorkerProtocolMessage::Heartbeat(_))
                | ("result", WorkerProtocolMessage::Result(_))
                | ("result-verdict", WorkerProtocolMessage::Result(_))
                | ("result-verdict-children", WorkerProtocolMessage::Result(_))
                | ("progress", WorkerProtocolMessage::Progress(_))
                | ("release", WorkerProtocolMessage::Release(_))
                | ("lease-ack", WorkerProtocolMessage::LeaseAck(_))
                | ("error", WorkerProtocolMessage::Error(_)) => {}
                (name, msg) => panic!("{name} parsed as unexpected variant: {msg:?}"),
            }
        }
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let msg: WorkerProtocolMessage = serde_json::from_str(
            r#"{"type":"poll","protocol_version":1,"worker_id":"w1","free_capacity":2,"future_field":"ignored"}"#,
        )
        .expect("unknown fields must be accepted");

        assert!(matches!(msg, WorkerProtocolMessage::Poll(_)));
    }

    #[test]
    fn single_repo_manifest_is_one_writable_primary() {
        let manifest = WorkspaceManifest::single(
            "ai/temper",
            "temper",
            "main",
            "main",
            "agent/pr-for-code-42",
            "pr-for-code-42",
        );
        assert_eq!(manifest.repos.len(), 1);
        let primary = manifest.primary().expect("primary present");
        assert_eq!(primary.repo, "ai/temper");
        assert!(primary.is_writable());
        assert_eq!(manifest.writable().count(), 1);
    }

    #[test]
    fn manifest_round_trips_and_reports_writable_repos() {
        let manifest = sample_manifest();
        assert_eq!(
            manifest
                .writable()
                .map(|repo| repo.repo.as_str())
                .collect::<Vec<_>>(),
            vec!["ai/temper", "ai/smith"]
        );
        assert_eq!(manifest.primary().unwrap().repo, "ai/temper");

        let value = serde_json::to_value(&manifest).expect("manifest serializes");
        assert_eq!(value["coordination_key"], "coord-for-code-42");
        assert_eq!(value["repos"][2]["access"], "read_only");
        assert_eq!(value["repos"][2].get("branch_hint"), None);
        // Landing order: smith lands after temper; independent repos omit it.
        assert_eq!(value["repos"][0].get("depends_on"), None);
        assert_eq!(
            value["repos"][1]["depends_on"],
            serde_json::json!(["ai/temper"])
        );
        let decoded: WorkspaceManifest = serde_json::from_value(value).expect("manifest parses");
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn workspace_repo_owner_name_splits_or_rejects() {
        let writable = &sample_manifest().repos[0];
        assert_eq!(writable.owner_name(), Some(("ai", "temper")));
        let malformed = WorkspaceRepo {
            repo: "ai/temper/extra".to_string(),
            ..writable.clone()
        };
        assert_eq!(malformed.owner_name(), None);
    }

    #[test]
    fn full_job_context_round_trips_without_loss() {
        let context = JobContext {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: Some(JobArtifactSnapshot {
                number: 42,
                title: "Cross-repo worker protocol change".to_string(),
                body: "Change the protocol in temper and consume it in smith.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                state: "Open".to_string(),
            }),
            workspace: Some(sample_manifest()),
            action: Some("open_pr".to_string()),
            checkout_capability: Some("writable".to_string()),
            allowed_verdicts: vec!["needs_architect".to_string()],
        };

        let value = serde_json::to_value(&context).expect("job context serializes");
        let decoded: JobContext =
            serde_json::from_value(value).expect("serialized job context parses");
        assert_eq!(decoded, context);
    }

    #[test]
    fn job_context_omits_empty_optional_fields() {
        let context = JobContext {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: Some(JobArtifactSnapshot {
                number: 42,
                title: "t".to_string(),
                body: "b".to_string(),
                labels: Vec::new(),
                state: "Open".to_string(),
            }),
            workspace: Some(WorkspaceManifest::single(
                "ai/temper",
                "temper",
                "main",
                "main",
                "agent/pr-for-code-42",
                "pr-for-code-42",
            )),
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
        };

        let value = serde_json::to_value(&context).expect("job context serializes");
        assert_eq!(value.get("action"), None);
        assert_eq!(value.get("checkout_capability"), None);
        assert_eq!(value.get("allowed_verdicts"), None);
    }

    #[test]
    fn thin_pre_enrichment_job_context_omits_artifact_and_workspace() {
        // The daemon's pure work-item mapping has no Forge access, so it emits a
        // thin context; enrichment fills artifact + workspace before dispatch.
        let context = JobContext {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
            queue: "code_ready".to_string(),
            artifact_kind: "code".to_string(),
            artifact: None,
            workspace: None,
            action: None,
            checkout_capability: None,
            allowed_verdicts: Vec::new(),
        };

        let value = serde_json::to_value(&context).expect("job context serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "role": "engineer",
                "repo": "ai/temper",
                "queue": "code_ready",
                "artifact_kind": "code"
            })
        );
        let decoded: JobContext = serde_json::from_value(value).expect("thin context parses");
        assert_eq!(decoded, context);
    }

    #[test]
    fn job_context_unknown_fields_are_ignored() {
        let context: JobContext = serde_json::from_value(serde_json::json!({
            "role": "engineer",
            "repo": "ai/temper",
            "queue": "code_ready",
            "artifact_kind": "code",
            "artifact": {
                "number": 42, "title": "t", "body": "b", "labels": [], "state": "Open"
            },
            "workspace": {
                "coordination_key": "pr-for-code-42",
                "repos": [{
                    "repo": "ai/temper", "dir": "temper", "access": "writable",
                    "default_branch": "main", "base_branch": "main",
                    "branch_hint": "agent/pr-for-code-42"
                }]
            },
            "future_field": "ignored"
        }))
        .expect("unknown job context fields must be accepted");

        assert_eq!(context.role, "engineer");
        assert_eq!(context.workspace.expect("workspace present").repos.len(), 1);
    }

    #[test]
    fn coordinated_job_result_round_trips_with_multiple_repos() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            repos: vec![
                RepoOutcome {
                    repo: "ai/temper".to_string(),
                    branch: Branch {
                        name: "agent/coord-for-code-42".to_string(),
                        head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
                    },
                },
                RepoOutcome {
                    repo: "ai/smith".to_string(),
                    branch: Branch {
                        name: "agent/coord-for-code-42".to_string(),
                        head_sha: "89abcdef0123456789abcdef0123456789abcdef".to_string(),
                    },
                },
            ],
            verdict: None,
            body: None,
            children: Vec::new(),
            failure: None,
            summary: Some("implemented coord-for-code-42 across 2 repos".to_string()),
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value["repos"][0]["repo"], "ai/temper");
        assert_eq!(
            value["repos"][1]["branch"]["name"],
            "agent/coord-for-code-42"
        );
        assert_eq!(value.get("verdict"), None);
        let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
        assert_eq!(decoded, result);
    }

    #[test]
    fn verdict_job_result_round_trips_without_repos() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            repos: Vec::new(),
            verdict: Some("ready_code".to_string()),
            body: Some("Rewritten implementation-ready issue body.".to_string()),
            children: Vec::new(),
            failure: None,
            summary: Some("triaged intake".to_string()),
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value.get("repos"), None);
        assert_eq!(value.get("children"), None);
        assert_eq!(value["verdict"], "ready_code");
        let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
        assert_eq!(decoded, result);
    }

    #[test]
    fn verdict_job_result_round_trips_with_children() {
        let result = JobResult {
            protocol_version: WORKER_PROTOCOL_VERSION,
            worker_id: "worker-1".to_string(),
            job_id: "job-123".to_string(),
            status: ResultStatus::Success,
            repos: Vec::new(),
            verdict: Some("needs_breakdown".to_string()),
            body: None,
            children: vec![
                JobChild {
                    slug: "api-schema".to_string(),
                    title: "Define the API schema".to_string(),
                    body: "Write the shared API schema.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: Vec::new(),
                    target_repo: None,
                },
                JobChild {
                    slug: "web-client".to_string(),
                    title: "Implement the web client".to_string(),
                    body: "Build the web client against the API schema.".to_string(),
                    labels: vec!["code".to_string(), "ready".to_string()],
                    depends_on: vec!["api-schema".to_string()],
                    target_repo: Some("acme/web".to_string()),
                },
            ],
            failure: None,
            summary: Some("planned breakdown".to_string()),
            details: None,
        };

        let value = serde_json::to_value(&result).expect("job result serializes");
        assert_eq!(value.get("repos"), None);
        assert_eq!(value["verdict"], "needs_breakdown");
        assert_eq!(value["children"][1]["target_repo"], "acme/web");
        let decoded: JobResult = serde_json::from_value(value).expect("serialized result parses");
        assert_eq!(decoded, result);
    }

    #[test]
    fn child_defaults_omit_empty_optional_fields() {
        let child = JobChild {
            slug: "api-schema".to_string(),
            title: "Define the API schema".to_string(),
            body: "Write the shared API schema.".to_string(),
            labels: Vec::new(),
            depends_on: Vec::new(),
            target_repo: None,
        };

        assert_eq!(
            serde_json::to_value(&child).expect("child serializes"),
            serde_json::json!({
                "slug": "api-schema",
                "title": "Define the API schema",
                "body": "Write the shared API schema."
            })
        );
    }
}
