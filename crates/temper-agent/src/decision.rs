//! Running a one-shot LLM decision on anvil's native agent loop.
//!
//! [`run_decision`] runs one bounded [`temper_agent_core::SubAgent`] turn with the
//! role system prompt and the work-item context as the user message, collects
//! the assistant's text, and parses it into a structured decision `D`. No
//! tools are registered: the agent must answer in one turn with a single JSON
//! object, keeping all workflow mutation in the calling adapter (the
//! authority boundary).
//!
//! Must be awaited inside a skein engine task (the sub-agent's drive loop
//! reads the runtime clock and its shell spawns I/O) — see
//! `temper_agent_io::block_on`.

use serde::de::DeserializeOwned;
use tongs::model::ContentBlock;

use crate::provider::{ProviderConfig, ProviderError};

/// Why a decision could not be obtained.
#[derive(Debug)]
pub enum DecisionError {
    /// Building the provider or loading the key failed (a setup error).
    Provider(ProviderError),
    /// The agent run failed (network, provider rejection, abort).
    Run(String),
    /// The model returned no text content.
    Empty,
    /// The model's text was not the expected JSON decision.
    Parse { snippet: String, error: String },
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecisionError::Provider(error) => write!(formatter, "{error}"),
            DecisionError::Run(message) => write!(formatter, "LLM run failed: {message}"),
            DecisionError::Empty => formatter.write_str("LLM returned no text content"),
            DecisionError::Parse { snippet, error } => {
                write!(
                    formatter,
                    "could not parse LLM decision ({error}): {snippet}"
                )
            }
        }
    }
}

impl std::error::Error for DecisionError {}

impl From<ProviderError> for DecisionError {
    fn from(error: ProviderError) -> Self {
        Self::Provider(error)
    }
}

/// Maximum tool iterations. We register no tools, so one model turn suffices;
/// keep a small ceiling as a guard.
const MAX_TOOL_ITERATIONS: usize = 1;

/// Runs one LLM turn and parses the reply into `D`.
pub async fn run_decision<D: DeserializeOwned>(
    handle: skein::runtime::RuntimeHandle,
    provider_config: &ProviderConfig,
    system_prompt: &str,
    user_context: &str,
) -> Result<D, DecisionError> {
    let provider = provider_config.build_provider()?;

    // Anthropic's Claude subscription OAuth path rejects any request whose first
    // `system` block is not exactly the Claude Code identity (HTTP 429
    // rate_limit_error). The SDK sends `system` as a single string, so for that
    // mode we send the identity as the system prompt and fold the role prompt
    // into the user turn; every other mode keeps the role prompt as `system`.
    let (effective_system, effective_user) = match provider_config.required_system_identity() {
        Some(identity) => (
            identity.to_string(),
            format!("{system_prompt}\n\n{user_context}"),
        ),
        None => (system_prompt.to_string(), user_context.to_string()),
    };

    // The provider authenticates from the per-request bearer carried in stream
    // options; never bake it into the provider object. For ChatGPT OAuth the
    // bearer is resolved (and refreshed if near expiry) fresh on each decision.
    // Mode-specific knobs: API-key (DeepSeek) pins temperature 0.0 and no
    // reasoning; the codex reasoning models leave temperature unset and request
    // the lowest supported reasoning effort; Anthropic OAuth injects Claude
    // Code-compatible identity headers through the per-request header path.
    let stream_options = tongs::provider::StreamOptions {
        api_key: Some(provider_config.resolve_bearer().await?),
        temperature: provider_config.temperature(),
        thinking_level: provider_config.thinking_level(),
        headers: provider_config.request_headers(),
        ..tongs::provider::StreamOptions::default()
    };

    // No tools: the registry is empty, so the model cannot reach bash/file
    // tools — the workflow mutation path stays exclusively in the adapter.
    let outcome = temper_agent_core::run_sub_agent(
        handle,
        temper_agent_core::SubAgent {
            system_prompt: Some(effective_system),
            user_message: effective_user,
            tools: tongs::tools::ToolRegistry::from_tools(Vec::new()),
            max_iterations: MAX_TOOL_ITERATIONS,
            provider,
            stream_options,
        },
    )
    .await
    .map_err(|error| DecisionError::Run(error.to_string()))?;
    if matches!(outcome.stop, temper_agent_core::AgentStop::ModelError) {
        return Err(DecisionError::Run(
            outcome
                .final_message
                .error_message
                .clone()
                .unwrap_or_else(|| "provider reported an error stop".to_string()),
        ));
    }

    let text = collect_text(&outcome.final_message.content);
    if text.trim().is_empty() {
        return Err(DecisionError::Empty);
    }
    parse_decision(&text)
}

/// Concatenates the assistant message's text blocks (ignoring thinking/tool
/// blocks).
fn collect_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Parses the model's reply into `D`, tolerating a code-fenced or prose-wrapped
/// JSON object by extracting the first balanced `{...}` span.
fn parse_decision<D: DeserializeOwned>(text: &str) -> Result<D, DecisionError> {
    let candidate = extract_json_object(text).unwrap_or_else(|| text.trim().to_string());
    serde_json::from_str::<D>(&candidate).map_err(|error| DecisionError::Parse {
        snippet: snippet(text),
        error: error.to_string(),
    })
}

/// Returns the first balanced top-level `{...}` substring, if any.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=start + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// A short, single-line snippet of the model reply for error messages. The reply
/// is the model's own text and carries no secrets.
fn snippet(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() > 200 {
        format!("{}…", &collapsed[..200])
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Eq, Deserialize)]
    #[serde(rename_all = "snake_case", tag = "action")]
    enum TestDecision {
        DoThing,
        NoAction,
    }

    #[test]
    fn parses_bare_json() {
        let decision: TestDecision =
            parse_decision(r#"{"action":"do_thing","reason":"x"}"#).unwrap();
        assert_eq!(decision, TestDecision::DoThing);
    }

    #[test]
    fn parses_code_fenced_json() {
        let text = "Here is my answer:\n```json\n{\"action\": \"no_action\"}\n```\n";
        let decision: TestDecision = parse_decision(text).unwrap();
        assert_eq!(decision, TestDecision::NoAction);
    }

    #[test]
    fn parses_json_with_surrounding_prose() {
        let text = "I think {\"action\": \"do_thing\"} is right.";
        let decision: TestDecision = parse_decision(text).unwrap();
        assert_eq!(decision, TestDecision::DoThing);
    }

    #[test]
    fn rejects_non_json() {
        let result = parse_decision::<TestDecision>("I cannot decide.");
        assert!(matches!(result, Err(DecisionError::Parse { .. })));
    }

    #[test]
    fn extract_handles_nested_braces_and_strings() {
        let text = r#"prefix {"a": {"b": "}"}, "c": 1} suffix"#;
        let extracted = extract_json_object(text).unwrap();
        assert_eq!(extracted, r#"{"a": {"b": "}"}, "c": 1}"#);
    }
}
