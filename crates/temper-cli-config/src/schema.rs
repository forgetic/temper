// SPDX-License-Identifier: MPL-2.0

//! `temper config schema` JSON Schema export command.

use std::process::ExitCode;

/// Runs `temper config schema`.
///
/// The schema command intentionally emits JSON for every global output format:
/// it is a machine-readable inspection command, and keeping the human/default
/// path identical makes it safe for tooling to call without extra flags.
pub(crate) fn command(args: &[String]) -> Result<ExitCode, String> {
    super::parse_options(args, false)?;
    let rendered = serde_json::to_string_pretty(&temper_config::config_json_schema())
        .map_err(|error| format!("render config schema JSON: {error}"))?;
    println!("{rendered}");
    Ok(ExitCode::SUCCESS)
}
