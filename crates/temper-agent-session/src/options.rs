// SPDX-License-Identifier: MPL-2.0

//! Command-line option parsing for the `temper-agent` binary.
//!
//! Every non-secret input is a flag (the sole secret, the provider credential,
//! arrives via `TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON`). The worker sets these
//! when it spawns the agent; an operator can set them by hand for debugging:
//!
//! ```text
//! --context <FILE>            (required) worker-written JSON job context
//! --result <FILE>             (required) JSON result file the agent must write
//! --workspace <DIR>           checkout/workspace; defaults to cwd
//! --submit-for-pr-address <ADDR>
//!                             worker-owned local submit_for_pr side channel
//! --provider <anthropic|chatgpt|deepseek>
//! --model <ID>                main model id
//! --investigate-model <ID>    cheaper read-only subagent model id
//! --provider-url <URL>        provider base URL override
//! --max-iterations <N>        maximum model/tool iterations
//! --subagents <on|off>        enable investigate/read-only subagents
//! --capture-dir <DIR>         optional debug capture / prompt-overlay dir
//! ```

use std::path::PathBuf;

use temper_agent::{AuthChoice, DEFAULT_MAX_ITERATIONS};
use temper_protocol_agent::SUBMIT_FOR_PR_ADDRESS_FLAG;

/// The fully-parsed agent command line. Every field originates from a flag; the
/// provider credential (the one secret) is read separately from the environment
/// in [`crate::entry`].
pub(crate) struct Options {
    /// Provider adapter (`--provider`), as the agent's [`AuthChoice`].
    pub(crate) provider: AuthChoice,
    /// Main model id (`--model`), if overridden.
    pub(crate) model: Option<String>,
    /// Cheaper read-only subagent model id (`--investigate-model`).
    pub(crate) investigate_model: Option<String>,
    /// Provider base URL override (`--provider-url`).
    pub(crate) provider_url: Option<String>,
    /// Worker-written context JSON path (`--context`, required).
    pub(crate) context: PathBuf,
    /// Result JSON path the agent must write (`--result`, required).
    pub(crate) result: PathBuf,
    /// Checkout/workspace dir (`--workspace`); `None` defaults to cwd.
    pub(crate) workspace: Option<PathBuf>,
    /// Optional worker-owned local `submit_for_pr` side-channel address.
    pub(crate) submit_for_pr_address: Option<String>,
    /// Optional debug capture / prompt-overlay dir (`--capture-dir`).
    pub(crate) capture_dir: Option<PathBuf>,
    /// Maximum model/tool iterations (`--max-iterations`).
    pub(crate) max_iterations: usize,
    /// Whether investigate/read-only subagents are enabled (`--subagents`).
    pub(crate) subagents: bool,
}

impl Options {
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut provider = AuthChoice::ChatGptOAuth;
        let mut model = None;
        let mut investigate_model = None;
        let mut provider_url = None;
        let mut context = None;
        let mut result = None;
        let mut workspace = None;
        let mut submit_for_pr_address = None;
        let mut capture_dir = None;
        let mut max_iterations = DEFAULT_MAX_ITERATIONS;
        let mut subagents = false;

        let mut iter = args.into_iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--provider" => provider = parse_provider(&value(&mut iter, "--provider")?)?,
                "--model" => model = Some(value(&mut iter, "--model")?),
                "--investigate-model" => {
                    investigate_model = Some(value(&mut iter, "--investigate-model")?)
                }
                "--provider-url" => provider_url = Some(value(&mut iter, "--provider-url")?),
                "--context" => context = Some(PathBuf::from(value(&mut iter, "--context")?)),
                "--result" => result = Some(PathBuf::from(value(&mut iter, "--result")?)),
                "--workspace" => workspace = Some(PathBuf::from(value(&mut iter, "--workspace")?)),
                flag if flag == SUBMIT_FOR_PR_ADDRESS_FLAG => {
                    submit_for_pr_address = Some(value(&mut iter, SUBMIT_FOR_PR_ADDRESS_FLAG)?)
                }
                "--capture-dir" => {
                    capture_dir = Some(PathBuf::from(value(&mut iter, "--capture-dir")?))
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
                "--subagents" => subagents = parse_toggle(&value(&mut iter, "--subagents")?)?,
                "--help" | "-h" => return Ok(None),
                other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
            }
        }

        let context = context.ok_or_else(|| format!("missing required --context\n{USAGE}"))?;
        let result = result.ok_or_else(|| format!("missing required --result\n{USAGE}"))?;

        Ok(Some(Self {
            provider,
            model,
            investigate_model,
            provider_url,
            context,
            result,
            workspace,
            submit_for_pr_address,
            capture_dir,
            max_iterations,
            subagents,
        }))
    }
}

pub(crate) const USAGE: &str = "temper agent --context <FILE> --result <FILE> [--workspace <DIR>] \
[--submit-for-pr-address <ADDR>] [--provider <anthropic|chatgpt|deepseek>] [--model <ID>] [--investigate-model <ID>] \
[--provider-url <URL>] [--max-iterations <N>] [--subagents <on|off>] [--capture-dir <DIR>]\n  \
reads the provider credential from $TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON, runs in \
--workspace (default cwd), writes the result to --result";

fn value(iter: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    iter.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// Maps the `--provider` value onto the agent's [`AuthChoice`].
fn parse_provider(value: &str) -> Result<AuthChoice, String> {
    match value {
        "deepseek" => Ok(AuthChoice::DeepSeek),
        "chatgpt" => Ok(AuthChoice::ChatGptOAuth),
        "anthropic" => Ok(AuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unknown --provider `{other}` (expected anthropic|chatgpt|deepseek)"
        )),
    }
}

/// Parses an `on`/`off` toggle.
fn parse_toggle(value: &str) -> Result<bool, String> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("expected `on` or `off`, got `{other}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        match parse_raw(args)? {
            Some(options) => Ok(options),
            None => Err("unexpected help request".to_string()),
        }
    }

    fn parse_raw(args: &[&str]) -> Result<Option<Options>, String> {
        Options::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn help_short_circuits_required_run_inputs() {
        assert!(parse_raw(&["--help"]).expect("help parses").is_none());
        assert!(parse_raw(&["-h"]).expect("help parses").is_none());
        assert!(USAGE.contains("temper agent --context <FILE> --result <FILE>"));
        assert!(USAGE.contains("TEMPER_AGENT_PROVIDER_CREDENTIALS_JSON"));
    }

    #[test]
    fn parses_minimal_required_flags() {
        let options =
            parse(&["--context", "/c.json", "--result", "/r.json"]).expect("parse minimal");
        assert_eq!(options.context, PathBuf::from("/c.json"));
        assert_eq!(options.result, PathBuf::from("/r.json"));
        assert!(options.workspace.is_none());
        assert!(options.submit_for_pr_address.is_none());
        assert_eq!(options.provider, AuthChoice::ChatGptOAuth);
        assert!(!options.subagents);
    }

    #[test]
    fn missing_context_or_result_is_an_error() {
        assert!(parse(&["--result", "/r.json"]).is_err());
        assert!(parse(&["--context", "/c.json"]).is_err());
    }

    #[test]
    fn parses_the_full_flag_set() {
        let options = parse(&[
            "--context",
            "/c.json",
            "--result",
            "/r.json",
            "--workspace",
            "/ws",
            "--submit-for-pr-address",
            "127.0.0.1:12345",
            "--provider",
            "anthropic",
            "--model",
            "claude-opus-4-8",
            "--investigate-model",
            "claude-haiku-4-5",
            "--provider-url",
            "http://fake-llm",
            "--max-iterations",
            "250",
            "--subagents",
            "on",
            "--capture-dir",
            "/cap",
        ])
        .expect("parse full");
        assert_eq!(options.provider, AuthChoice::AnthropicOAuth);
        assert_eq!(options.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(
            options.investigate_model.as_deref(),
            Some("claude-haiku-4-5")
        );
        assert_eq!(options.provider_url.as_deref(), Some("http://fake-llm"));
        assert_eq!(options.workspace, Some(PathBuf::from("/ws")));
        assert_eq!(
            options.submit_for_pr_address.as_deref(),
            Some("127.0.0.1:12345")
        );
        assert_eq!(options.capture_dir, Some(PathBuf::from("/cap")));
        assert_eq!(options.max_iterations, 250);
        assert!(options.subagents);
    }

    #[test]
    fn provider_maps_to_auth_choice() {
        assert_eq!(parse_provider("deepseek").unwrap(), AuthChoice::DeepSeek);
        assert_eq!(parse_provider("chatgpt").unwrap(), AuthChoice::ChatGptOAuth);
        assert_eq!(
            parse_provider("anthropic").unwrap(),
            AuthChoice::AnthropicOAuth
        );
        assert!(parse_provider("bogus").is_err());
    }

    #[test]
    fn subagents_toggle_parses_on_off() {
        assert!(parse_toggle("on").unwrap());
        assert!(!parse_toggle("off").unwrap());
        assert!(parse_toggle("yes").is_err());
    }
}
