// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;
use std::path::PathBuf;

use temper_config::{EnvMap, PathResolver, Resolved};
use temper_protocol_worker::WorkerProtocolMessage;
use temper_worker_registry::WorkerRegistry;

use super::scratch;
use crate::{DaemonInputs, RuntimeOverrides, Service, apply_runtime_overrides, load_for};

fn resolved_from_config(tag: &str, config_text: &str) -> (PathBuf, Resolved) {
    let dir = scratch(tag);
    let config = dir.join("config.toml");
    std::fs::write(&config, config_text).expect("write config");

    let env = EnvMap::new();
    let paths = PathResolver::default();
    let inputs = DaemonInputs {
        config: Some(config),
        credentials: None,
        service: None,
        runtime: RuntimeOverrides::default(),
        env: &env,
        paths: &paths,
    };
    let (resolved, _) = load_for(&inputs).expect("config resolves");
    (dir, resolved)
}

fn base_config() -> &'static str {
    "schema_version = 1\n\
     [engine]\n\
     repos = [\"ai/temper\"]\n\
     roles = [\"engineer\"]\n"
}

fn worker_pool_config() -> &'static str {
    "schema_version = 1\n\
     [engine]\n\
     repos = [\"legacy/repo\"]\n\
     roles = [\"legacy\"]\n\
     [worker]\n\
     max_concurrent_jobs = 7\n\
     [[worker.pools]]\n\
     name = \"builders\"\n\
     roles = [\"engineer\", \"reviewer\"]\n\
     repos = [\"ai/temper\", \"acme/widgets\"]\n\
     max_concurrent_jobs = 2\n\
     [[worker.pools]]\n\
     name = \"broad\"\n\
     roles = [\"legacy\", \"engineer\", \"admin\"]\n\
     repos = [\"legacy/repo\", \"ai/temper\", \"acme/widgets\"]\n\
     max_concurrent_jobs = 9\n"
}

#[test]
fn standalone_id_overrides_engine_and_in_process_worker_id() {
    let (dir, mut resolved) = resolved_from_config("standalone-id", base_config());

    apply_runtime_overrides(
        &mut resolved,
        None,
        &RuntimeOverrides {
            process_id: Some("node-a".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect("standalone id applies");

    assert_eq!(resolved.engine.daemon_id, "node-a");
    assert_eq!(resolved.worker.worker_id, "node-a");
    assert_eq!(
        temper_engine_service::daemon_run_config(&resolved)
            .expect("engine runtime config builds")
            .daemon_id,
        "node-a"
    );
    assert_eq!(
        temper_worker_service::worker_config(&resolved)
            .expect("worker runtime config builds")
            .worker_id,
        "node-a"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn engine_id_overrides_engine_runtime_identity_only() {
    let (dir, mut resolved) = resolved_from_config("engine-id", base_config());

    apply_runtime_overrides(
        &mut resolved,
        Some(Service::Engine),
        &RuntimeOverrides {
            process_id: Some("engine-a".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect("engine id applies");

    assert_eq!(resolved.engine.daemon_id, "engine-a");
    assert_eq!(resolved.worker.worker_id, "temper-worker-1");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_flags_override_worker_runtime_config() {
    let (dir, mut resolved) = resolved_from_config("worker-overrides", base_config());

    apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            process_id: Some("worker-a".to_string()),
            worker_capacity: Some(4),
            worker_engine_url: Some("http://engine.local:9000".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect("worker overrides apply");

    let worker_config =
        temper_worker_service::worker_config(&resolved).expect("worker runtime config builds");
    assert_eq!(worker_config.worker_id, "worker-a");
    assert_eq!(worker_config.daemon_url, "http://engine.local:9000");
    assert_eq!(worker_config.max_concurrent_jobs, 4);
    assert_eq!(resolved.engine.daemon_id, "temper-daemon-1");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_pool_derives_capabilities_and_default_capacity() {
    let (dir, mut resolved) = resolved_from_config("worker-pool", worker_pool_config());
    let pool_policies = temper_engine_service::daemon_run_config(&resolved)
        .expect("engine runtime config builds")
        .worker_pools;
    assert_eq!(pool_policies.len(), 2);
    assert_eq!(pool_policies[0].name, "builders");
    assert_eq!(pool_policies[0].max_concurrent_jobs, Some(2));
    assert_eq!(pool_policies[1].name, "broad");

    apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("builders".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect("pool applies");

    let worker_config =
        temper_worker_service::worker_config(&resolved).expect("worker runtime config builds");
    assert_eq!(worker_config.worker_pool.as_deref(), Some("builders"));
    assert_eq!(worker_config.max_concurrent_jobs, 2);
    let capabilities: BTreeSet<(String, String)> = worker_config
        .capabilities
        .iter()
        .map(|capability| (capability.repo.clone(), capability.role.clone()))
        .collect();
    assert_eq!(
        capabilities,
        BTreeSet::from([
            ("acme/widgets".to_string(), "engineer".to_string()),
            ("acme/widgets".to_string(), "reviewer".to_string()),
            ("ai/temper".to_string(), "engineer".to_string()),
            ("ai/temper".to_string(), "reviewer".to_string()),
        ])
    );

    let register = match temper_worker::client::register_message(&worker_config) {
        WorkerProtocolMessage::Register(register) => register,
        other => panic!("expected register message, got {other:?}"),
    };
    assert_eq!(register.worker_pool.as_deref(), Some("builders"));
    assert_eq!(register.labels, Some(vec!["pool:builders".to_string()]));
    let mut registry = WorkerRegistry::new();
    registry.register(&register);
    assert_eq!(
        registry.assign_candidate("engineer", "ai/temper"),
        Some("temper-worker-1".to_string())
    );
    assert_eq!(registry.assign_candidate("legacy", "legacy/repo"), None);
    assert_eq!(registry.assign_candidate("admin", "acme/widgets"), None);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_with_pools_requires_explicit_pool_selection() {
    let (dir, mut resolved) = resolved_from_config("worker-pool-required", worker_pool_config());

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides::default(),
    )
    .expect_err("worker with pools should require --pool");

    assert!(error.contains("worker pools are configured"), "{error}");
    assert!(error.contains("--pool"), "{error}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn capacity_override_may_lower_pool_default_capacity() {
    let (dir, mut resolved) = resolved_from_config("worker-pool-capacity", worker_pool_config());

    apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("builders".to_string()),
            worker_capacity: Some(1),
            ..RuntimeOverrides::default()
        },
    )
    .expect("pool and capacity apply");

    assert_eq!(
        temper_worker_service::worker_config(&resolved)
            .expect("worker runtime config builds")
            .max_concurrent_jobs,
        1
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn capacity_override_above_pool_policy_fails() {
    let (dir, mut resolved) =
        resolved_from_config("worker-pool-capacity-high", worker_pool_config());

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("builders".to_string()),
            worker_capacity: Some(3),
            ..RuntimeOverrides::default()
        },
    )
    .expect_err("capacity above pool policy should fail");

    assert!(error.contains("--capacity 3"), "{error}");
    assert!(error.contains("builders"), "{error}");
    assert!(error.contains("max_concurrent_jobs 2"), "{error}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn unknown_worker_pool_name_fails_clearly() {
    let (dir, mut resolved) = resolved_from_config("worker-pool-unknown", worker_pool_config());

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("missing-pool".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect_err("unknown pool should fail");

    assert!(error.contains("missing-pool"), "{error}");
    assert!(error.contains("unknown worker pool"), "{error}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_pool_without_repos_fails_instead_of_using_legacy_capabilities() {
    let (dir, mut resolved) = resolved_from_config(
        "worker-pool-empty",
        "schema_version = 1\n\
         [engine]\n\
         repos = [\"legacy/repo\"]\n\
         roles = [\"legacy\"]\n\
         [[worker.pools]]\n\
         name = \"empty\"\n\
         roles = [\"engineer\"]\n\
         max_concurrent_jobs = 1\n",
    );

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("empty".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect_err("empty pool should fail");

    assert!(error.contains("empty"), "{error}");
    assert!(
        error.contains("cannot produce runtime capabilities"),
        "{error}"
    );
    assert_eq!(resolved.worker.capabilities[0].repo, "legacy/repo");
    assert_eq!(resolved.worker.capabilities[0].role, "legacy");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn standalone_with_target_pools_uses_single_configured_pool() {
    let (dir, mut resolved) = resolved_from_config(
        "standalone-pool",
        "schema_version = 1\n\
         [engine]\n\
         repos = [\"legacy/repo\", \"ai/temper\"]\n\
         roles = [\"legacy\", \"engineer\"]\n\
         [worker]\n\
         capabilities = [\"legacy/repo:legacy\", \"ai/temper:engineer\"]\n\
         [[worker.pools]]\n\
         name = \"local\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         max_concurrent_jobs = 1\n",
    );

    apply_runtime_overrides(&mut resolved, None, &RuntimeOverrides::default())
        .expect("standalone selects local pool");

    let worker_config =
        temper_worker_service::worker_config(&resolved).expect("worker runtime config builds");
    assert_eq!(worker_config.worker_pool.as_deref(), Some("local"));
    assert_eq!(worker_config.max_concurrent_jobs, 1);
    assert_eq!(worker_config.capabilities.len(), 1);
    assert_eq!(worker_config.capabilities[0].repo, "ai/temper");
    assert_eq!(worker_config.capabilities[0].role, "engineer");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn selected_pool_without_capacity_policy_fails() {
    let (dir, mut resolved) = resolved_from_config(
        "worker-pool-no-capacity",
        "schema_version = 1\n\
         [engine]\n\
         repos = [\"legacy/repo\"]\n\
         roles = [\"legacy\"]\n\
         [[worker.pools]]\n\
         name = \"no-policy\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n",
    );

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("no-policy".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect_err("pool without capacity policy should fail");

    assert!(error.contains("no-policy"), "{error}");
    assert!(error.contains("max_concurrent_jobs"), "{error}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn selected_pool_with_missing_agent_profile_fails() {
    let (dir, mut resolved) = resolved_from_config(
        "worker-pool-missing-profile",
        "schema_version = 1\n\
         [engine]\n\
         repos = [\"legacy/repo\"]\n\
         roles = [\"legacy\"]\n\
         [[worker.pools]]\n\
         name = \"profiled\"\n\
         roles = [\"engineer\"]\n\
         repos = [\"ai/temper\"]\n\
         max_concurrent_jobs = 1\n\
         agent_profile = \"missing\"\n",
    );

    let error = apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides {
            worker_pool: Some("profiled".to_string()),
            ..RuntimeOverrides::default()
        },
    )
    .expect_err("missing pool profile should fail");

    assert!(error.contains("profiled"), "{error}");
    assert!(error.contains("missing agent profile `missing`"), "{error}");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn worker_without_pool_preserves_legacy_capabilities() {
    let (dir, mut resolved) = resolved_from_config(
        "worker-legacy-capabilities",
        "schema_version = 1\n\
         [engine]\n\
         repos = [\"ai/temper\"]\n\
         roles = [\"engineer\"]\n\
         [worker]\n\
         capabilities = [\"legacy/repo:architect\"]\n\
         max_concurrent_jobs = 6\n",
    );

    apply_runtime_overrides(
        &mut resolved,
        Some(Service::Worker),
        &RuntimeOverrides::default(),
    )
    .expect("default worker runtime applies");

    let worker_config =
        temper_worker_service::worker_config(&resolved).expect("worker runtime config builds");
    assert_eq!(worker_config.max_concurrent_jobs, 6);
    assert_eq!(worker_config.capabilities.len(), 1);
    assert_eq!(worker_config.capabilities[0].repo, "legacy/repo");
    assert_eq!(worker_config.capabilities[0].role, "architect");

    let _ = std::fs::remove_dir_all(dir);
}
