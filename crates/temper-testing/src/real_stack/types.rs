use std::path::PathBuf;

use jig_core::{Reply, Script, StopReason, Turn};
use serde_json::json;

use super::DEFAULT_WORKER_ID;

/// Repository seed for [`crate::real_stack::HermeticRealStackBuilder`].
#[derive(Clone, Debug)]
pub struct HermeticRepoSpec {
    pub owner: String,
    pub name: String,
    pub default_branch: String,
    pub(crate) seed_files: Vec<(PathBuf, String)>,
}

impl HermeticRepoSpec {
    /// Creates an `owner/name` repo with default branch `main`.
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
            default_branch: "main".to_string(),
            seed_files: Vec::new(),
        }
    }

    /// Overrides the default branch seeded in both MemoryForge and the local git
    /// origin.
    #[must_use]
    pub fn default_branch(mut self, branch: impl Into<String>) -> Self {
        self.default_branch = branch.into();
        self
    }

    /// Adds a file to the initial git commit. Paths must be relative, normal
    /// repository paths; validation happens during builder `build`.
    #[must_use]
    pub fn seed_file(mut self, path: impl Into<PathBuf>, contents: impl Into<String>) -> Self {
        self.seed_files.push((path.into(), contents.into()));
        self
    }

    /// The protocol/Forge path (`owner/name`).
    pub fn path(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// Issue seed for [`crate::real_stack::HermeticRealStackBuilder`].
#[derive(Clone, Debug)]
pub struct HermeticIssueSpec {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

impl HermeticIssueSpec {
    /// Creates a ready code issue (`code`, `ready`) that the reference workflow
    /// scans into the engineer `code_ready` queue.
    pub fn ready_code(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            labels: vec!["code".to_string(), "ready".to_string()],
        }
    }

    /// Creates an untriaged intake issue for the basic-delivery architect queue.
    pub fn untriaged_intake(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            labels: vec!["untriaged".to_string()],
        }
    }

    /// Replaces the issue labels.
    #[must_use]
    pub fn labels(mut self, labels: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }

    /// Appends one label.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }
}

/// Git identity and capacity for the worker role under test.
#[derive(Clone, Debug)]
pub struct WorkerRoleSpec {
    pub role: String,
    pub worker_id: String,
    pub git_user: String,
    pub git_email: String,
    pub git_token: String,
    pub max_concurrent_jobs: u32,
}

impl WorkerRoleSpec {
    /// Default architect identity used by read-only triage tests.
    pub fn architect() -> Self {
        Self {
            role: "architect".to_string(),
            worker_id: DEFAULT_WORKER_ID.to_string(),
            git_user: "Hermetic Architect".to_string(),
            git_email: "architect@example.test".to_string(),
            git_token: "test-token".to_string(),
            max_concurrent_jobs: 1,
        }
    }

    /// Default engineer identity used by `code_ready` smoke tests.
    pub fn engineer() -> Self {
        Self {
            role: "engineer".to_string(),
            worker_id: DEFAULT_WORKER_ID.to_string(),
            git_user: "Hermetic Engineer".to_string(),
            git_email: "engineer@example.test".to_string(),
            git_token: "test-token".to_string(),
            max_concurrent_jobs: 1,
        }
    }

    /// Overrides the worker id that registers with the daemon.
    #[must_use]
    pub fn worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.worker_id = worker_id.into();
        self
    }

    /// Overrides the git identity persisted into worker-owned checkouts.
    #[must_use]
    pub fn git_identity(
        mut self,
        user: impl Into<String>,
        email: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        self.git_user = user.into();
        self.git_email = email.into();
        self.git_token = token.into();
        self
    }

    /// Overrides the worker capacity.
    #[must_use]
    pub fn max_concurrent_jobs(mut self, max_concurrent_jobs: u32) -> Self {
        self.max_concurrent_jobs = max_concurrent_jobs;
        self
    }
}

/// One file edit that a Jig-backed fake model should ask the native agent to
/// perform through the real workspace tools.
#[derive(Clone, Debug)]
pub struct FakeModelWrite {
    pub path: String,
    pub content: String,
}

impl FakeModelWrite {
    pub fn new(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content: content.into(),
        }
    }
}

/// Typed canned model behavior for the common “write product files, then return
/// a successful workspace result” path.
#[derive(Clone, Debug)]
pub struct FakeModelResponse {
    pub writes: Vec<FakeModelWrite>,
    pub summary: String,
}

impl FakeModelResponse {
    /// A single product-file write followed by a summary-only success result.
    pub fn write_file(
        path: impl Into<String>,
        content: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            writes: vec![FakeModelWrite::new(path, content)],
            summary: summary.into(),
        }
    }

    /// Several product-file writes followed by a summary-only success result.
    pub fn write_files(
        writes: impl IntoIterator<Item = FakeModelWrite>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            writes: writes.into_iter().collect(),
            summary: summary.into(),
        }
    }

    pub(crate) fn into_script(self) -> Script {
        let writes = self.writes;
        let result_json = json!({ "summary": self.summary }).to_string();
        Script::rule(move |view| {
            if view.prior_tool_results == 0 && !writes.is_empty() {
                Reply {
                    turns: writes
                        .iter()
                        .enumerate()
                        .map(|(index, write)| Turn::ToolCall {
                            id: format!("call_write_{index}"),
                            name: "write".to_string(),
                            args: json!({
                                "path": write.path,
                                "content": write.content,
                            }),
                        })
                        .collect(),
                    usage: Default::default(),
                    stop: StopReason::ToolCalls,
                }
            } else {
                Reply::text(result_json.clone())
            }
        })
    }
}

pub(crate) enum FakeModelSetup {
    Response(FakeModelResponse),
    Script(Script),
}
