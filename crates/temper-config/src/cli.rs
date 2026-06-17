// SPDX-License-Identifier: MPL-2.0

//! Shared parsing for the `--config` / `--credentials` / `--secrets` flags every
//! temper binary accepts, so the slim per-service binaries stay one-liners and
//! the unified binary parses these the same way.

use std::path::PathBuf;

use crate::LoadOptions;

/// The common flags parsed off a binary's argument list.
#[derive(Debug, Clone, Default)]
pub struct CommonArgs {
    /// `--config` / `--credentials` / `--secrets` paths.
    pub options: LoadOptions,
    /// `-h` / `--help` was given.
    pub help: bool,
    /// `-V` / `--version` was given.
    pub version: bool,
    /// Tokens not recognized here (e.g. `--service <name>` for the unified
    /// daemon), preserved in order for the caller to interpret.
    pub rest: Vec<String>,
}

/// Parses `--config <path>`, `--credentials <path>`, `--secrets <path>`,
/// `-h`/`--help`, and `-V`/`--version`, collecting everything else into
/// [`CommonArgs::rest`].
pub fn parse_common_args(args: impl IntoIterator<Item = String>) -> Result<CommonArgs, String> {
    let mut parsed = CommonArgs::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => {
                parsed.options.config = Some(PathBuf::from(value(&mut iter, "--config")?));
            }
            "--credentials" | "--secrets" => {
                parsed.options.credentials = Some(PathBuf::from(value(&mut iter, &arg)?));
            }
            "-h" | "--help" => parsed.help = true,
            "-V" | "--version" => parsed.version = true,
            other => parsed.rest.push(other.to_string()),
        }
    }
    Ok(parsed)
}

fn value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| format!("{flag} requires a value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_alias_populates_credentials_option() {
        let parsed = parse_common_args([
            "--secrets".to_string(),
            "local/credentials.toml".to_string(),
        ])
        .expect("parse succeeds");

        assert_eq!(
            parsed.options.credentials,
            Some(PathBuf::from("local/credentials.toml"))
        );
    }

    #[test]
    fn credentials_flag_still_populates_credentials_option() {
        let parsed = parse_common_args([
            "--credentials".to_string(),
            "legacy/credentials.toml".to_string(),
        ])
        .expect("parse succeeds");

        assert_eq!(
            parsed.options.credentials,
            Some(PathBuf::from("legacy/credentials.toml"))
        );
    }

    #[test]
    fn missing_secrets_value_names_alias() {
        let err = parse_common_args(["--secrets".to_string()]).expect_err("missing value errors");

        assert_eq!(err, "--secrets requires a value");
    }
}
