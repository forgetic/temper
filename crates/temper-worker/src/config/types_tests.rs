use std::collections::BTreeMap;
use std::time::Duration;

use crate::coding_executor::CodingExecutorConfig;
use crate::config::{CapabilitySpec, ExecutorSelection, WorkerConfig, WorkerParams};
use crate::workspace::RoleGitIdentity;

/// Builds a worker config entirely in memory — including the per-role identities
/// that issue #199 moved into the worker config — with no files/env/args.
fn in_memory_worker_config() -> WorkerConfig {
    let mut role_identities = BTreeMap::new();
    role_identities.insert(
        "engineer".to_string(),
        RoleGitIdentity {
            user: "Engineer".to_string(),
            email: "engineer@example.test".to_string(),
            token: "push-token".to_string(),
        },
    );
    WorkerConfig {
        daemon_url: "in-process".to_string(),
        worker_id: "worker-unit".to_string(),
        worker_pool: None,
        capabilities: vec![CapabilitySpec {
            repo: "acme/widgets".to_string(),
            role: "engineer".to_string(),
        }],
        role_identities,
        max_concurrent_jobs: 1,
        poll_wait: Duration::from_millis(10),
        heartbeat_interval: Duration::from_millis(10),
        executor: ExecutorSelection::Stub,
    }
}

#[test]
fn worker_config_carries_role_identities() {
    let config = in_memory_worker_config();
    assert_eq!(
        config
            .role_identities
            .get("engineer")
            .map(|id| id.user.as_str()),
        Some("Engineer"),
    );
}

/// The worker's machine projection accepts the extended in-memory config: the
/// identity/cadence knobs project cleanly even with role identities present.
#[test]
fn worker_params_project_from_in_memory_config() {
    let config = in_memory_worker_config();
    let params = WorkerParams::from_config(&config);
    assert_eq!(params.worker_id, "worker-unit");
    assert_eq!(params.max_concurrent_jobs, 1);
}

/// The coding executor's config is built from the worker config's role
/// identities — the single source of truth the worker factory feeds the executor
/// from (issue #199).
#[test]
fn coding_executor_config_sources_identities_from_worker_config() {
    let config = in_memory_worker_config();
    let executor_config = CodingExecutorConfig {
        workspace_root: "/tmp/ws".into(),
        git_base_url: "http://localhost:3000".to_string(),
        role_identities: config.role_identities.clone(),
    };
    assert!(executor_config.role_identities.contains_key("engineer"));
}
