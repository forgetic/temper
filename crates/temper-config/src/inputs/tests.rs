// SPDX-License-Identifier: MPL-2.0

//! Hermeticity tests for [`load_explicit`].
//!
//! Every input is constructed in-memory: no test here reads `std::env`, `$HOME`,
//! or the real `~/.config/temper`. The first test is the direct fix for the e2e
//! incident — a default load with empty inputs must discover NOTHING even when
//! the operator's global config exists on the box.

use std::path::PathBuf;

use crate::ExposeSecret;
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

const CONFIG_WITH_ADMIN: &str = "schema_version = 1\n\
     [forge]\n\
     type = \"forgejo\"\n\
     url = \"http://localhost:3000\"\n\
     admin = \"agent\"\n\
     [engine]\n\
     repos = [\"a/b\"]\n\
     roles = [\"engineer\"]\n";

fn credentials_with_agent_token(token: &str) -> String {
    format!(
        "schema_version = 1\n\
         [forge.users.agent]\n\
         token = \"{token}\"\n"
    )
}

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
fn explicit_config_directory_loads_config_toml_and_sibling_credentials() {
    let dir = temp_dir("explicit-config-dir");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    let config_path = bundle.join("config.toml");
    let credentials_path = bundle.join("credentials.toml");
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::write(
        &credentials_path,
        credentials_with_agent_token("sibling-token"),
    )
    .expect("write credentials");

    let inputs = LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("bundle load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(credentials_path.as_path())
    );
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("sibling-token")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explicit_config_file_uses_sibling_credentials_without_ambient_default() {
    let dir = temp_dir("explicit-config-file-sibling");
    let home = dir.join("home");
    let global = home.join(".config").join("temper");
    std::fs::create_dir_all(&global).expect("create global config dir");
    std::fs::write(
        global.join("credentials.toml"),
        credentials_with_agent_token("poisoned-global-token"),
    )
    .expect("write global credentials");

    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    let config_path = bundle.join("local-dev.toml");
    let credentials_path = bundle.join("credentials.toml");
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::write(
        &credentials_path,
        credentials_with_agent_token("sibling-token"),
    )
    .expect("write credentials");

    let mut env = EnvMap::new();
    env.insert("HOME", home.to_string_lossy().into_owned());
    let paths = PathResolver::from_env(&env);
    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit file load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(credentials_path.as_path())
    );
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("sibling-token")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explicit_config_file_without_sibling_does_not_discover_default_credentials() {
    let dir = temp_dir("explicit-config-file-no-sibling");
    let home = dir.join("home");
    let global = home.join(".config").join("temper");
    std::fs::create_dir_all(&global).expect("create global config dir");
    std::fs::write(
        global.join("credentials.toml"),
        credentials_with_agent_token("poisoned-global-token"),
    )
    .expect("write global credentials");

    let config_path = dir.join("bundle").join("local-dev.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).expect("create bundle dir");
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");

    let mut env = EnvMap::new();
    env.insert("HOME", home.to_string_lossy().into_owned());
    let paths = PathResolver::from_env(&env);
    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit file load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert!(
        loaded.credentials.is_none(),
        "global credentials must not load behind explicit config: {:?}",
        loaded.credentials
    );
    assert!(
        resolved.forge.admin_token.is_none(),
        "poisoned global token must not resolve"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn explicit_credentials_directory_loads_credentials_toml() {
    let dir = temp_dir("explicit-credentials-dir");
    let config_path = dir.join("config.toml");
    let credentials_dir = dir.join("secrets");
    let credentials_path = credentials_dir.join("credentials.toml");
    std::fs::write(&config_path, CONFIG_WITH_ADMIN).expect("write config");
    std::fs::create_dir_all(&credentials_dir).expect("create credentials dir");
    std::fs::write(
        &credentials_path,
        credentials_with_agent_token("explicit-token"),
    )
    .expect("write credentials");

    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: Some(credentials_dir.clone()),
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit credentials dir loads");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        loaded.credentials.as_deref(),
        Some(credentials_dir.as_path())
    );
    assert_eq!(
        resolved
            .forge
            .admin_token
            .as_ref()
            .map(|token| token.expose_secret()),
        Some("explicit-token")
    );
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
fn temper_config_env_is_ignored() {
    // `TEMPER_CONFIG` no longer selects the config file: file locations come
    // only from `--config` / `--secrets` and the XDG/HOME default location.
    // With that env var set but no explicit path and an empty PathResolver,
    // discovery still finds NOTHING — the env-pointed file is ignored.
    let dir = temp_dir("env-config-ignored");
    let config_path = dir.join("custom-config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let mut env = EnvMap::new();
    env.insert("TEMPER_CONFIG", config_path.to_string_lossy().into_owned());

    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("load resolves to defaults");

    // The env-pointed file was ignored: nothing discovered, defaults only.
    assert!(
        loaded.config.is_none(),
        "TEMPER_CONFIG must not select a file, got {:?}",
        loaded.config
    );
    assert!(
        loaded.credentials.is_none(),
        "env must not select a credentials file, got {:?}",
        loaded.credentials
    );
    assert!(resolved.engine.roles.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn from_env_derives_base_dirs_from_injected_env() {
    // PathResolver::from_env reads HOME / XDG_* through the injected env, so a
    // load built from it discovers <home>/.config/temper — restoring the
    // discovery `load_with_env` relied on before paths/env were injectable.
    let home = temp_dir("from-env-home");
    let config_dir = home.join(".config").join("temper");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_path = config_dir.join("config.toml");
    std::fs::write(&config_path, MINIMAL_CONFIG).expect("write config");

    let mut env = EnvMap::new();
    env.insert("HOME", home.to_string_lossy().into_owned());

    let paths = PathResolver::from_env(&env);
    assert_eq!(paths.home.as_deref(), Some(home.as_path()));

    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &env,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("env-derived discovery loads");
    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(resolved.engine.roles, vec!["engineer"]);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn from_env_with_empty_env_discovers_nothing() {
    // The hermeticity contract still holds for from_env: an empty env yields an
    // all-`None` resolver, so nothing is discovered.
    let paths = PathResolver::from_env(&NoEnv);
    assert!(paths.home.is_none());
    assert!(paths.xdg_config_home.is_none());
    assert!(paths.xdg_state_home.is_none());

    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &paths,
    };
    let (_resolved, loaded) = load_explicit(&inputs).expect("empty from_env load resolves");
    assert!(loaded.config.is_none());
    assert!(loaded.credentials.is_none());
}

#[test]
fn explicit_config_directory_resolves_relative_paths_under_bundle_root() {
    let dir = temp_dir("relative-bundle-paths");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    std::fs::write(
        bundle.join("config.toml"),
        "schema_version = 1\n\
         [engine]\n\
         workflow = \"flows/workflow.json\"\n\
         webhook_secret_file = \"secrets/webhook-secret\"\n\
         [worker]\n\
         workspace = \"workspace\"\n\
         [agent]\n\
         config_dir = \"agent-config\"\n",
    )
    .expect("write config");

    let inputs = LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("bundle load succeeds");

    assert_eq!(
        loaded.config.as_deref(),
        Some(bundle.join("config.toml").as_path())
    );
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(bundle.join("flows/workflow.json").as_path())
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(bundle.join("secrets/webhook-secret").as_path())
    );
    assert_eq!(resolved.worker.workspace_root, bundle.join("workspace"));
    assert_eq!(
        resolved.agent.config_dir.as_deref(),
        Some(bundle.join("agent-config").as_path())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

mod target_paths;

#[test]
fn explicit_config_file_resolves_relative_paths_under_file_parent() {
    let dir = temp_dir("relative-file-paths");
    let profile_dir = dir.join("profiles");
    std::fs::create_dir_all(&profile_dir).expect("create profile dir");
    let config_path = profile_dir.join("local.toml");
    std::fs::write(
        &config_path,
        "schema_version = 1\n\
         [engine]\n\
         workflow = \"flows/workflow.json\"\n\
         webhook_secret_file = \"secrets/webhook-secret\"\n\
         [worker]\n\
         workspace = \"workspace\"\n\
         [agent]\n\
         config_dir = \"agent-config\"\n",
    )
    .expect("write config");

    let inputs = LoadInputs {
        explicit_config: Some(config_path.clone()),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("explicit file load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(profile_dir.join("flows/workflow.json").as_path())
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(profile_dir.join("secrets/webhook-secret").as_path())
    );
    assert_eq!(
        resolved.worker.workspace_root,
        profile_dir.join("workspace")
    );
    assert_eq!(
        resolved.agent.config_dir.as_deref(),
        Some(profile_dir.join("agent-config").as_path())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn default_config_resolves_relative_paths_under_default_config_root() {
    let xdg = temp_dir("relative-default-xdg");
    let config_root = xdg.join("temper");
    std::fs::create_dir_all(&config_root).expect("create default config root");
    let config_path = config_root.join("config.toml");
    std::fs::write(
        &config_path,
        "schema_version = 1\n\
         [engine]\n\
         workflow = \"flows/workflow.json\"\n\
         webhook_secret_file = \"secrets/webhook-secret\"\n\
         [worker]\n\
         workspace = \"workspace\"\n\
         [agent]\n\
         config_dir = \"agent-config\"\n",
    )
    .expect("write config");

    let paths = PathResolver {
        xdg_config_home: Some(xdg.clone()),
        ..PathResolver::default()
    };
    let inputs = LoadInputs {
        explicit_config: None,
        explicit_credentials: None,
        env: &NoEnv,
        paths: &paths,
    };
    let (resolved, loaded) = load_explicit(&inputs).expect("default config load succeeds");

    assert_eq!(loaded.config.as_deref(), Some(config_path.as_path()));
    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(config_root.join("flows/workflow.json").as_path())
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(config_root.join("secrets/webhook-secret").as_path())
    );
    assert_eq!(
        resolved.worker.workspace_root,
        config_root.join("workspace")
    );
    assert_eq!(
        resolved.agent.config_dir.as_deref(),
        Some(config_root.join("agent-config").as_path())
    );
    let _ = std::fs::remove_dir_all(&xdg);
}

#[test]
fn config_relative_resolution_preserves_absolute_and_tilde_paths() {
    let dir = temp_dir("absolute-tilde-paths");
    let bundle = dir.join("bundle");
    let home = dir.join("home");
    std::fs::create_dir_all(&bundle).expect("create bundle dir");
    let absolute_workflow = dir.join("elsewhere").join("workflow.json");
    let absolute_workspace = dir.join("elsewhere").join("workspace");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [engine]\n\
             workflow = \"{}\"\n\
             webhook_secret_file = \"~/.temper/webhook-secret\"\n\
             [worker]\n\
             workspace = \"{}\"\n\
             [agent]\n\
             config_dir = \"~service/prompts\"\n",
            absolute_workflow.display(),
            absolute_workspace.display(),
        ),
    )
    .expect("write config");

    let mut env = EnvMap::new();
    env.insert("HOME", home.to_string_lossy().into_owned());
    let inputs = LoadInputs {
        explicit_config: Some(bundle.clone()),
        explicit_credentials: None,
        env: &env,
        paths: &PathResolver::default(),
    };
    let (resolved, _loaded) = load_explicit(&inputs).expect("bundle load succeeds");

    assert_eq!(
        resolved.engine.workflow_file.as_deref(),
        Some(absolute_workflow.as_path())
    );
    assert_eq!(
        resolved.engine.webhook_secret_file.as_deref(),
        Some(home.join(".temper/webhook-secret").as_path())
    );
    assert_eq!(resolved.worker.workspace_root, absolute_workspace);
    assert_eq!(
        resolved.agent.config_dir.as_deref(),
        Some(std::path::Path::new("~service/prompts"))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_explicit_file_is_an_error() {
    // An explicit `--config` path is *required*: a missing file errors (it is not
    // silently treated as absent like a default-location file).
    let inputs = LoadInputs {
        explicit_config: Some(PathBuf::from("/nonexistent/temper/config.toml")),
        explicit_credentials: None,
        env: &NoEnv,
        paths: &PathResolver::default(),
    };
    let err = load_explicit(&inputs).expect_err("missing required file errors");
    assert!(
        format!("{err}").contains("config.toml"),
        "error must name the file: {err}"
    );
}
