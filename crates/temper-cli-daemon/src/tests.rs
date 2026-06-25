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

use std::path::{Path, PathBuf};

use temper_config::{EnvMap, ExposeSecret, PathResolver};

use super::{
    DaemonInputs, SERVE_ENGINE_USAGE, SERVE_STANDALONE_USAGE, SERVE_USAGE, SERVE_WORKER_USAGE,
    ServeInvocation, Service, load_for, parse_daemon_args, parse_serve_invocation,
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
fn serve_usage_documents_supported_components() {
    assert!(
        SERVE_USAGE.contains("standalone"),
        "serve help should advertise standalone mode"
    );
    assert!(
        SERVE_USAGE.contains("engine      Run the engine service"),
        "serve help should advertise engine as supported: {SERVE_USAGE}"
    );
    assert!(
        SERVE_USAGE.contains("worker      Run the worker service"),
        "serve help should advertise worker as supported: {SERVE_USAGE}"
    );
    assert!(
        SERVE_USAGE.contains("trigger     Not implemented yet"),
        "serve help should keep trigger explicitly unimplemented"
    );
    assert!(
        SERVE_USAGE.contains("temper --config") && SERVE_USAGE.contains("--secrets"),
        "serve help should show deployment file flags before `serve`"
    );
    assert!(SERVE_STANDALONE_USAGE.contains("serve standalone"));
    assert!(!SERVE_STANDALONE_USAGE.contains("--secrets"));
    assert!(!SERVE_STANDALONE_USAGE.contains("--config"));
    assert!(
        SERVE_STANDALONE_USAGE.contains("temper daemon"),
        "standalone help should identify the compatibility wrapper"
    );
}

#[test]
fn serve_service_help_documents_current_thin_dispatch_surface() {
    for (usage, component) in [
        (SERVE_ENGINE_USAGE, "engine"),
        (SERVE_WORKER_USAGE, "worker"),
    ] {
        assert!(usage.contains(&format!("serve {component}")), "{usage}");
        assert!(
            usage.contains(&format!("temper daemon --service {component}")),
            "{usage}"
        );
        assert!(usage.contains("temper --config"), "{usage}");
        assert!(usage.contains("--secrets"), "{usage}");
        for flag in ["--id", "--pool", "--capacity", "--engine-url"] {
            assert!(usage.contains(flag), "{usage}");
        }
        assert!(usage.contains("Not implemented yet"), "{usage}");
    }
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

#[test]
fn serve_standalone_maps_to_daemon_standalone() {
    assert_eq!(
        parse_serve_invocation(vec!["standalone".to_string()]).expect("standalone command parses"),
        ServeInvocation::Standalone
    );
}

#[test]
fn serve_engine_and_worker_map_to_daemon_services() {
    assert_eq!(
        parse_serve_invocation(vec!["engine".to_string()]).expect("engine command parses"),
        ServeInvocation::Service(Service::Engine)
    );
    assert_eq!(
        parse_serve_invocation(vec!["worker".to_string()]).expect("worker command parses"),
        ServeInvocation::Service(Service::Worker)
    );
}

#[test]
fn serve_components_reject_local_config_and_secrets_flags() {
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
fn serve_services_reject_future_target_flags() {
    for component in ["engine", "worker"] {
        for flag in ["--id", "--pool", "--capacity", "--engine-url"] {
            let error = parse_serve_invocation(vec![
                component.to_string(),
                flag.to_string(),
                "value".to_string(),
            ])
            .expect_err("future target flags must not be accepted yet");

            assert!(error.contains(component), "{error}");
            assert!(error.contains(flag), "{error}");
            assert!(error.contains("not implemented yet"), "{error}");
        }
    }
}

#[test]
fn serve_trigger_remains_rejected_with_helpful_message() {
    let error = parse_serve_invocation(vec!["trigger".to_string()])
        .expect_err("trigger serve component should remain rejected");

    assert!(error.contains("temper serve trigger"), "{error}");
    assert!(error.contains("not implemented yet"), "{error}");
    assert!(error.contains("later workitem"), "{error}");
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
