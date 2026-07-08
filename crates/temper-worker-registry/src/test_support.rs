// SPDX-License-Identifier: MPL-2.0

use serde_json::json;
use temper_protocol_worker::{
    Artifact, Capability, Capacity, ErrorCode, Heartbeat, JobResult, Poll, Register, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::WorkItem;

pub(crate) fn register(
    worker_id: &str,
    role: &str,
    repo: &str,
    max_concurrent_jobs: u32,
) -> Register {
    Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: vec![Capability {
            role: role.to_string(),
            repo: repo.to_string(),
        }],
        capacity: Capacity {
            max_concurrent_jobs,
        },
        worker_pool: None,
        labels: None,
    }
}

pub(crate) fn register_multi(
    worker_id: &str,
    role: &str,
    repos: &[&str],
    max_concurrent_jobs: u32,
) -> Register {
    Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        capabilities: repos
            .iter()
            .map(|repo| Capability {
                role: role.to_string(),
                repo: (*repo).to_string(),
            })
            .collect(),
        capacity: Capacity {
            max_concurrent_jobs,
        },
        worker_pool: None,
        labels: None,
    }
}

pub(crate) fn work(job_id: &str, role: &str, repo: &str) -> WorkItem {
    WorkItem::single(job_id, role, repo)
}

pub(crate) fn coordinated(job_id: &str, role: &str, repos: &[&str]) -> WorkItem {
    WorkItem {
        job_id: job_id.to_string(),
        role: role.to_string(),
        coordination_key: None,
        repo: repos[0].to_string(),
        repos: repos.iter().map(|repo| (*repo).to_string()).collect(),
    }
}

pub(crate) fn coordinated_workstream(
    job_id: &str,
    role: &str,
    repo: &str,
    coordination_key: &str,
) -> WorkItem {
    WorkItem::single(job_id, role, repo).with_coordination_key(coordination_key)
}

pub(crate) fn poll(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        free_capacity: 1,
        max_wait_ms: Some(30_000),
    })
}

pub(crate) fn heartbeat(worker_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Heartbeat(Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: worker_id.to_string(),
        jobs: Vec::new(),
        free_capacity: Some(1),
        worker_pool: None,
        max_concurrent_jobs: None,
        capabilities: Vec::new(),
    })
}

pub(crate) fn artifact() -> Artifact {
    Artifact {
        item: json!(99),
        kind: "issue".to_string(),
    }
}

pub(crate) fn result(worker_id: &str, job_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Result(JobResult {
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
        summary: Some("done".to_string()),
        details: None,
    })
}

pub(crate) fn assert_error(msg: Option<WorkerProtocolMessage>, code: ErrorCode, message: &str) {
    match msg {
        Some(WorkerProtocolMessage::Error(error)) => {
            assert_eq!(error.code, code);
            assert_eq!(error.message, message);
        }
        other => panic!("expected protocol error, got {other:?}"),
    }
}

pub(crate) fn coordinated_payload(coordination_key: &str, repos: &[&str]) -> serde_json::Value {
    let repos = repos
        .iter()
        .map(|repo| {
            json!({
                "repo": repo,
                "dir": repo.split('/').next_back().unwrap(),
                "access": "writable",
                "default_branch": "main",
                "base_branch": "main",
                "branch_hint": format!("agent/{coordination_key}"),
            })
        })
        .collect::<Vec<_>>();
    json!({ "workspace": { "coordination_key": coordination_key, "repos": repos } })
}
