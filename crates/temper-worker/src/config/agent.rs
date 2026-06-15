use std::path::PathBuf;

use super::types::{AgentAuthChoice, AgentSurface, AnvilNativeAgentSurface};

/// The program name that selects the anvil-native out-of-process agent surface.
pub(super) const ANVIL_NATIVE_PROGRAM: &str = "anvil-native";
/// The agent binary the anvil-native surface spawns by default.
pub const TEMPER_AGENT_PROGRAM: &str = "temper-agent";

impl AnvilNativeAgentSurface {
    /// Renders the spawn command `OutOfProcessRunner` runs: the agent program
    /// followed by the same flags `anvil-agent` parses (`--auth`, `--auth-file`,
    /// `--codex-model`, `--config-dir`, `--max-iterations`, `--enable-subagents`).
    pub fn into_command(self) -> Vec<String> {
        let mut command = vec![self.agent_program];
        command.push("--auth".to_string());
        command.push(
            match self.auth {
                AgentAuthChoice::DeepSeek => "deepseek",
                AgentAuthChoice::ChatGptOAuth => "chatgpt-oauth",
                AgentAuthChoice::AnthropicOAuth => "anthropic-oauth",
            }
            .to_string(),
        );
        if let Some(codex_model) = self.codex_model {
            command.push("--codex-model".to_string());
            command.push(codex_model);
        }
        if let Some(auth_file) = self.auth_file {
            command.push("--auth-file".to_string());
            command.push(auth_file.to_string_lossy().into_owned());
        }
        if let Some(config_dir) = self.config_dir {
            command.push("--config-dir".to_string());
            command.push(config_dir.to_string_lossy().into_owned());
        }
        if let Some(max_iterations) = self.max_iterations {
            command.push("--max-iterations".to_string());
            command.push(max_iterations.to_string());
        }
        if self.enable_subagents {
            command.push("--enable-subagents".to_string());
        }
        command
    }
}

/// Builds the [`AgentSurface`] for the `--agent-command` program and its
/// trailing `--agent-arg` values. The `anvil-native` program name parses the
/// agent flags in-process; any other program is spawned as an external command.
pub(super) fn agent_surface(program: &str, args: Vec<String>) -> Result<AgentSurface, String> {
    if program == ANVIL_NATIVE_PROGRAM {
        Ok(AgentSurface::AnvilNative(parse_anvil_native_agent_surface(
            args,
        )?))
    } else {
        let mut command = Vec::with_capacity(args.len() + 1);
        command.push(program.to_string());
        command.extend(args);
        Ok(AgentSurface::ExternalCommand(command))
    }
}

/// Parses the anvil-native agent flags from the `--agent-arg` values — the
/// flags the `anvil-agent` binary parses for itself.
fn parse_anvil_native_agent_surface(args: Vec<String>) -> Result<AnvilNativeAgentSurface, String> {
    let mut agent_program = TEMPER_AGENT_PROGRAM.to_string();
    let mut auth = AgentAuthChoice::ChatGptOAuth;
    let mut codex_model = None;
    let mut auth_file = None;
    let mut config_dir = None;
    let mut max_iterations = None;
    let mut enable_subagents = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--enable-subagents" => {
                enable_subagents = true;
            }
            "--agent-program" => {
                agent_program = iter
                    .next()
                    .ok_or_else(|| "--agent-program requires a value".to_string())?;
            }
            "--auth" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--auth requires a value".to_string())?;
                auth = parse_agent_auth(&value)?;
            }
            "--codex-model" => {
                codex_model = Some(
                    iter.next()
                        .ok_or_else(|| "--codex-model requires a value".to_string())?,
                );
            }
            "--auth-file" => {
                auth_file = Some(PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--auth-file requires a value".to_string())?,
                ));
            }
            "--config-dir" => {
                config_dir =
                    Some(PathBuf::from(iter.next().ok_or_else(|| {
                        "--config-dir requires a value".to_string()
                    })?));
            }
            "--max-iterations" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--max-iterations requires a value".to_string())?;
                let parsed: usize = value.parse().map_err(|error| {
                    format!("--max-iterations must be a positive integer: {error}")
                })?;
                if parsed == 0 {
                    return Err("--max-iterations must be greater than zero".to_string());
                }
                max_iterations = Some(parsed);
            }
            other => {
                return Err(format!(
                    "unknown anvil-native agent arg `{other}`; expected --agent-program, --auth, --codex-model, --auth-file, --config-dir, --max-iterations, or --enable-subagents"
                ));
            }
        }
    }

    Ok(AnvilNativeAgentSurface {
        agent_program,
        auth,
        codex_model,
        auth_file,
        config_dir,
        max_iterations,
        enable_subagents,
    })
}

fn parse_agent_auth(value: &str) -> Result<AgentAuthChoice, String> {
    match value {
        "deepseek" => Ok(AgentAuthChoice::DeepSeek),
        "chatgpt-oauth" => Ok(AgentAuthChoice::ChatGptOAuth),
        "anthropic-oauth" => Ok(AgentAuthChoice::AnthropicOAuth),
        other => Err(format!(
            "unsupported --auth `{other}`; expected deepseek, chatgpt-oauth, or anthropic-oauth"
        )),
    }
}
