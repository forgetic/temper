// SPDX-License-Identifier: MPL-2.0

//! Worker simulants: in-sim tasks speaking the real worker protocol over the
//! production h1 byte path, with seeded misbehavior profiles.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use skein::cx::Cx;
use temper_protocol_worker::{
    Branch, Capability, Capacity, ErrorCode, JobResult, Poll, Register, RepoOutcome, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

use crate::SimProtocolClient;
use crate::model::SimModel;

/// How a simulated worker behaves.
#[derive(Clone, Debug)]
pub struct WorkerProfile {
    pub worker_id: String,
    pub role: String,
    pub repo: String,
    /// Virtual time spent "executing" each assignment.
    pub work_time: Duration,
    /// Long-poll wait requested from the daemon.
    pub poll_wait_ms: u64,
    /// Consecutive empty polls (timeouts) before the worker exits.
    pub max_empty_polls: u32,
    /// Consecutive transport failures tolerated before the worker exits.
    pub max_transport_failures: u32,
    /// Submit every result twice (duplicate-delivery misbehavior).
    pub duplicate_results: bool,
    /// Crash (stop participating) after taking this many assignments,
    /// without ever reporting the last one.
    pub crash_after_assignments: Option<usize>,
}

impl WorkerProfile {
    pub fn normal(worker_id: impl Into<String>, role: &str, repo: &str) -> Self {
        Self {
            worker_id: worker_id.into(),
            role: role.to_string(),
            repo: repo.to_string(),
            work_time: Duration::from_millis(20),
            poll_wait_ms: 50,
            max_empty_polls: 3,
            max_transport_failures: 5,
            duplicate_results: false,
            crash_after_assignments: None,
        }
    }
}

/// Increments a shared counter when dropped — fires on normal exit *and* on
/// chaos cancellation (the future is dropped either way), so `run_until` can
/// wait for "all workers gone" under any schedule.
pub struct DoneGuard(Arc<AtomicUsize>);

impl DoneGuard {
    pub fn new(counter: Arc<AtomicUsize>) -> Self {
        Self(counter)
    }
}

impl Drop for DoneGuard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn register_message(profile: &WorkerProfile) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: profile.worker_id.clone(),
        capabilities: vec![Capability {
            role: profile.role.clone(),
            repo: profile.repo.clone(),
        }],
        capacity: Capacity {
            max_concurrent_jobs: 1,
        },
        labels: None,
    })
}

fn poll_message(profile: &WorkerProfile) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: profile.worker_id.clone(),
        free_capacity: 1,
        max_wait_ms: Some(profile.poll_wait_ms),
    })
}

fn result_message(profile: &WorkerProfile, job_id: &str) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Result(JobResult {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: profile.worker_id.clone(),
        job_id: job_id.to_string(),
        status: ResultStatus::Success,
        repos: vec![RepoOutcome {
            repo: profile.repo.clone(),
            branch: Branch {
                name: format!("agent/{job_id}"),
                head_sha: "feedc0de".to_string(),
            },
        }],
        verdict: None,
        title: None,
        body: None,
        children: Vec::new(),
        failure: None,
        summary: Some(json!({"worker": profile.worker_id}).to_string()),
        details: None,
    })
}

/// Run one simulated worker to completion: register, then poll/work/report
/// until the queue stays empty, transport keeps failing, or the profile says
/// to crash. Every observation is recorded in the model.
pub async fn run_worker(
    cx: Cx,
    client: SimProtocolClient,
    model: SimModel,
    profile: WorkerProfile,
    _done: DoneGuard,
) {
    // Registration failures count as transport trouble, not a crash.
    let mut transport_failures = 0u32;
    while client.try_send(&register_message(&profile)).await.is_err() {
        transport_failures += 1;
        if transport_failures > profile.max_transport_failures {
            return;
        }
    }

    let mut empty_polls = 0u32;
    let mut assignments_taken = 0usize;
    loop {
        match client.try_send(&poll_message(&profile)).await {
            Ok(Some(WorkerProtocolMessage::Assign(assign))) => {
                empty_polls = 0;
                transport_failures = 0;
                assignments_taken += 1;
                model.record_assign(&assign.job_id, &profile.worker_id);

                if profile
                    .crash_after_assignments
                    .is_some_and(|limit| assignments_taken >= limit)
                {
                    // Take the job and vanish: never report, never poll again.
                    return;
                }

                if !profile.work_time.is_zero() {
                    skein::time::sleep(
                        temper_engine_io::runtime::timer_now(&cx),
                        profile.work_time,
                    )
                    .await;
                }

                let result = result_message(&profile, &assign.job_id);
                let submissions = if profile.duplicate_results { 2 } else { 1 };
                for _ in 0..submissions {
                    if let Ok(reply) = client.try_send(&result).await {
                        if let Some(WorkerProtocolMessage::Release(release)) = reply {
                            if release.job_id == assign.job_id {
                                model.record_release(&assign.job_id);
                            }
                        }
                    }
                }
            }
            Ok(Some(WorkerProtocolMessage::Error(error)))
                if error.code == ErrorCode::PollTimeout =>
            {
                empty_polls += 1;
                if empty_polls >= profile.max_empty_polls {
                    return;
                }
            }
            Ok(_) => {
                // Unexpected but well-formed reply; keep going.
                empty_polls += 1;
                if empty_polls >= profile.max_empty_polls {
                    return;
                }
            }
            Err(_) => {
                transport_failures += 1;
                if transport_failures > profile.max_transport_failures {
                    return;
                }
            }
        }
    }
}
