// SPDX-License-Identifier: MPL-2.0

//! Hermeticity tests for [`load_explicit`].
//!
//! Every input is constructed in-memory: no test here reads `std::env`, `$HOME`,
//! or the real `~/.config/temper`. The first test is the direct fix for the e2e
//! incident — a default load with empty inputs must discover NOTHING even when
//! the operator's global config exists on the box.

use std::path::PathBuf;

use crate::env::{EnvMap, NoEnv};
use crate::inputs::{LoadInputs, PathResolver, load_explicit};

/// A temp directory unique to this process + a nonce, cleaned by the caller.
fn temp_dir(tag: &str) -> PathBuf {
    let pid = std::process::id();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("temper-config-inputs-{tag}-{pid}-{nonce}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

const MINIMAL_CONFIG: &str =
    "schema_version = 1\n[engine]\nrepos = [\"a/b\"]\nroles = [\"engineer\"]\n";

#[test]
fn empty_inputs_discover_nothing() {
    // The incident fix: empty PathResolver + empty env discovers nothing, even
    // though the real ~/.config/temper may exist on this machine. We never
    // construct a PathResolver from the system here, so there is no path to the
    // operator's global config at all.
    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("empty load resolves to defaults");

    assert!(
        loaded.config.is_none(),
        "no config file may be discovered, got {:?}",
        loaded.config
    );
    assert!(
        loaded.credentials.is_none(),
        "no credentials file may be discovered, got {:?}",
        loaded.credentials
    );
    // Defaults only: no repos, no roles, no forge URL.
    assert!(resolved.engine.repos.is_empty());
    assert!(resolved.engine.roles.is_empty());
    assert!(resolved.forge.url.is_none());
}

#[test]
fn explicit_paths_load_with_empty_resolver() {
    // Even with an empty PathResolver, an explicit --config path loads.
    let dir = temp_dir("explicit");
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit config loads");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(resolved.engine.roles, vec!["engineer"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mocked_home_discovers_default_config() {
    // Default discovery finds <home>/.config/temper/config.toml — but only
    // because we inject `home`, never read the real one.
    let home = temp_dir("home");
    let config_dir = home.join(".config").join("temper");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_path = config_dir.join("config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let paths = PathResolver {
        home: Some(home.clone()),
        ..PathResolver::default()
    };
    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("default discovery loads");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(resolved.engine.roles, vec!["engineer"]);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn xdg_config_home_wins_over_home() {
    let home = temp_dir("xdg-home");
    let xdg = temp_dir("xdg-config");
    let xdg_temper = xdg.join("temper");
    std::fs::create_dir_all(&xdg_temper).expect("create xdg config dir");
    let config_path = xdg_temper.join("config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let paths = PathResolver {
        xdg_config_home: Some(xdg.clone()),
        home: Some(home.clone()),
        ..PathResolver::default()
    };
    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &paths,
    };
    let (_resolved, loaded) = load_explicit(&inputs).expect("xdg discovery loads");
    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn temper_config_env_honored_from_injected_env() {
    // TEMPER_CONFIG / TEMPER_CREDENTIALS come from the INJECTED env, not the
    // real process environment, and override the (empty) default discovery.
    let dir = temp_dir("env-config");
    let config_path = dir.join("custom-config.toml");
    let credentials_path = dir.join("custom-credentials.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");
    std::fs::write(
        &credentials_path,
        "schema_version = 1\n[agent.providers.anthropic]\ntype = \"api-key\"\nkey = \"sk-x\"\n",
    )
    .expect("write credentials");

    let mut env = EnvMap::new();
    env.insert("TEMPER_CONFIG", config_path.to_string_lossy().into_owned());
    env.insert(
        "TEMPER_CREDENTIALS",
        credentials_path.to_string_lossy().into_owned(),
    );

    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("env-pointed files load");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(credentials_path.as_path())
    );
    assert_eq!(resolved.engine.roles, vec!["engineer"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_env_pointed_file_is_an_error() {
    // An env-var path is *required*: a missing file errors (it is not silently
    // treated as absent like a default-location file).
    let mut env = EnvMap::new();
    env.insert("TEMPER_CONFIG", "/nonexistent/temper/config.toml");
    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let err = load_explicit(&inputs).expect_err("missing required file errors");
    assert!(
        format!("{err}").contains("config.toml"),
        "error must name the file: {err}"
    );
}
