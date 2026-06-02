use super::*;

#[test]
fn deepseek_mode_targets_openai_completions() {
    let config = ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
    let entry = config.model_entry();
    assert_eq!(entry.model.provider, "deepseek");
    assert_eq!(entry.model.api, OPENAI_COMPLETIONS_API);
    assert!(!entry.model.reasoning);
    assert_eq!(config.temperature(), Some(0.0));
    assert!(config.thinking_level().is_none());
    assert!(config.build_provider().is_ok());
}

#[test]
fn chatgpt_oauth_mode_targets_codex_route() {
    let config = ProviderConfig::chatgpt_oauth_from_env();
    let entry = config.model_entry();
    assert_eq!(entry.model.provider, CODEX_PROVIDER_ID);
    assert!(entry.model.reasoning);
    assert_eq!(config.model_id(), DEFAULT_CODEX_MODEL);
    assert!(config.temperature().is_none());
    assert_eq!(config.thinking_level(), Some(ThinkingLevel::Low));
    assert!(config.build_provider().is_ok());
}

#[test]
fn chatgpt_oauth_applies_model_override() {
    let config = ProviderConfig::chatgpt_oauth(Some("gpt-5.9-codex".to_string()), None);
    assert_eq!(config.model_id(), "gpt-5.9-codex");
}

#[test]
fn anthropic_oauth_requires_claude_code_system_identity() {
    let config = ProviderConfig::anthropic_oauth_from_env();
    assert_eq!(
        config.required_system_identity(),
        Some("You are Claude Code, Anthropic's official CLI for Claude.")
    );
    let deepseek = ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
    assert_eq!(deepseek.required_system_identity(), None);
    assert_eq!(
        ProviderConfig::chatgpt_oauth_from_env().required_system_identity(),
        None
    );
}

#[test]
fn anthropic_oauth_mode_targets_anthropic_messages_route() {
    let config = ProviderConfig::anthropic_oauth_from_env();
    let entry = config.model_entry();
    assert_eq!(entry.model.provider, ANTHROPIC_PROVIDER_ID);
    assert_eq!(entry.model.api, ANTHROPIC_MESSAGES_API);
    assert_eq!(entry.model.base_url, ANTHROPIC_BASE_URL);
    assert!(entry.model.reasoning);
    assert_eq!(entry.model.input, vec![InputType::Text, InputType::Image]);
    assert_eq!(entry.model.context_window, 1_000_000);
    assert_eq!(entry.model.max_tokens, 128_000);
    assert_eq!(config.model_id(), DEFAULT_ANTHROPIC_MODEL);
    assert!(config.temperature().is_none());
    assert!(config.thinking_level().is_none());
    assert!(config.build_provider().is_ok());
}

#[test]
fn from_auth_oauth_preflights_missing_login() {
    let missing = std::env::temp_dir().join(format!(
        "harness-agents-absent-{}-{}.json",
        std::process::id(),
        "from-auth"
    ));
    let _ = std::fs::remove_file(&missing);
    let error = ProviderConfig::from_auth(AuthChoice::ChatGptOAuth, None, Some(missing))
        .expect_err("missing login must fail the preflight");
    assert!(matches!(error, ProviderError::OAuthUnavailable(_)));
    assert!(format!("{error}").contains("openai-codex"));
}

#[test]
fn from_auth_anthropic_oauth_preflights_missing_login() {
    let missing = std::env::temp_dir().join(format!(
        "harness-agents-absent-{}-{}.json",
        std::process::id(),
        "from-auth-anthropic"
    ));
    let _ = std::fs::remove_file(&missing);
    let error = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, Some(missing))
        .expect_err("missing Anthropic login must fail the preflight");
    assert!(matches!(error, ProviderError::AnthropicOAuthUnavailable(_)));
    assert!(format!("{error}").contains("pi /login anthropic"));
}

#[test]
fn from_auth_anthropic_oauth_preflights_present_login() {
    let path = std::env::temp_dir().join(format!(
        "harness-agents-present-{}-{}.json",
        std::process::id(),
        "from-auth-anthropic"
    ));
    let contents = serde_json::json!({
        "anthropic": {
            "type": "oauth",
            "access": "sk-ant-oat-access",
            "refresh": "refresh-token",
            "expires": 4_102_444_800_000_i64,
        }
    })
    .to_string();
    std::fs::write(&path, contents).expect("write auth fixture");
    let result = ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, Some(path.clone()));
    let _ = std::fs::remove_file(&path);
    assert!(result.is_ok(), "present Anthropic login should preflight");
}

#[test]
fn debug_redacts_api_key() {
    let config = ProviderConfig::new("deepseek", "deepseek-chat", DEFAULT_BASE_URL, "sk-secret");
    let rendered = format!("{config:?}");
    assert!(rendered.contains("<redacted>"));
    assert!(!rendered.contains("sk-secret"));
    assert!(rendered.contains("api_key"));
}

#[test]
fn debug_redacts_anthropic_oauth() {
    let config = ProviderConfig::anthropic_oauth(None);
    let rendered = format!("{config:?}");
    assert!(rendered.contains("<redacted>"));
    assert!(rendered.contains("anthropic_oauth"));
    assert!(!rendered.contains("sk-ant"));
}
