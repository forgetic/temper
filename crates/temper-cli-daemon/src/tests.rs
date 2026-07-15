// SPDX-License-Identifier: MPL-2.0

//! Hermeticity regression tests for the daemon load path.
//!
//! These pin the fix for the original incident: a daemon started with an
//! explicit `--config` (and no secret source) silently layered in the
//! operator's global `~/.config/temper/credentials.toml`, so a *poisoned* global
//! token was used instead of the explicit deployment's secrets. The tests poison
//! a fake `$HOME`'s global credentials, load via [`super::load_for`] with explicit
//! paths and an env snapshot that has no legacy config-file selectors, and
//! assert the poisoned token is never used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use temper_config::{EnvMap, ExposeSecret, PathResolver, WorkerSettings};
use temper_worker::CapabilitySpec;

use super::{
    DaemonInputs, RuntimeOverrides, SERVE_ENGINE_USAGE, SERVE_WORKER_USAGE, ServeInvocation,
    Service, load_for, parse_daemon_args, parse_serve_invocation, standalone,
};

const POISONED_TOKEN: &str = "POISONED-GLOBAL-TOKEN-DO-NOT-USE";
const EXPLICIT_TOKEN: &str = "explicit-deployment-token";

/// A scratch directory unique to one test, created without reading the process
/// environment (this is a library test and must stay hermetic).
fn scratch(tag: &str) -> PathBuf {
    let dir = PathBuf::from("/tmp").join(format!(
        "temper-daemon-hermeticity-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir creates");
    dir
}

/// Writes a minimal config file: forge URL, the admin user `agent` (so the
/// admin token resolves from `[forge.users.agent]`), and an engine block.
fn write_config(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("config parent creates");
    }
    std::fs::write(
        path,
        "schema_version = 1\n\
         [forge]\n\
         type = \"forgejo\"\n\
         url = \"http://explicit-forge:3000\"\n\
         admin = \"agent\"\n\
         [engine]\n\
         repos = [\"acme/widgets\"]\n\
         roles = [\"engineer\"]\n",
    )
    .expect("write config");
}

/// Writes a credentials file whose admin (`agent`) token is `token`.
fn write_credentials(path: &Path, token: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("credentials parent creates");
    }
    std::fs::write(
        path,
        format!(
            "schema_version = 1\n\
             [forge.users.agent]\n\
             token = \"{token}\"\n"
        ),
    )
    .expect("write credentials");
}

fn worker_settings_with_capacity(max_concurrent_jobs: u32) -> WorkerSettings {
    WorkerSettings {
        worker_id: "standalone-worker".to_string(),
        daemon_url: "http://unused-in-process".to_string(),
        workspace_root: PathBuf::from("/tmp/temper-workspace"),
        git_base_url: None,
        max_concurrent_jobs,
        poll_wait: Duration::from_secs(99),
        heartbeat_interval: Duration::from_secs(98),
        capabilities: Vec::new(),
        pools: Vec::new(),
        worker_pool_tokens: BTreeMap::new(),
        selected_pool: None,
    }
}

/// Plants a poisoned global `~/.config/temper/{config,credentials}.toml` under
/// `home` and returns an env snapshot whose `HOME` points there. This is exactly
/// the incident box: a real operator with a global config + credentials in
/// `$HOME`. The global config declares `admin = "agent"` so the poisoned global
/// token *would* resolve if discovery were not suppressed.
fn poison_home(home: &Path) -> EnvMap {
    write_config(&home.join(".config/temper/config.toml"));
    write_credentials(
        &home.join(".config/temper/credentials.toml"),
        POISONED_TOKEN,
    );
    let mut env = EnvMap::new();
    env.insert("HOME", home.to_string_lossy().to_string());
    env
}

#[test]
fn standalone_worker_config_uses_resolved_capacity() {
    let config = standalone::standalone_worker_config(
        &worker_settings_with_capacity(2),
        vec![CapabilitySpec {
            role: "engineer".to_string(),
            repo: "ai/temper".to_string(),
        }],
        BTreeMap::new(),
        temper_worker::WorkerAgentTraceConfig::default(),
    )
    .expect("standalone worker config builds");

    assert_eq!(config.max_concurrent_jobs, 2);
    assert_eq!(config.worker_id, "standalone-worker");
    assert_eq!(config.capabilities.len(), 1);
    assert_eq!(config.capabilities[0].role, "engineer");
    assert_eq!(config.capabilities[0].repo, "ai/temper");
}

/// Explicit `--config` + `--secrets` must use ONLY the explicit pair, even
/// though a poisoned global credentials file exists under the env snapshot's HOME.
#[test]
fn explicit_config_and_credentials_ignore_poisoned_global() {
    let dir = scratch("both");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home creates");
    let env = poison_home(&home);

    let config = dir.join("deploy/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).expect("config dir");
    write_config(&config);
    let credentials = dir.join("deploy/credentials.toml");
    write_credentials(&credentials, EXPLICIT_TOKEN);

    let paths = PathResolver::from_env(&env);
    let inputs = DaemonInputs {
        config: Some(config),
        credentials: Some(credentials),
        service: None,
        runtime: RuntimeOverrides::default(),
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_for(&inputs).expect("load succeeds");

    let admin = resolved
        .forge
        .admin_token
        .as_ref()
        .expect("admin token resolved");
    assert_eq!(
        admin.expose_secret(),
        EXPLICIT_TOKEN,
        "explicit credentials must win"
    );
    assert_ne!(
        admin.expose_secret(),
        POISONED_TOKEN,
        "poisoned global token must NEVER be used"
    );
    // And the global file under HOME must not even be on the loaded-paths record.
    assert!(
        loaded
            .credentials
            .as_deref()
            .map(|p| !p.to_string_lossy().contains(".config/temper"))
            .unwrap_or(true),
        "loaded credentials path must not be the global file: {:?}",
        loaded.credentials
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An explicit config directory is a local bundle root: the daemon loads
/// `<root>/config.toml` and, when no `--secrets` is supplied, the sibling
/// `<root>/credentials.toml` instead of any global credentials file.
#[test]
fn explicit_config_directory_uses_sibling_credentials_not_global() {
    let dir = scratch("config-dir-bundle");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home creates");
    let env = poison_home(&home);

    let bundle = dir.join("deploy");
    let config = bundle.join("config.toml");
    write_config(&config);
    let credentials = bundle.join("credentials.toml");
    write_credentials(&credentials, EXPLICIT_TOKEN);

    let paths = PathResolver::from_env(&env);
    let inputs = DaemonInputs {
        config: Some(bundle.clone()),
        credentials: None,
        service: None,
        runtime: RuntimeOverrides::default(),
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_for(&inputs).expect("load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config.as_path()));
    assert_eq!(loaded.credentials.as_deref(), Some(credentials.as_path()));
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret().to_string()),
        Some(EXPLICIT_TOKEN.to_string())
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The narrower, exact-incident case: explicit `--config` only, no secret
/// source, and no sibling credentials file. The poisoned global credentials must
/// global credentials must NOT layer in — default `~/.config/temper` discovery is
/// suppressed, so no credentials are discovered at all (the poisoned token is
/// absent from the result).
#[test]
fn explicit_config_only_does_not_layer_poisoned_global() {
    let dir = scratch("config-only");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home creates");
    let env = poison_home(&home);

    let config = dir.join("deploy/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).expect("config dir");
    write_config(&config);

    let paths = PathResolver::from_env(&env);
    let inputs = DaemonInputs {
        config: Some(config),
        credentials: None,
        service: None,
        runtime: RuntimeOverrides::default(),
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_for(&inputs).expect("load succeeds");

    // No credentials were discovered (the global file was NOT layered in), so the
    // admin token is unset — and, crucially, it is never the poisoned value.
    if let Some(admin) = resolved.forge.admin_token.as_ref() {
        assert_ne!(
            admin.expose_secret(),
            POISONED_TOKEN,
            "poisoned global token must NEVER be used"
        );
    }
    assert!(
        loaded.credentials.is_none(),
        "no credentials file should have been discovered, got {:?}",
        loaded.credentials
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Sanity / negative control: with NO explicit paths and the same env snapshot,
/// the global credentials under HOME *are* discovered (proving the test harness
/// really plants a reachable poisoned file). This documents that the suppression
/// in the cases above is the explicit-path behavior, not an artifact of the file
/// being unreachable.
#[test]
fn without_explicit_paths_global_is_discovered() {
    let dir = scratch("ambient");
    let home = dir.join("home");
    std::fs::create_dir_all(&home).expect("home creates");
    let env = poison_home(&home);

    let paths = PathResolver::from_env(&env);
    let inputs = DaemonInputs {
        config: None,
        credentials: None,
        service: None,
        runtime: RuntimeOverrides::default(),
        env: &env,
        paths: &paths,
    };
    let (resolved, _loaded) = load_for(&inputs).expect("load succeeds");

    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|t| t.expose_secret().to_string()),
        Some(POISONED_TOKEN.to_string()),
        "with no explicit path, the ambient global file is (intentionally) discovered"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn serve_service_help_documents_implemented_target_flags() {
    assert!(
        SERVE_ENGINE_USAGE.contains("serve engine"),
        "{SERVE_ENGINE_USAGE}"
    );
    assert!(
        SERVE_ENGINE_USAGE.contains("/forgejo/webhook"),
        "{SERVE_ENGINE_USAGE}"
    );
    assert!(
        SERVE_ENGINE_USAGE.contains("temper --config"),
        "{SERVE_ENGINE_USAGE}"
    );
    assert!(
        SERVE_ENGINE_USAGE.contains("--secrets"),
        "{SERVE_ENGINE_USAGE}"
    );
    assert!(
        SERVE_ENGINE_USAGE.contains("--id <ID>"),
        "{SERVE_ENGINE_USAGE}"
    );
    assert!(
        !SERVE_ENGINE_USAGE.contains("Not implemented yet"),
        "{SERVE_ENGINE_USAGE}"
    );

    assert!(
        SERVE_WORKER_USAGE.contains("serve worker"),
        "{SERVE_WORKER_USAGE}"
    );
    assert!(
        SERVE_WORKER_USAGE.contains("[[worker.pools]]"),
        "{SERVE_WORKER_USAGE}"
    );
    assert!(
        SERVE_WORKER_USAGE.contains("temper --config"),
        "{SERVE_WORKER_USAGE}"
    );
    assert!(
        SERVE_WORKER_USAGE.contains("--secrets"),
        "{SERVE_WORKER_USAGE}"
    );
    for flag in ["--id", "--pool", "--capacity", "--engine-url"] {
        assert!(SERVE_WORKER_USAGE.contains(flag), "{SERVE_WORKER_USAGE}");
    }
    assert!(
        !SERVE_WORKER_USAGE.contains("Not implemented yet"),
        "{SERVE_WORKER_USAGE}"
    );
}

#[test]
fn serve_help_forms_are_parsed_without_starting_daemon() {
    assert_eq!(
        parse_serve_invocation(vec!["--help".to_string()]).expect("serve help parses"),
        ServeInvocation::Help
    );
    assert_eq!(
        parse_serve_invocation(vec!["standalone".to_string(), "--help".to_string()])
            .expect("standalone help parses"),
        ServeInvocation::StandaloneHelp
    );
    assert_eq!(
        parse_serve_invocation(vec!["engine".to_string(), "--help".to_string()])
            .expect("engine help parses"),
        ServeInvocation::ServiceHelp(Service::Engine)
    );
    assert_eq!(
        parse_serve_invocation(vec!["worker".to_string(), "--help".to_string()])
            .expect("worker help parses"),
        ServeInvocation::ServiceHelp(Service::Worker)
    );
}

fn runtime_with_id(id: &str) -> RuntimeOverrides {
    RuntimeOverrides {
        process_id: Some(id.to_string()),
        ..RuntimeOverrides::default()
    }
}

#[test]
fn serve_standalone_maps_to_daemon_standalone() {
    assert_eq!(
        parse_serve_invocation(vec!["standalone".to_string()]).expect("standalone command parses"),
        ServeInvocation::Standalone(RuntimeOverrides::default())
    );
}

#[test]
fn serve_engine_and_worker_map_to_daemon_services() {
    assert_eq!(
        parse_serve_invocation(vec!["engine".to_string()]).expect("engine command parses"),
        ServeInvocation::Service(Service::Engine, RuntimeOverrides::default())
    );
    assert_eq!(
        parse_serve_invocation(vec!["worker".to_string()]).expect("worker command parses"),
        ServeInvocation::Service(Service::Worker, RuntimeOverrides::default())
    );
}

#[test]
fn serve_components_parse_supported_target_flags() {
    assert_eq!(
        parse_serve_invocation(vec![
            "standalone".to_string(),
            "--id".to_string(),
            "all-in-one-a".to_string(),
        ])
        .expect("standalone --id parses"),
        ServeInvocation::Standalone(runtime_with_id("all-in-one-a"))
    );
    assert_eq!(
        parse_serve_invocation(vec![
            "engine".to_string(),
            "--id".to_string(),
            "engine-a".to_string(),
        ])
        .expect("engine --id parses"),
        ServeInvocation::Service(Service::Engine, runtime_with_id("engine-a"))
    );
    assert_eq!(
        parse_serve_invocation(vec![
            "worker".to_string(),
            "--pool".to_string(),
            "builders".to_string(),
            "--id".to_string(),
            "worker-a".to_string(),
            "--capacity".to_string(),
            "3".to_string(),
            "--engine-url".to_string(),
            "http://engine.local:8080".to_string(),
        ])
        .expect("worker target flags parse"),
        ServeInvocation::Service(
            Service::Worker,
            RuntimeOverrides {
                process_id: Some("worker-a".to_string()),
                worker_pool: Some("builders".to_string()),
                worker_capacity: Some(3),
                worker_engine_url: Some("http://engine.local:8080".to_string()),
            },
        )
    );
}

#[test]
fn serve_components_reject_local_config_and_secrets_flags() {
    for flag in ["--config", "--secrets", "-c"] {
        let error =
            parse_serve_invocation(vec![flag.to_string(), "deploy/config.toml".to_string()])
                .expect_err("file-location flags must be global-only directly after serve");

        assert!(error.contains(flag), "{error}");
        assert!(error.contains("global option"), "{error}");
        assert!(error.contains("before `serve`"), "{error}");
    }

    for component in ["standalone", "engine", "worker"] {
        for flag in ["--config", "--secrets", "-c"] {
            let error = parse_serve_invocation(vec![
                component.to_string(),
                flag.to_string(),
                "deploy/config.toml".to_string(),
            ])
            .expect_err("file-location flags must be global-only");

            assert!(error.contains(flag), "{error}");
            assert!(error.contains("global option"), "{error}");
            assert!(error.contains("before `serve`"), "{error}");
        }
    }
    let error = parse_serve_invocation(vec![
        "worker".to_string(),
        "--id".to_string(),
        "worker-a".to_string(),
        "--secrets".to_string(),
        "deploy/credentials.toml".to_string(),
    ])
    .expect_err("file-location flags must remain global-only after target flags");
    assert!(error.contains("--secrets"), "{error}");
    assert!(error.contains("global option"), "{error}");
    assert!(error.contains("before `serve`"), "{error}");
}

#[test]
fn serve_components_reject_legacy_secret_source_flag() {
    let legacy = format!("--{}", "credentials");
    for component in ["standalone", "engine", "worker"] {
        let error = parse_serve_invocation(vec![
            component.to_string(),
            legacy.clone(),
            "deploy/credentials.toml".to_string(),
        ])
        .expect_err("legacy secret-source flag must not be accepted under serve component");

        assert!(error.contains(&legacy), "{error}");
    }
}

#[test]
fn serve_standalone_rejects_service_escape_hatch() {
    let error = parse_serve_invocation(vec![
        "standalone".to_string(),
        "--service".to_string(),
        "engine".to_string(),
    ])
    .expect_err("--service must not be accepted under serve standalone");

    assert!(error.contains("--service"));
    assert!(error.contains("standalone path"));
}

#[test]
fn serve_components_reject_missing_target_flag_values() {
    for (component, flag) in [
        ("standalone", "--id"),
        ("engine", "--id"),
        ("worker", "--id"),
        ("worker", "--pool"),
        ("worker", "--capacity"),
        ("worker", "--engine-url"),
    ] {
        let error = parse_serve_invocation(vec![component.to_string(), flag.to_string()])
            .expect_err("target flags require values");

        assert!(error.contains(flag), "{error}");
        assert!(error.contains("requires a value"), "{error}");
    }
}

#[test]
fn serve_components_reject_empty_target_flag_values() {
    for (component, flag) in [
        ("standalone", "--id"),
        ("engine", "--id"),
        ("worker", "--id"),
    ] {
        let error = parse_serve_invocation(vec![
            component.to_string(),
            flag.to_string(),
            "   ".to_string(),
        ])
        .expect_err("identity flags require non-empty values");

        assert!(error.contains(flag), "{error}");
        assert!(error.contains("non-empty"), "{error}");
    }
}

#[test]
fn serve_worker_rejects_invalid_capacity_values() {
    for (raw, expected) in [
        ("0", "greater than zero"),
        ("many", "invalid --capacity"),
        ("-1", "invalid --capacity"),
    ] {
        let error = parse_serve_invocation(vec![
            "worker".to_string(),
            "--capacity".to_string(),
            raw.to_string(),
        ])
        .expect_err("invalid capacity should be rejected");

        assert!(error.contains(expected), "{error}");
        assert!(error.contains("--capacity"), "{error}");
    }
}

#[test]
fn serve_components_reject_flags_for_wrong_component() {
    for (component, flag) in [
        ("standalone", "--pool"),
        ("standalone", "--capacity"),
        ("standalone", "--engine-url"),
        ("engine", "--pool"),
        ("engine", "--capacity"),
        ("engine", "--engine-url"),
    ] {
        let error = parse_serve_invocation(vec![
            component.to_string(),
            flag.to_string(),
            "value".to_string(),
        ])
        .expect_err("worker-only flags must be rejected on other components");

        assert!(error.contains(component), "{error}");
        assert!(error.contains(flag), "{error}");
        assert!(error.contains("serve worker"), "{error}");
    }
}

#[test]
fn daemon_rejects_local_config_and_secrets_flags() {
    for flag in ["--config", "--secrets", "-c"] {
        let error = parse_daemon_args(vec![flag.to_string(), "deploy/config.toml".to_string()])
            .expect_err("file-location flags must be global-only");

        assert!(error.contains(flag), "{error}");
        assert!(error.contains("global option"), "{error}");
    }
}

mod serve_runtime;
mod serve_trigger_contract;
