// SPDX-License-Identifier: MPL-2.0

//! Shared plumbing for the `temper` subcommand crates.
//!
//! The CLI crates own only argv parsing, terminal I/O, exit codes, and
//! orchestration; the real work lives in the library crates (`temper-config`,
//! `temper-provision`, …). This crate is the lightest of them — it links only
//! [`temper_config`] + `std` + `rpassword` — and hosts the pieces every CLI
//! crate shares:
//!
//! - the [`Prompter`] trait (the interactive-I/O testability seam) and its two
//!   implementations, [`TerminalPrompter`] (real tty) and [`ScriptedPrompter`]
//!   (tests; never touches a tty);
//! - small argv/exit-code helpers ([`next_value`], [`run`]);
//! - file-writing helpers ([`write_new_file`]/[`WriteOutcome`], [`restrict_600`],
//!   [`expand_tilde`], [`resolve_targets`]/[`FileTargets`]).
//!
//! The common config-flag types are re-exported so a subcommand crate need not
//! also depend on `temper-config` just for `--config`/`--secrets` parsing.

mod prompt;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub use prompt::{Prompter, ScriptedPrompter, TerminalPrompter};

// Re-exported from `temper-config` so subcommand crates parse the common flags
// the same way without each depending on `temper-config` directly.
pub use temper_config::{
    CommonArgs, EX_USAGE, EnvLookup, EnvMap, LoadOptions, PathResolver, parse_common_args,
};

/// The environment snapshot every CLI subcommand is dispatched with.
///
/// Built once at the binary boundary by `boot()` in `src/bin/temper.rs`
/// (`std::env::args` / [`EnvMap::from_system`] / [`PathResolver::from_system`] /
/// `current_dir`), then threaded down through dispatch. No library code reads
/// the real environment: it takes the snapshot from here. This is the whole
/// point of the config-centralization epic — `./src/bin/*` is the sole reader of
/// real env/args, and everything below works off this plain-data snapshot.
pub struct CliEnv {
    /// The program arguments *after* `argv[0]` (`std::env::args().skip(1)`).
    pub args: Vec<String>,
    /// A snapshot of the process environment.
    pub env: EnvMap,
    /// The base directories (HOME / XDG_*) resolved once from `env`.
    pub paths: PathResolver,
    /// The process's current working directory.
    pub cwd: PathBuf,
}

/// Pulls the value following a flag out of an iterator, erroring with the flag
/// name when it is missing.
///
/// Lifted verbatim from the old `config_cmd.rs` `next` helper and shared by
/// every subcommand's hand-rolled flag parsing.
pub fn next_value<'a>(
    iter: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<String, String> {
    iter.next()
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// Turns a subcommand's `Result<ExitCode, String>` into an [`ExitCode`],
/// printing any error to stderr prefixed with `prefix` (e.g. `temper config`).
pub fn run(prefix: &str, result: Result<ExitCode, String>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(error) => {
            tracing::error!(target: "temper_cli", prefix = %prefix, %error, "command failed");
            ExitCode::FAILURE
        }
    }
}

/// The outcome of a [`write_new_file`] call.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum WriteOutcome {
    /// The file did not exist and was created.
    Created,
    /// The file existed and was overwritten (only possible with `force`).
    Overwritten,
}

/// Writes `contents` to `path`, creating parent directories as needed.
///
/// With `force == false` an existing `path` is an error (so a CLI never
/// silently clobbers an operator's file); with `force == true` it is
/// overwritten. Returns whether the file was created or overwritten.
pub fn write_new_file(path: &Path, contents: &str, force: bool) -> Result<WriteOutcome, String> {
    let existed = path.exists();
    if existed && !force {
        return Err(format!(
            "{} already exists (pass --force to overwrite)",
            path.display()
        ));
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, contents).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(if existed {
        WriteOutcome::Overwritten
    } else {
        WriteOutcome::Created
    })
}

/// Restricts `path` to owner read/write only (`chmod 0600`) on Unix; a no-op on
/// other platforms.
#[cfg(unix)]
pub fn restrict_600(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("chmod {}: {error}", path.display()))
}

/// Restricts `path` to owner read/write only (`chmod 0600`) on Unix; a no-op on
/// other platforms.
#[cfg(not(unix))]
pub fn restrict_600(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Expands a leading `~` / `~/…` in `value` to `home`.
///
/// Anything else (an absolute path, a relative path, a `~user` form) is taken
/// verbatim. So is a tilde when `home` is `None`. The home directory is passed
/// in explicitly — this crate is a library and never reads the environment;
/// `src/bin` resolves `HOME` once and hands it down.
pub fn expand_tilde(value: &str, home: Option<&Path>) -> PathBuf {
    if value == "~" {
        if let Some(home) = home {
            return home.to_path_buf();
        }
    } else if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    PathBuf::from(value)
}

/// The resolved on-disk locations of the config + credentials files.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FileTargets {
    /// Where `config.toml` is (or will be) written.
    pub config: PathBuf,
    /// Where `credentials.toml` is (or will be) written.
    pub credentials: PathBuf,
}

/// Resolves both file targets from [`LoadOptions`], failing when a default path
/// cannot be determined (no `--config`/`--secrets`, no env override, no
/// `HOME`/`XDG_CONFIG_HOME`).
///
/// This is the single place a CLI turns "where should I read/write?" into
/// concrete paths, honoring the same `--config` / `--secrets` / default
/// precedence the loader uses. The environment is injected (the snapshot
/// `src/bin` took): this helper never reads `std::env`.
pub fn resolve_targets(
    options: &LoadOptions,
    env: &dyn EnvLookup,
    paths: &PathResolver,
) -> Result<FileTargets, String> {
    let config =
        temper_config::config_path(options.config.clone(), paths, env).ok_or_else(|| {
            "cannot determine a default config path (no HOME); pass --config".to_string()
        })?;
    let credentials = temper_config::paired_credentials_path(
        options.credentials.clone(),
        options.config.clone(),
        paths,
        env,
    )
    .ok_or_else(|| {
        "cannot determine a default credentials path (no HOME); pass --secrets".to_string()
    })?;
    Ok(FileTargets {
        config,
        credentials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A writable temporary directory for tests, chosen without reading the
    /// process environment (this crate is a library and must stay env-free).
    fn temp_dir() -> PathBuf {
        PathBuf::from("/tmp")
    }

    #[test]
    fn write_new_file_refuses_to_clobber_without_force() {
        let dir = temp_dir().join(format!("temper-cli-common-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.txt");
        assert_eq!(
            write_new_file(&path, "x", false).expect("first write creates"),
            WriteOutcome::Created
        );
        let err = write_new_file(&path, "y", false).expect_err("second write refused");
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(
            write_new_file(&path, "z", true).expect("force overwrites"),
            WriteOutcome::Overwritten
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "z");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_tilde_passes_through_non_tilde() {
        let home = Path::new("/home/operator");
        assert_eq!(
            expand_tilde("/abs/path", Some(home)),
            PathBuf::from("/abs/path")
        );
        assert_eq!(
            expand_tilde("rel/path", Some(home)),
            PathBuf::from("rel/path")
        );
    }

    #[test]
    fn expand_tilde_uses_explicit_home() {
        let home = Path::new("/home/operator");
        assert_eq!(
            expand_tilde("~", Some(home)),
            PathBuf::from("/home/operator")
        );
        assert_eq!(
            expand_tilde("~/.config/temper", Some(home)),
            PathBuf::from("/home/operator/.config/temper")
        );
    }

    #[test]
    fn expand_tilde_without_home_is_verbatim() {
        assert_eq!(expand_tilde("~", None), PathBuf::from("~"));
        assert_eq!(
            expand_tilde("~/.config/temper", None),
            PathBuf::from("~/.config/temper")
        );
    }

    #[test]
    fn resolve_targets_treats_explicit_config_dir_as_bundle_root() {
        let bundle = temp_dir().join(format!("temper-cli-common-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bundle);

        let targets = resolve_targets(
            &LoadOptions {
                config: Some(bundle.clone()),
                credentials: None,
            },
            &temper_config::NoEnv,
            &PathResolver::default(),
        )
        .expect("explicit config root supplies both targets");

        assert_eq!(targets.config, bundle.join("config.toml"));
        assert_eq!(targets.credentials, bundle.join("credentials.toml"));
    }

    #[test]
    fn resolve_targets_treats_explicit_credentials_dir_as_credentials_toml() {
        let root = temp_dir().join(format!(
            "temper-cli-common-secrets-dir-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("deploy.toml");
        let secrets = root.join("secrets");

        let targets = resolve_targets(
            &LoadOptions {
                config: Some(config.clone()),
                credentials: Some(secrets.clone()),
            },
            &temper_config::NoEnv,
            &PathResolver::default(),
        )
        .expect("explicit paths supply both targets");

        assert_eq!(targets.config, config);
        assert_eq!(targets.credentials, secrets.join("credentials.toml"));
    }

    #[test]
    fn resolve_targets_preserves_explicit_toml_files() {
        let root = temp_dir().join(format!(
            "temper-cli-common-explicit-files-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("deploy.toml");
        let credentials = root.join("secret-values.toml");

        let targets = resolve_targets(
            &LoadOptions {
                config: Some(config.clone()),
                credentials: Some(credentials.clone()),
            },
            &temper_config::NoEnv,
            &PathResolver::default(),
        )
        .expect("explicit toml files supply both targets");

        assert_eq!(targets.config, config);
        assert_eq!(targets.credentials, credentials);
    }
}
