// SPDX-License-Identifier: MPL-2.0

//! Command-line option parsing for the `temper-agent` binary.
//!
//! Auth/iteration knobs come from flags, mirroring the former in-process runner:
//! `--auth <deepseek|chatgpt-oauth|anthropic-oauth>` `--auth-file <path>`
//! `--codex-model <id>` `--max-iterations <n>` `--config-dir <path>`
//! `--enable-subagents`.

use std::path::PathBuf;

use temper_agent::{AuthChoice, DEFAULT_MAX_ITERATIONS};

pub(crate) struct Options {
    pub(crate) auth: AuthChoice,
    pub(crate) codex_model: Option<String>,
    pub(crate) auth_file: Option<PathBuf>,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) max_iterations: usize,
    pub(crate) enable_subagents: bool,
}

impl Options {
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut auth = AuthChoice::ChatGptOAuth;
        let mut codex_model = None;
        let mut auth_file = None;
        let mut config_dir = None;
        let mut max_iterations = DEFAULT_MAX_ITERATIONS;
        let mut enable_subagents = false;

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--auth" => auth = parse_auth(&value(&mut iter, "--auth")?)?,
                "--codex-model" => codex_model = Some(value(&mut iter, "--codex-model")?),
                "--auth-file" => auth_file = Some(PathBuf::from(value(&mut iter, "--auth-file")?)),
                "--config-dir" => {
                    config_dir = Some(PathBuf::from(value(&mut iter, "--config-dir")?))
                }
                "--max-iterations" => {
                    let raw = value(&mut iter, "--max-iterations")?;
                    max_iterations = raw.parse::<usize>().map_err(|_| {
                        format!("--max-iterations expects a positive integer, got `{raw}`")
                    })?;
                    if max_iterations == 0 {
                        return Err("--max-iterations must be greater than zero".to_string());
                    }
                }
                "--enable-subagents" => enable_subagents = true,
                "--help" | "-h" => return Err(USAGE.to_string()),
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }

        Ok(Self {
            auth,
            codex_model,
            auth_file,
            config_dir,
            max_iterations,
            enable_subagents,
        })
    }
}

const USAGE: &str = "temper-agent [--auth <deepseek|chatgpt-oauth|anthropic-oauth>] [--auth-file <path>] [--codex-model <id>] [--max-iterations <n>] [--config-dir <path>] [--enable-subagents]\n  reads context from $TEMPER_CODING_WORKSPACE_CONTEXT, runs in cwd, writes result to $TEMPER_CODING_WORKSPACE_RESULT, emits step-progress JSON lines on stdout";

fn value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_auth(value: &str) -> Result<AuthChoice, String> {
    match value {
        "deepseek" => Ok(AuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unknown --auth `{other}` (expected deepseek|chatgpt-oauth|anthropic-oauth)"
        )),
    }
}
