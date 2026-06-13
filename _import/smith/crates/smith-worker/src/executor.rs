use temper_worker_protocol::{
    Assign, Branch, Failure, FailureClass, JobChild, JobResult, ResultStatus,
    WORKER_PROTOCOL_VERSION,
};

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutcome {
    Success {
        branch: Branch,
        summary: Option<String>,
    },
    Verdict {
        verdict: String,
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
                    branch: Branch {
                        name: format!("smith-worker/stub/{}", assign.job_id),
                        head_sha: "0000000000000000000000000000000000000000".to_string(),
                    },
                    summary: Some("stub executor completed without doing IO".to_string()),
                },
                StubMode::Failure { class, message } => JobOutcome::Failure { class, message },
            }
        }
    }
}

pub fn job_result(worker_id: &str, job_id: &str, outcome: JobOutcome) -> JobResult {
    match outcome {
        JobOutcome::Success { branch, summary } => job_result_from_value(serde_json::json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": worker_id,
            "job_id": job_id,
            "status": ResultStatus::Success,
            "branch": branch,
            "failure": null,
            "summary": summary,
            "details": null,
            "verdict": null,
            "body": null,
        })),
        JobOutcome::Verdict {
            verdict,
            body,
            summary,
            children,
        } => job_result_from_value(serde_json::json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": worker_id,
            "job_id": job_id,
            "status": ResultStatus::Success,
            "branch": null,
            "failure": null,
            "summary": summary,
            "details": null,
            "verdict": verdict,
            "body": body,
            "children": children,
        })),
        JobOutcome::Failure { class, message } => job_result_from_value(serde_json::json!({
            "protocol_version": WORKER_PROTOCOL_VERSION,
            "worker_id": worker_id,
            "job_id": job_id,
            "status": ResultStatus::Failure,
            "branch": null,
            "failure": Failure { class, message },
            "summary": null,
            "details": null,
            "verdict": null,
            "body": null,
        })),
    }
}

fn job_result_from_value(value: serde_json::Value) -> JobResult {
    serde_json::from_value(value).expect("smith-worker constructs valid worker-protocol JobResult")
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use temper_worker_protocol::Artifact;

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

    #[tokio::test]
    async fn success_stub_maps_to_success_result_with_branch() {
        let outcome = StubExecutor::success().execute(assign("job-123")).await;
        let result = job_result("worker-1", "job-123", outcome);

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-1");
        assert_eq!(result.job_id, "job-123");
        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.failure, None);
        assert_eq!(
            result.summary.as_deref(),
            Some("stub executor completed without doing IO")
        );
        assert_eq!(
            result.branch,
            Some(Branch {
                name: "smith-worker/stub/job-123".to_string(),
                head_sha: "0000000000000000000000000000000000000000".to_string(),
            })
        );
    }

    #[test]
    fn verdict_outcome_maps_to_success_result_without_branch() {
        let result = job_result(
            "worker-3",
            "job-789",
            JobOutcome::Verdict {
                verdict: "ready_code".to_string(),
                body: Some("rewritten issue body".to_string()),
                summary: Some("triaged".to_string()),
                children: Vec::new(),
            },
        );

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-3");
        assert_eq!(result.job_id, "job-789");
        assert_eq!(result.status, ResultStatus::Success);
        assert_eq!(result.branch, None);
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
    fn verdict_outcome_maps_children_to_success_result() {
        let children = vec![
            JobChild {
                slug: "api-schema".to_string(),
                title: "Define the API schema".to_string(),
                body: "Write the shared API schema.".to_string(),
                labels: vec!["code".to_string(), "ready".to_string()],
                depends_on: Vec::new(),
                target_repo: Some("acme/api".to_string()),
            },
            JobChild {
                slug: "web-client".to_string(),
                title: "Implement the web client".to_string(),
                body: "Build the web client against the API schema.".to_string(),
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

    #[tokio::test]
    async fn failure_stub_maps_to_failure_result_without_branch() {
        let outcome = StubExecutor::failure(FailureClass::Permanent, "configured failure")
            .execute(assign("job-456"))
            .await;
        let result = job_result("worker-2", "job-456", outcome);

        assert_eq!(result.protocol_version, WORKER_PROTOCOL_VERSION);
        assert_eq!(result.worker_id, "worker-2");
        assert_eq!(result.job_id, "job-456");
        assert_eq!(result.status, ResultStatus::Failure);
        assert_eq!(result.branch, None);
        assert_eq!(result.summary, None);
        assert_eq!(
            result.failure,
            Some(Failure {
                class: FailureClass::Permanent,
                message: "configured failure".to_string(),
            })
        );
    }
}
