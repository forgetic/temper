#[cfg(feature = "test-provider-base-url-override")]
mod feature_enabled {
    use jig_core::{Reply, Script};
    use jig_server::FakeLlm;
    use smith_temper_agent::{AuthChoice, ProviderConfig};

    fn jig_auth_fixture() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json")
    }

    #[test]
    fn fake_llm_base_url_can_back_oauth_provider_config() {
        let fake = FakeLlm::start(Script::Fixed(Reply::text("hi"))).expect("start fake LLM");

        let config = ProviderConfig::anthropic_oauth(Some(jig_auth_fixture()))
            .with_base_url_override(fake.base_url());

        assert!(
            ProviderConfig::from_auth(AuthChoice::AnthropicOAuth, None, Some(jig_auth_fixture()))
                .is_ok()
        );
        assert!(config.build_provider().is_ok());
    }
}
