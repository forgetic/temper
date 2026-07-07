use serde_json::Value;
use temper_protocol_worker::{
    Assign, Branch, Failure, FailureClass, JobChild, JobResult, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION,
};

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutcome {
    Success {
        /// Per-repo head products — one per writable repo that produced a diff.
        /// The daemon opens one pull request per entry. A coding job that wrote
        /// to a single repo produces exactly one outcome.
        repos: Vec<RepoOutcome>,
        /// Optional agent-authored implementation PR title for no-verdict
        /// success results.
        title: Option<String>,
        /// Optional agent-authored implementation PR report body for no-verdict
        /// success results.
        body: Option<String>,
        summary: Option<String>,
        /// Optional structured metadata for daemon-side application.
        details: Option<Value>,
    },
    Verdict {
        verdict: String,
        /// Optional agent-authored title for verdict transitions that create a
        /// pull request from metadata instead of a pushed workspace head.
        title: Option<String>,
        body: Option<String>,
        summary: Option<String>,
        children: Vec<JobChild>,
    },
    Failure {
        class: FailureClass,
        message: String,
    },
}

pub trait JobExecutor {
    fn execute(&self, assign: Assign) -> impl std::future::Future<Output = JobOutcome> + Send;
}

#[derive(Clone, Debug, PartialEq)]
pub struct StubExecutor {
    mode: StubMode,
}

#[derive(Clone, Debug, PartialEq)]
enum StubMode {
    Success,
    Failure {
        class: FailureClass,
        message: String,
    },
}

impl StubExecutor {
    pub fn success() -> Self {
        Self {
            mode: StubMode::Success,
        }
    }

    pub fn failure(class: FailureClass, message: impl Into<String>) -> Self {
        Self {
            mode: StubMode::Failure {
                class,
                message: message.into(),
            },
        }
    }
}

impl JobExecutor for StubExecutor {
    fn execute(&self, assign: Assign) -> impl std::future::Future<Output = JobOutcome> + Send {
        let mode = self.mode.clone();
        async move {
            match mode {
                StubMode::Success => JobOutcome::Success {
                    repos: vec![RepoOutcome {
                        repo: assign.repo.clone(),
                        branch: Branch {
                            name: format!("temper-worker/stub/{}", assign.job_id),
                            head_sha: "0000000000000000000000000000000000000000".to_string(),
                        },
                    }],
                    title: None,
                    body: None,
                    summary: Some("stub executor completed without doing IO".to_string()),
                    details: None,
                },
                StubMode::Failure { class, message } => JobOutcome::Failure { class, message },
            }
        }
    }
}

pub fn job_result(worker_id: &str, job_id: &str, outcome: JobOutcome) -> JobResult {
    let base = JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: Vec::new(),
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: None,
        details: None,
    };
    match outcome {
        JobOutcome::Success {
            repos,
            title,
            body,
            summary,
            details,
        } => JobResult {
            status: ResultStatus::Success,
            repos,
            title,
            body,
            summary,
            details,
            ..base
        },
        JobOutcome::Verdict {
            verdict,
            title,
            body,
            summary,
            children,
        } => JobResult {
            status: ResultStatus::Success,
            verdict: Some(verdict),
            title,
            body,
            children,
            summary,
            ..base
        },
        JobOutcome::Failure { class, message } => JobResult {
            status: ResultStatus::Failure,
            failure: Some(Failure { class, message }),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use temper_protocol_worker::Artifact;

    use super::*;

    fn assign(job_id: &str) -> Assign {
        Assign {
            protocol_version: WORKER_PROTOCOL_VERSION,
            job_id: job_id.to_string(),
            role: "coder".to_string(),
            repo: "ai/temper".to_string(),
            artifact: Artifact {
                item: json!(78),
                kind: "issue".to_string(),
            },
            job_payload: json!({}),
        }
    }

    #[test]
    fn success_stub_maps_to_success_result_with_branch() {
        temper_worker_io::block_on(async {
            let outcome = StubExecutor::success().execute(assign("job-123")).await;
            let result = job_result("worker-1", "job-123", outcome);

            assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
            assert_eq!(result.worker_id, "worker-1");
            assert_eq!(result.job_id, "job-123");
            assert_eq!(result.status, ResultStatus::Success);
            assert_eq!(result.failure, None);
            assert_eq!(result.details, None);
            assert_eq!(
                result.summary.as_deref(),
                Some("stub executor completed without doing IO")
            );
            assert_eq!(
                result.repos,
                vec![RepoOutcome {
                    repo: "ai/temper".to_string(),
                    branch: Branch {
                        name: "temper-worker/stub/job-123".to_string(),
                        head_sha: "0000000000000000000000000000000000000000".to_string(),
                    },
                }]
            );
        });
    }

    #[test]
    fn success_outcome_maps_structured_details_to_result() {
        let details = json!({"extra":{"note":"worker metadata"}});
        let result = job_result(
            "worker-1",
            "job-123",
            JobOutcome::Success {
                repos: Vec::new(),
                title: None,
                body: None,
                summary: Some("implemented".to_string()),
                details: Some(details.clone()),
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.summary.as_deref(), Some("implemented"));
        assert_eq!(result.details, Some(details));
    }

    #[test]
    fn success_outcome_maps_handoff_title_and_body_to_result() {
        let result = job_result(
            "worker-1",
            "job-123",
            JobOutcome::Success {
                repos: Vec::new(),
                title: Some("Implement agent-authored handoff".to_string()),
                body: Some("# Implementation report\n\nDone.".to_string()),
                summary: Some("implemented".to_string()),
                details: None,
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(
            result.title.as_deref(),
            Some("Implement agent-authored handoff")
        );
        assert_eq!(
            result.body.as_deref(),
            Some("# Implementation report\n\nDone.")
        );
        assert_eq!(result.verdict, None);
    }

    #[test]
    fn verdict_outcome_maps_to_success_result_without_branch() {
        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "ready_code".to_string(),
                title: None,
                body: Some("rewritten issue body".to_string()),
                summary: Some("triaged".to_string()),
                children: Vec::new(),
            },
        );

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-3");
        assert_eq!(result.job_id, "job-789");
        assert_eq!(result.status, ResultStatus::Success);
        assert!(result.repos.is_empty());
        assert_eq!(result.failure, None);
        assert_eq!(result.summary.as_deref(), Some("triaged"));
        assert!(result.children.is_empty());

        let serialized = serde_json::to_value(&result).expect("JobResult serializes");
        assert_eq!(serialized["verdict"], "ready_code");
        assert_eq!(serialized["body"], "rewritten issue body");
        assert!(
            serialized.get("children").is_none(),
            "empty children must stay wire-compatible: {serialized}"
        );
    }

    #[test]
    fn verdict_outcome_preserves_authored_title_for_metadata_pr_create() {
        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "passed".to_string(),
                title: Some("Land validated feature branch".to_string()),
                body: Some("# Validation report".to_string()),
                summary: Some("validated".to_string()),
                children: Vec::new(),
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.verdict.as_deref(), Some("passed"));
        assert_eq!(
            result.title.as_deref(),
            Some("Land validated feature branch")
        );
    }

    #[test]
    fn verdict_outcome_maps_children_to_success_result() {
        let children = vec![
            JobChild {
                slug: "api-schema".to_string(),
                title: "Define the API schema".to_string(),
                body: "Write the shared API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: Vec::new(),
                target_repo: Some("acme/api".to_string()),
            },
            JobChild {
                slug: "web-client".to_string(),
                title: "Implement the web client".to_string(),
                body: "Build the web client against the API schema.".to_string(),
                kind: None,
                labels: vec!["code".to_string()],
                depends_on: vec!["api-schema".to_string()],
                target_repo: None,
            },
        ];

        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "needs_breakdown".to_string(),
                title: None,
                body: None,
                summary: Some("planned breakdown".to_string()),
                children: children.clone(),
            },
        );

        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.children, children);

        let serialized = serde_json::to_value(&result).expect("JobResult serializes");
        assert_eq!(serialized["verdict"], "needs_breakdown");
        assert_eq!(serialized["children"][0]["slug"], "api-schema");
        assert_eq!(serialized["children"][0]["title"], "Define the API schema");
        assert_eq!(
            serialized["children"][0]["body"],
            "Write the shared API schema."
        );
        assert_eq!(
            serialized["children"][0]["labels"],
            json!(["code", "ready"])
        );
        assert_eq!(serialized["children"][0]["target_repo"], "acme/api");
        assert_eq!(serialized["children"][1]["slug"], "web-client");
        assert_eq!(
            serialized["children"][1]["depends_on"],
            json!(["api-schema"])
        );
        assert_eq!(serialized["children"][1]["labels"], json!(["code"]));
        assert!(serialized["children"][1].get("target_repo").is_none());
    }

    #[test]
    fn failure_stub_maps_to_failure_result_without_branch() {
        temper_worker_io::block_on(async {
            let outcome = StubExecutor::failure(FailureClass::Permanent, "configured failure")
                .execute(assign("job-456"))
                .await;
            let result = job_result("worker-2", "job-456", outcome);

            assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
            assert_eq!(result.worker_id, "worker-2");
            assert_eq!(result.job_id, "job-456");
            assert_eq!(result.status, ResultStatus::Failure);
            assert!(result.repos.is_empty());
            assert_eq!(result.summary, None);
            assert_eq!(
                result.failure,
                Some(Failure {
                    class: FailureClass::Permanent,
                    message: "configured failure".to_string(),
                })
            );
        });
    }
}
