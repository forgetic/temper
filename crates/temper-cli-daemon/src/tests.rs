// SPDX-License-Identifier: MPL-2.0

//! Hermeticity regression tests for the daemon load path.
//!
//! These pin the fix for the original incident: a daemon started with an
//! explicit `--config` (and no `--credentials`) silently layered in the
//! operator's global `~/.config/temper/credentials.toml`, so a *poisoned* global
//! token was used instead of the explicit deployment's secrets. The tests poison
//! a fake `$HOME`'s global credentials, load via [`super::load_for`] with explicit
//! paths and an env snapshot that does NOT set `TEMPER_CONFIG` /
//! `TEMPER_CREDENTIALS`, and assert the poisoned token is never used.

use std::path::{Path, PathBuf};

use temper_config::{EnvMap, ExposeSecret, PathResolver};

use super::{DaemonInputs, load_for};

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
/// `home` and returns an env snapshot whose `HOME` points there — but which sets
/// neither `TEMPER_CONFIG` nor `TEMPER_CREDENTIALS`. This is exactly the box the
/// incident happened on: a real operator with a global config + credentials in
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

/// Explicit `--config` + `--credentials` must use ONLY the explicit pair, even
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

/// The narrower, exact-incident case: explicit `--config` only, no
/// `--credentials`. The poisoned global credentials must NOT layer in — with an
/// explicit config and no env override, default `~/.config/temper` discovery is
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
