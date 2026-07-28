//! Role→capability mapping, writability, and sub-agent registration.

use crate::coding_agent::*;

#[test]
fn role_maps_to_capability() {
    assert_eq!(
        Capability::for_role("engineer"),
        Capability::CodingWorkspace
    );
    assert_eq!(
        Capability::for_role("scenario_author"),
        Capability::CodingWorkspace
    );
    assert_eq!(
        Capability::for_role("reviewer"),
        Capability::ReviewWorkspace
    );
    assert_eq!(Capability::for_role("tester"), Capability::ReviewWorkspace);
    assert_eq!(
        Capability::for_role("architect"),
        Capability::TriageWorkspace
    );
    // Unknown roles fall back to read-only triage; they must never be writable.
    assert_eq!(Capability::for_role("mystery"), Capability::TriageWorkspace);
    assert!(!Capability::for_role("mystery").is_writable());
}

#[test]
fn coding_workspace_roles_are_writable() {
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
    let scope_factory =
        crate::activity::ScopeFactory::new(crate::activity::AgentActivityConfig::default(), totals);
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
                &scope_factory,
                "main-scope",
                temper_agent_core::AgentOperationLimits::default(),
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
    assert!(!writable_names.contains(&"checkpoint"));
    assert!(!readonly_names.contains(&"checkpoint"));
    assert!(!writable_names.contains(&"publish_plan"));
    assert!(!readonly_names.contains(&"publish_plan"));
    assert!(Capability::CodingWorkspace.is_writable());
    assert!(!Capability::TriageWorkspace.is_writable());
}

#[test]
fn submit_for_pr_tool_exposed_only_to_writable_coding_sessions() {
    let cwd = std::env::temp_dir();
    let callback = || {
        std::sync::Arc::new(|_| {
            Box::pin(async { temper_protocol_agent::SubmitForPrResponse::accepted("ok") })
                as SubmitForPrFuture
        }) as SubmitForPrCallback
    };

    let engineer = super::common::parsed_fixture();
    let engineer_tools = tool_registry_for_context(
        Capability::CodingWorkspace,
        &engineer,
        &cwd,
        Some(callback()),
        None,
    );
    assert!(tool_names(&engineer_tools).contains(&"submit_for_pr"));

    let mut scenario_author = engineer.clone();
    scenario_author.work_item.role = "scenario_author".to_string();
    let scenario_tools = tool_registry_for_context(
        Capability::for_role("scenario_author"),
        &scenario_author,
        &cwd,
        Some(callback()),
        None,
    );
    let names = tool_names(&scenario_tools);
    assert!(names.contains(&"write"));
    assert!(names.contains(&"submit_for_pr"));

    let mut read_only_engineer = engineer.clone();
    read_only_engineer.checkout = Some("read_only".to_string());
    let read_only_tools = tool_registry_for_context(
        Capability::CodingWorkspace,
        &read_only_engineer,
        &cwd,
        Some(callback()),
        None,
    );
    assert!(!tool_names(&read_only_tools).contains(&"submit_for_pr"));

    let mut architect = engineer.clone();
    architect.work_item.role = "architect".to_string();
    architect.checkout = Some("read_only".to_string());
    let architect_tools = tool_registry_for_context(
        Capability::TriageWorkspace,
        &architect,
        &cwd,
        Some(callback()),
        None,
    );
    assert!(!tool_names(&architect_tools).contains(&"submit_for_pr"));

    let mut reviewer = engineer;
    reviewer.work_item.role = "reviewer".to_string();
    reviewer.checkout = Some("pull_request_read_only".to_string());
    let reviewer_tools = tool_registry_for_context(
        Capability::ReviewWorkspace,
        &reviewer,
        &cwd,
        Some(callback()),
        None,
    );
    assert!(!tool_names(&reviewer_tools).contains(&"submit_for_pr"));
}

#[test]
fn forge_tools_are_available_to_every_role_only_with_a_host() {
    let cwd = std::env::temp_dir();
    let host: ForgeContextHost = std::sync::Arc::new(|_| {
        Box::pin(async { Err(temper_protocol_agent::ForgeContextErrorCode::NotFound) })
    });
    for (role, capability) in [
        ("architect", Capability::TriageWorkspace),
        ("engineer", Capability::CodingWorkspace),
        ("scenario_author", Capability::CodingWorkspace),
        ("reviewer", Capability::ReviewWorkspace),
        ("tester", Capability::ReviewWorkspace),
    ] {
        let mut context = super::common::parsed_fixture();
        context.work_item.role = role.to_string();
        let with_host =
            tool_registry_for_context(capability, &context, &cwd, None, Some(host.clone()));
        let names = tool_names(&with_host);
        assert!(names.contains(&"forge_get_item"), "role={role}");
        assert!(names.contains(&"forge_list_related"), "role={role}");

        let without_host = tool_registry_for_context(capability, &context, &cwd, None, None);
        let names = tool_names(&without_host);
        assert!(!names.contains(&"forge_get_item"), "role={role}");
        assert!(!names.contains(&"forge_list_related"), "role={role}");
    }
}

fn tool_names(registry: &ToolRegistry) -> Vec<&str> {
    registry.tools().iter().map(|tool| tool.name()).collect()
}
