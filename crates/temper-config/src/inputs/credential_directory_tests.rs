// SPDX-License-Identifier: MPL-2.0

use std::path::PathBuf;

use crate::ExposeSecret;
use crate::env::{EnvMap, NoEnv};
use crate::inputs::{LoadInputs, PathResolver, load_explicit};
use crate::paths::{CREDENTIALS_DIRECTORY_ENV, CREDENTIALS_FILE_NAME};

fn temp_dir(tag: &str) -> PathBuf {
    let pid = std::process::id();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("temper-config-credentials-dir-{tag}-{pid}-{nonce}"));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

const CONFIG_WITH_ADMIN: &str = "schema_version = 1\n\
     [forge]\n\
     type = \"forgejo\"\n\
     url = \"http://localhost:3000\"\n\
     admin = \"agent\"\n\
     [engine]\n\
     repos = [\"a/b\"]\n\
     roles = [\"engineer\"]\n";

const CONFIG_WITH_NAMED_ENGINE_SECRETS: &str = "schema_version = 1\n\
     [forge]\n\
     type = \"forgejo\"\n\
     url = \"http://localhost:3000\"\n\
     [engine]\n\
     forge_token = \"forge-engine-token\"\n\
     webhook_secret = \"webhook-secret\"\n\
     repos = [\"a/b\"]\n\
     roles = [\"engineer\"]\n";

fn credentials_with_agent_token(token: &str) -> String {
    format!(
        "schema_version = 1\n\
         [forge.users.agent]\n\
         token = \"{token}\"\n"
    )
}

fn assert_named_engine_secrets_resolve(resolved: &crate::Resolved) {
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("named-forge-token")
    );
    assert_eq!(
        resolved
            .engine
            .forge_token
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.available)),
        Some(("forge-engine-token", true))
    );
    assert_eq!(
        resolved
            .engine
            .webhook_secret
            .as_ref()
            .map(|reference| (reference.name.as_str(), reference.available)),
        Some(("webhook-secret", true))
    );
    assert_eq!(
        resolved
            .engine
            .webhook_secret_value
            .as_ref()
            .map(|secret| secret.expose_secret()),
        Some("named-webhook-secret")
    );
}

#[test]
fn explicit_secrets_directory_loads_named_files_without_credentials_toml() {
    let dir = temp_dir("explicit-named-files");
    let config_path = dir.join("config.toml");
    let secrets_dir = dir.join("secrets");
    std::fs::create_dir_all(&secrets_dir).expect("create secrets dir");
    std::fs::write(&config_path, CONFIG_WITH_NAMED_ENGINE_SECRETS).expect("write config");
    std::fs::write(secrets_dir.join("forge-engine-token"), "named-forge-token\n")
        .expect("write forge token");
    std::fs::write(secrets_dir.join("webhook-secret"), "named-webhook-secret\n")
        .expect("write webhook secret");

    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: Some(secrets_dir.clone()),
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("named files load");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(loaded.credentials.as_deref(), Some(secrets_dir.as_path()));
    assert_named_engine_secrets_resolve(&resolved);
    let rendered = format!("{resolved:?}");
    assert!(!rendered.contains("named-forge-token"), "secret leaked: {rendered}");
    assert!(
        !rendered.contains("named-webhook-secret"),
        "secret leaked: {rendered}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn credentials_directory_loads_named_files_without_credentials_toml() {
    let dir = temp_dir("systemd-named-files");
    let config_path = dir.join("config.toml");
    let credentials_dir = dir.join("systemd-creds");
    std::fs::create_dir_all(&credentials_dir).expect("create credentials dir");
    std::fs::write(&config_path, CONFIG_WITH_NAMED_ENGINE_SECRETS).expect("write config");
    std::fs::write(credentials_dir.join("forge-engine-token"), "named-forge-token\n")
        .expect("write forge token");
    std::fs::write(credentials_dir.join("webhook-secret"), "named-webhook-secret\n")
        .expect("write webhook secret");

    let mut env = EnvMap::new();
    env.insert(
        CREDENTIALS_DIRECTORY_ENV,
        credentials_dir.to_string_lossy().into_owned(),
    );
    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("systemd named files load");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(loaded.credentials.as_deref(), Some(credentials_dir.as_path()));
    assert_named_engine_secrets_resolve(&resolved);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn credentials_directory_credentials_load_when_no_explicit_secrets() {
    let dir = temp_dir("load");
    let bundle = dir.join("deploy");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    let config_path = bundle.join("config.toml");
    let sibling_credentials_path = bundle.join(CREDENTIALS_FILE_NAME);
    let credentials_dir = dir.join("systemd-creds");
    let credentials_path = credentials_dir.join(CREDENTIALS_FILE_NAME);
    std::fs::create_dir_all(&credentials_dir).expect("create credentials dir");
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::write(
        &sibling_credentials_path,
        credentials_with_agent_token("sibling-token"),
    )
    .expect("write sibling credentials");
    std::fs::write(
        &credentials_path,
        credentials_with_agent_token("systemd-token"),
    )
    .expect("write systemd credentials");

    let mut env = EnvMap::new();
    env.insert(
        CREDENTIALS_DIRECTORY_ENV,
        credentials_dir.to_string_lossy().into_owned(),
    );
    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("systemd credentials load");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(loaded.credentials.as_deref(), Some(credentials_dir.as_path()));
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("systemd-token")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn explicit_secrets_win_over_credentials_directory() {
    let dir = temp_dir("explicit-wins");
    let config_path = dir.join("config.toml");
    let explicit_dir = dir.join("explicit-secrets");
    let explicit_path = explicit_dir.join(CREDENTIALS_FILE_NAME);
    let credentials_dir = dir.join("systemd-creds");
    let credentials_path = credentials_dir.join(CREDENTIALS_FILE_NAME);
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::create_dir_all(&explicit_dir).expect("create explicit credentials dir");
    std::fs::create_dir_all(&credentials_dir).expect("create systemd credentials dir");
    std::fs::write(
        &explicit_path,
        credentials_with_agent_token("explicit-token"),
    )
    .expect("write explicit credentials");
    std::fs::write(
        &credentials_path,
        credentials_with_agent_token("systemd-token"),
    )
    .expect("write systemd credentials");

    let mut env = EnvMap::new();
    env.insert(
        CREDENTIALS_DIRECTORY_ENV,
        credentials_dir.to_string_lossy().into_owned(),
    );
    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: Some(explicit_dir.clone()),
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit credentials load");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(loaded.credentials.as_deref(), Some(explicit_dir.as_path()));
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("explicit-token")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn absent_credentials_directory_preserves_sibling_and_default_credentials() {
    let dir = temp_dir("absent");
    let bundle = dir.join("deploy");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    let config_path = bundle.join("config.toml");
    let sibling_credentials_path = bundle.join(CREDENTIALS_FILE_NAME);
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::write(
        &sibling_credentials_path,
        credentials_with_agent_token("sibling-token"),
    )
    .expect("write sibling credentials");

    let sibling_inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&sibling_inputs).expect("sibling credentials load");
    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(sibling_credentials_path.as_path())
    );
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("sibling-token")
    );

    let xdg = dir.join("xdg");
    let default_root = xdg.join("temper");
    std::fs::create_dir_all(&default_root).expect("create default config root");
    let default_config_path = default_root.join("config.toml");
    let default_credentials_path = default_root.join(CREDENTIALS_FILE_NAME);
    std::fs::write(&default_config_path, CONFIG_WITH_ADMIN).expect("write default config");
    std::fs::write(
        &default_credentials_path,
        credentials_with_agent_token("default-token"),
    )
    .expect("write default credentials");
    let paths = PathResolver {
        xdg_config_home: Some(xdg),
        ..PathResolver::default()
    };
    let default_inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&default_inputs).expect("default credentials load");
    assert_eq!(
        loaded.config.as_deref(),
        Some(default_config_path.as_path())
    );
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(default_credentials_path.as_path())
    );
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("default-token")
    );
    let _ = std::fs::remove_dir_all(dir);
}
