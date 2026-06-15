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
//! also depend on `temper-config` just for `--config`/`--credentials` parsing.

mod prompt;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub use prompt::{Prompter, ScriptedPrompter, TerminalPrompter};

// Re-exported from `temper-config` so subcommand crates parse the common flags
// the same way without each depending on `temper-config` directly.
pub use temper_config::{CommonArgs, EX_USAGE, LoadOptions, parse_common_args};

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
            eprintln!("{prefix}: {error}");
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

/// Expands a leading `~` / `~/…` in `value` to the user's home directory.
///
/// Anything else (an absolute path, a relative path, a `~user` form) is taken
/// verbatim. Used so an operator can type `~/.config/temper` and have it land
/// where they expect.
pub fn expand_tilde(value: &str) -> PathBuf {
    if value == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = value.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
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
/// cannot be determined (no `--config`/`--credentials`, no env override, no
/// `HOME`/`XDG_CONFIG_HOME`).
///
/// This is the single place a CLI turns "where should I read/write?" into
/// concrete paths, honoring the same `--config` / `--credentials` / env /
/// default precedence the loader uses.
pub fn resolve_targets(options: &LoadOptions) -> Result<FileTargets, String> {
    let config = temper_config::config_path(options.config.clone()).ok_or_else(|| {
        "cannot determine a default config path (no HOME); pass --config".to_string()
    })?;
    let credentials =
        temper_config::credentials_path(options.credentials.clone()).ok_or_else(|| {
            "cannot determine a default credentials path (no HOME); pass --credentials".to_string()
        })?;
    Ok(FileTargets {
        config,
        credentials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_new_file_refuses_to_clobber_without_force() {
        let dir = std::env::temp_dir().join(format!("temper-cli-common-{}", std::process::id()));
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
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/path"), PathBuf::from("rel/path"));
    }
}
