use std::collections::BTreeSet;

use temper_worker_protocol::{
    Capability, Capacity, Heartbeat, HeartbeatState, JobHeartbeat, Poll, Register,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};
use thiserror::Error;

use crate::config::WorkerConfig;

/// Fatal worker-setup errors. Per-request transport/HTTP failures are no longer
/// errors here — the sans-IO shell turns them into completions the machine logs
/// and retries (see [`crate::worker_shell`]). What remains are the conditions
/// that prevent the worker from running at all.
#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("worker runtime handle unavailable; run_worker must be driven on an engine runtime")]
    RuntimeUnavailable,
}

pub fn register_message(config: &WorkerConfig) -> WorkerProtocolMessage {
    register_message_params(&crate::config::WorkerParams::from_config(config))
}

pub fn poll_message(config: &WorkerConfig, free_capacity: u32) -> WorkerProtocolMessage {
    poll_message_params(
        &crate::config::WorkerParams::from_config(config),
        free_capacity,
    )
}

pub fn heartbeat_message(
    config: &WorkerConfig,
    in_flight: &BTreeSet<String>,
    free_capacity: u32,
) -> WorkerProtocolMessage {
    heartbeat_message_params(
        &crate::config::WorkerParams::from_config(config),
        in_flight,
        free_capacity,
    )
}

pub fn register_message_params(params: &crate::config::WorkerParams) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Register(Register {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: params.worker_id.clone(),
        capabilities: params
            .capabilities
            .iter()
            .map(|capability| Capability {
                role: capability.role.clone(),
                repo: capability.repo.clone(),
            })
            .collect(),
        capacity: Capacity {
            max_concurrent_jobs: params.max_concurrent_jobs,
        },
        labels: None,
    })
}

pub fn poll_message_params(
    params: &crate::config::WorkerParams,
    free_capacity: u32,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Poll(Poll {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: params.worker_id.clone(),
        free_capacity,
        max_wait_ms: Some(duration_millis(params.poll_wait)),
    })
}

pub fn heartbeat_message_params(
    params: &crate::config::WorkerParams,
    in_flight: &BTreeSet<String>,
    free_capacity: u32,
) -> WorkerProtocolMessage {
    WorkerProtocolMessage::Heartbeat(Heartbeat {
        protocol_version: WORKER_PROTOCOL_VERSION,
        worker_id: params.worker_id.clone(),
        jobs: in_flight
            .iter()
            .map(|job_id| JobHeartbeat {
                job_id: job_id.clone(),
                state: HeartbeatState::Running,
                message: "running".to_string(),
            })
            .collect(),
        free_capacity: Some(free_capacity),
    })
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::config::CapabilitySpec;

    use super::*;

    fn config() -> WorkerConfig {
        WorkerConfig {
            daemon_url: "http://127.0.0.1:1234".to_string(),
            worker_id: "worker-1".to_string(),
            capabilities: vec![
                CapabilitySpec {
                    repo: "ai/temper".to_string(),
                    role: "coder".to_string(),
                },
                CapabilitySpec {
                    repo: "ai/smith".to_string(),
                    role: "engineer".to_string(),
                },
            ],
            max_concurrent_jobs: 2,
            poll_wait: Duration::from_millis(1_500),
            heartbeat_interval: Duration::from_millis(500),
            executor: crate::config::ExecutorSelection::Stub,
        }
    }

    #[test]
    fn protocol_message_helpers_fill_version_and_config_fields() {
        let config = config();

        match register_message(&config) {
            WorkerProtocolMessage::Register(register) => {
                assert_eq!(register.protocol_version, WORKER_PROTOCOL_VERSION);
                assert_eq!(register.worker_id, "worker-1");
                assert_eq!(register.capacity.max_concurrent_jobs, 2);
                assert_eq!(register.capabilities.len(), 2);
                assert_eq!(register.capabilities[0].repo, "ai/temper");
            }
            other => panic!("unexpected message: {other:?}"),
        }

        match poll_message(&config, 1) {
            WorkerProtocolMessage::Poll(poll) => {
                assert_eq!(poll.protocol_version, WORKER_PROTOCOL_VERSION);
                assert_eq!(poll.worker_id, "worker-1");
                assert_eq!(poll.free_capacity, 1);
                assert_eq!(poll.max_wait_ms, Some(1_500));
            }
            other => panic!("unexpected message: {other:?}"),
        }

        let in_flight = BTreeSet::from(["job-a".to_string(), "job-b".to_string()]);
        match heartbeat_message(&config, &in_flight, 0) {
            WorkerProtocolMessage::Heartbeat(heartbeat) => {
                assert_eq!(heartbeat.protocol_version, WORKER_PROTOCOL_VERSION);
                assert_eq!(heartbeat.worker_id, "worker-1");
                assert_eq!(heartbeat.free_capacity, Some(0));
                assert_eq!(heartbeat.jobs.len(), 2);
                assert_eq!(heartbeat.jobs[0].job_id, "job-a");
                assert_eq!(heartbeat.jobs[0].state, HeartbeatState::Running);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }
}
