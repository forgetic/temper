//! Role→capability mapping, writability, and sub-agent registration.

use crate::coding_agent::*;

#[test]
fn role_maps_to_capability() {
    assert_eq!(
        Capability::for_role("engineer"),
        Capability::CodingWorkspace
    );
    assert_eq!(
        Capability::for_role("reviewer"),
        Capability::ReviewWorkspace
    );
    assert_eq!(
        Capability::for_role("architect"),
        Capability::TriageWorkspace
    );
    // Unknown roles fall back to read-only triage; they must never be writable.
    assert_eq!(Capability::for_role("mystery"), Capability::TriageWorkspace);
    assert!(!Capability::for_role("mystery").is_writable());
}

#[test]
fn only_engineer_is_writable() {
    assert!(Capability::CodingWorkspace.is_writable());
    assert!(!Capability::TriageWorkspace.is_writable());
    assert!(!Capability::ReviewWorkspace.is_writable());
}

#[test]
fn two_subagent_roles_with_distinct_tiers_and_tools() {
    // Mirrors Claude's Explore (cheap, read-only) + general-purpose (main model,
    // has bash) split: the orchestrator chooses the role, the role fixes the
    // model — the LLM never picks a model directly.
    let specs = subagent_specs();
    assert_eq!(specs.len(), 2);

    let investigate = specs
        .iter()
        .find(|s| s.name == "investigate")
        .expect("investigate role");
    assert!(matches!(investigate.tier, SubAgentTier::Cheap));
    assert!(!investigate.with_bash, "the read-only searcher has no bash");

    let delegate = specs
        .iter()
        .find(|s| s.name == "delegate")
        .expect("delegate role");
    assert!(matches!(delegate.tier, SubAgentTier::Main));
    assert!(
        delegate.with_bash,
        "the heavier reviewer has bash for inspection"
    );
}

#[test]
fn subagent_tools_register_parallel_safe_and_on_the_right_model() {
    // Build the registry offline (the factory only contacts a provider when a
    // sub-agent is *invoked*, not at registration) and assert both tools exist
    // and declare read-only effects so the parent can fan them out in parallel.
    let fixture =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jig_auth.json");
    let provider_config = ProviderConfig::anthropic_oauth(Some(fixture));
    let stream_options = tongs::provider::StreamOptions {
        headers: provider_config.request_headers(),
        ..Default::default()
    };
    let totals = std::sync::Arc::new(crate::usage::UsageTotals::default());
    // add_subagents stores a runtime handle in each tool (for its nested runs);
    // obtain one explicitly from a runtime. Registration itself does no I/O.
    // Clone the config into the closure so the original survives for the
    // tier assertion below.
    let registry = {
        let provider_config = provider_config.clone();
        temper_agent_io::block_on_with(move |_cx, handle| async move {
            add_subagents(
                handle,
                ToolRegistry::new(),
                &provider_config,
                &stream_options,
                std::path::Path::new("."),
                &totals,
            )
        })
    };

    let names: Vec<&str> = registry.tools().iter().map(|t| t.name()).collect();
    assert!(
        names.contains(&"investigate"),
        "investigate registered: {names:?}"
    );
    assert!(
        names.contains(&"delegate"),
        "delegate registered: {names:?}"
    );
    for tool in registry.tools() {
        assert!(
            tool.effects().parallel_safe(),
            "{} must declare read-only effects for parallel fan-out",
            tool.name()
        );
    }

    // The cheap searcher runs on the sub-agent tier; the heavier reviewer on the
    // main model — the two tiers must differ (asserted on the config the factory
    // captures, not via a live call).
    assert_ne!(
        provider_config.model_id(),
        provider_config.subagent_model_id(),
        "investigate (cheap tier) must run on a different model than delegate (main)"
    );
}

#[test]
fn tool_registry_writability_matches_capability() {
    // Constructing the registries must not panic and must be scoped to cwd.
    // We can't easily introspect tool names, but we assert the writable mapping
    // is what selects the edit/write tools.
    let cwd = std::env::temp_dir();
    let writable = tool_registry(Capability::CodingWorkspace, &cwd);
    let readonly = tool_registry(Capability::TriageWorkspace, &cwd);
    let writable_names: Vec<&str> = writable.tools().iter().map(|tool| tool.name()).collect();
    let readonly_names: Vec<&str> = readonly.tools().iter().map(|tool| tool.name()).collect();
    assert!(writable_names.contains(&"write"));
    assert!(writable_names.contains(&"edit"));
    assert!(!writable_names.contains(&"publish_plan"));
    assert!(!readonly_names.contains(&"publish_plan"));
    assert!(Capability::CodingWorkspace.is_writable());
    assert!(!Capability::TriageWorkspace.is_writable());
}
