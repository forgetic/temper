//! SDK [`ModelEntry`] construction for a [`ProviderConfig`].
//!
//! Maps the resolved auth mode to the per-provider model parameters (API route,
//! reasoning flag, accepted input types, cost, context window, output cap) the
//! tongs provider factory consumes. The bearer is carried per request through
//! the agent's `stream_options`, never baked into the entry.

use std::collections::HashMap;

use tongs::model::{InputType, Model, ModelCost};
use tongs::provider::ModelEntry;

use super::anthropic_oauth;
use super::auth::AuthMode;
use super::{ANTHROPIC_MESSAGES_API, CODEX_RESPONSES_API, OPENAI_COMPLETIONS_API, ProviderConfig};

/// Builds the [`ModelEntry`] the SDK factory consumes for `config`.
pub(super) fn build_model_entry(config: &ProviderConfig) -> ModelEntry {
    let model_id = config.model_id();
    let (api, reasoning, input, cost, context_window, max_tokens) = match config.auth() {
        AuthMode::ChatGptOAuth { .. } => {
            // Codex models are reasoning models; the codex route sends no
            // explicit `max_output_tokens` (the model decides) and reads the
            // reasoning effort from `stream_options.thinking_level`. A
            // generous context window matches the gpt-5.x context.
            (
                CODEX_RESPONSES_API,
                true,
                vec![InputType::Text],
                zero_cost(),
                400_000,
                0,
            )
        }
        AuthMode::AnthropicOAuth { .. } => {
            // Anthropic Opus 4.x is a reasoning-capable, multimodal model,
            // but the initial OAuth path deliberately sends no explicit
            // thinking level until live validation proves the SDK's legacy
            // thinking-body shape is compatible with this model.
            //
            // `max_tokens` must not exceed the *model's* output ceiling: the
            // API rejects an over-cap request with a 400
            // `invalid_request_error`, and since the sub-agent tier runs a
            // smaller model (e.g. Haiku, capped at 64K vs Opus' 128K) a
            // single hard-coded cap would break every sub-agent request.
            (
                ANTHROPIC_MESSAGES_API,
                true,
                vec![InputType::Text, InputType::Image],
                ModelCost {
                    input: 15.0,
                    output: 75.0,
                    cache_read: 1.5,
                    cache_write: 18.75,
                },
                anthropic_oauth::context_window_for(model_id),
                anthropic_oauth::max_output_tokens_for(model_id),
            )
        }
        AuthMode::ApiKey { .. } => (
            OPENAI_COMPLETIONS_API,
            false,
            vec![InputType::Text],
            zero_cost(),
            64_000,
            8_192,
        ),
    };
    let model = Model {
        id: model_id.to_string(),
        name: model_id.to_string(),
        api: api.to_string(),
        provider: config.provider_id().to_string(),
        base_url: config.base_url().to_string(),
        reasoning,
        input,
        cost,
        context_window,
        max_tokens,
        headers: HashMap::new(),
    };
    ModelEntry {
        model,
        // The bearer flows through the agent's `stream_options.api_key`, not
        // the entry, so it is not duplicated here.
        api_key: None,
        headers: HashMap::new(),
        auth_header: true,
        compat: None,
        oauth_config: None,
    }
}

fn zero_cost() -> ModelCost {
    ModelCost {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
    }
}
