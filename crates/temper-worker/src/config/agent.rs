use std::path::PathBuf;

use super::types::{AgentProviderChoice, AgentSurface, AnvilNativeAgentSurface};

/// The program name that selects the anvil-native out-of-process agent surface.
pub(super) const ANVIL_NATIVE_PROGRAM: &str = "anvil-native";
/// The agent binary the anvil-native surface spawns by default.
pub const TEMPER_AGENT_PROGRAM: &str = "temper-agent";

impl AnvilNativeAgentSurface {
    /// Renders the spawn command `OutOfProcessRunner` runs: the agent program
    /// followed by the flags the agent parses (`--provider`, `--model`,
    /// `--capture-dir`, `--max-iterations`, `--subagents`, and optional
    /// `--checkpoints`). The per-job `--context`/`--result`/`--workspace` flags
    /// are appended by the runner; the provider credential is injected as the
    /// one secret env var.
    pub fn into_command(self) -> Vec<String> {
        let mut command = vec![self.agent_program];
        command.push("--provider".to_string());
        command.push(
            match self.provider {
                AgentProviderChoice::DeepSeek => "deepseek",
                AgentProviderChoice::ChatGpt => "chatgpt",
                AgentProviderChoice::Anthropic => "anthropic",
            }
            .to_string(),
        );
        if let Some(model) = self.model {
            command.push("--model".to_string());
            command.push(model);
        }
        if let Some(capture_dir) = self.capture_dir {
            command.push("--capture-dir".to_string());
            command.push(capture_dir.to_string_lossy().into_owned());
        }
        if let Some(max_iterations) = self.max_iterations {
            command.push("--max-iterations".to_string());
            command.push(max_iterations.to_string());
        }
        command.push("--subagents".to_string());
        command.push(if self.enable_subagents { "on" } else { "off" }.to_string());
        if self.enable_checkpoints {
            command.push("--checkpoints".to_string());
            command.push("on".to_string());
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
/// flags the agent binary parses for itself.
fn parse_anvil_native_agent_surface(args: Vec<String>) -> Result<AnvilNativeAgentSurface, String> {
    let mut agent_program = TEMPER_AGENT_PROGRAM.to_string();
    let mut provider = AgentProviderChoice::ChatGpt;
    let mut model = None;
    let mut capture_dir = None;
    let mut max_iterations = None;
    let mut enable_subagents = false;
    let mut enable_checkpoints = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--agent-program" => {
                agent_program = iter
                    .next()
                    .ok_or_else(|| "--agent-program requires a value".to_string())?;
            }
            "--provider" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--provider requires a value".to_string())?;
                provider = parse_agent_provider(&value)?;
            }
            "--model" => {
                model = Some(
                    iter.next()
                        .ok_or_else(|| "--model requires a value".to_string())?,
                );
            }
            "--capture-dir" => {
                capture_dir =
                    Some(PathBuf::from(iter.next().ok_or_else(|| {
                        "--capture-dir requires a value".to_string()
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
            "--subagents" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--subagents requires a value".to_string())?;
                enable_subagents = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    other => {
                        return Err(format!("--subagents expects `on` or `off`, got `{other}`"));
                    }
                };
            }
            "--checkpoints" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--checkpoints requires a value".to_string())?;
                enable_checkpoints = match value.as_str() {
                    "on" => true,
                    "off" => false,
                    other => {
                        return Err(format!("--checkpoints expects `on` or `off`, got `{other}`"));
                    }
                };
            }
            other => {
                return Err(format!(
                    "unknown anvil-native agent arg `{other}`; expected --agent-program, --provider, --model, --capture-dir, --max-iterations, --subagents, or --checkpoints"
                ));
            }
        }
    }

    Ok(AnvilNativeAgentSurface {
        agent_program,
        provider,
        model,
        capture_dir,
        max_iterations,
        enable_subagents,
        enable_checkpoints,
    })
}

fn parse_agent_provider(value: &str) -> Result<AgentProviderChoice, String> {
    match value {
        "deepseek" => Ok(AgentProviderChoice::DeepSeek),
        "chatgpt" => Ok(AgentProviderChoice::ChatGpt),
        "anthropic" => Ok(AgentProviderChoice::Anthropic),
        other => Err(format!(
            "unsupported --provider `{other}`; expected deepseek, chatgpt, or anthropic"
        )),
    }
}
