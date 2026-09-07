//! Provider startup, scope preparation and capability-checked registration.
use super::*;

pub(super) async fn start_toolset(
    config: &CodebaseMemoryToolConfig,
    role: &str,
    mut scope: WorkspaceScope,
    generic_tool_timeout: Duration,
    containment: &AgentContainmentContext,
    serving_admitted: Option<&(dyn Fn() + Send + Sync)>,
) -> std::result::Result<CodebaseMemoryToolset, McpError> {
    let startup_timeout = Duration::from_secs(config.startup_timeout_secs);
    let index_timeout = Duration::from_secs(config.index_timeout_secs);
    let call_timeout = effective_mcp_call_timeout(index_timeout, generic_tool_timeout);
    let mcp_config = StdioMcpServerConfig::new(config.command.clone(), config.args.clone())
        .with_containment_identity("codebase-memory")
        .with_working_directory(scope.primary_root())
        .with_startup_timeout(startup_timeout)
        .with_call_timeout(index_timeout);
    emit_agent_tool_configured(AgentToolConfigured {
        role,
        tool_name: "codebase_memory",
        mode: codebase_memory_mode(config.mode),
        index: codebase_memory_index(config.index),
        model_visible: false,
        repo_root: &scope.primary_root().display().to_string(),
    });
    let discovery_client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    let discovery_tools = discovery_client.list_tools(startup_timeout).await?;
    validate_provider_contract(&discovery_client, &discovery_tools)?;
    let mut setup_notes = Vec::new();

    let discovery_started = Instant::now();
    match discover_workspace_projects(&discovery_client, startup_timeout, &scope).await {
        Ok(states) => {
            let record_count = states.len();
            scope
                .apply_targeted_discovery(states)
                .map_err(McpError::Protocol)?;
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome: DiscoveryOutcome::Success,
                record_count,
                cache_bytes: None,
                failure: FailureCategory::None,
            });
            for project in &scope.projects {
                let outcome = match project.index_state {
                    scope::ProjectIndexState::Missing => "missing",
                    scope::ProjectIndexState::Stale => "stale",
                    scope::ProjectIndexState::Fresh => "fresh",
                    _ => "unavailable",
                };
                emit_identity_selected(&project.canonical_alias, &project.provider_key, outcome);
            }
        }
        Err(error) if config.mode == CodebaseMemoryMode::Auto => {
            let outcome = if matches!(error, McpError::Timeout { .. }) {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::Failure
            };
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome,
                record_count: 0,
                cache_bytes: None,
                failure: FailureCategory::from(&error),
            });
            scope.mark_discovery_unavailable();
            setup_notes.push(
                "safe targeted project discovery was unavailable; indexing was skipped for every prepared repo and no path-keyed fallback was attempted"
                    .to_string(),
            );
        }
        Err(error) => {
            let outcome = if matches!(error, McpError::Timeout { .. }) {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::Failure
            };
            emit_discovery(DiscoveryEvidence {
                method: "index_status",
                inventory: "targeted",
                duration: discovery_started.elapsed(),
                outcome,
                record_count: 0,
                cache_bytes: None,
                failure: FailureCategory::from(&error),
            });
            return Err(error);
        }
    }

    // Discovery requests and their timeouts are process-fatal in the stdio
    // client. Never clone that process into model-visible wrappers, even after
    // successful discovery: initialize and validate a fresh serving client.
    let client =
        StdioMcpClient::connect_with_containment(mcp_config.clone(), containment.clone()).await?;
    let advertised = client.list_tools(startup_timeout).await?;
    validate_provider_contract(&client, &advertised)?;
    if let Some(admitted) = serving_admitted {
        admitted();
    }
    // The replacement must be admitted before a healthy discovery session
    // leaves the shared provider generation. A timed-out discovery connection
    // is already poisoned; it is never reused by the serving wrappers.
    drop(discovery_client);

    // Establish the serving client before spawning a background upsert. That
    // makes the background index's readiness visible from the first model tool
    // call instead of consuming its work while the serving client starts.
    setup_notes
        .extend(prepare_indexes(config, &mcp_config, &advertised, &mut scope, containment).await?);
    emit_mcp_server_started(McpServerStarted {
        tool_name: "codebase_memory",
        command: &config.command,
        repo_root: &scope.primary_root().display().to_string(),
    });
    scope.rebuild_alias_map();
    if !advertised.iter().any(coverage_capability) {
        setup_notes.push("Coverage capability unavailable: read cited source directly and limit negative/exhaustive claims; no coverage retries. This optional diagnostic does not change required startup/index policy.".to_string());
    }
    let prompt_status = scope.prompt_status(config.index, &setup_notes);
    let scope = Arc::new(scope);

    // This state belongs to exactly this toolset build (one agent run) and is
    // shared by every wrapper cloned from the serving client.
    let health = Arc::new(CodebaseMemoryHealth::new(client.cancellation_handle()));
    // Provider-shaped target values remain in this wrapper-local registry. The
    // core receives only an opaque root and typed aggregate lineage record.
    let decision_anchor_lineages = Arc::new(DecisionAnchorLineageRegistry::new(Arc::clone(&scope)));
    let lineage_admission: LineageAdmissionHandle = decision_anchor_lineages.clone();

    let mut tools: Vec<Box<dyn Tool>> = Vec::new();
    let mut registered_tool_metadata = Vec::new();

    let mut coverage = None;
    for descriptor in advertised {
        if descriptor.name == "check_index_coverage" && !coverage_capability(&descriptor) {
            continue;
        }
        let Some(allowed) = allowed_tool(&descriptor.name) else {
            let hidden_name = format!("codebase_memory_{}", descriptor.name);
            emit_agent_tool_hidden(AgentToolHidden {
                role,
                tool_name: &hidden_name,
                mcp_tool: &descriptor.name,
                model_visible: false,
                reason: "not on safe model allowlist",
            });
            continue;
        };
        let public_name = allowed.public_name.to_string();
        let default_project_key = default_project_key(allowed.mcp_name, &descriptor.input_schema);
        let description = description_for(*allowed, &descriptor.description, &scope);
        let parameters = scoped_parameters(&descriptor.input_schema, *allowed, &scope);
        emit_agent_tool_exposed(AgentToolExposed {
            role,
            tool_name: &public_name,
            mcp_tool: &descriptor.name,
            model_visible: true,
            repo_root: &scope.primary_root().display().to_string(),
            mcp_project: &scope.primary_actual_project(),
        });
        registered_tool_metadata.push(CodebaseMemoryToolMetadata {
            name: public_name.clone(),
            description: description.clone(),
        });
        let tool = CodebaseMemoryTool::new(
            client.clone(),
            Arc::clone(&health),
            descriptor.name,
            *allowed,
            public_name,
            description,
            parameters,
            default_project_key,
            call_timeout,
            Arc::clone(&scope),
            Arc::clone(&decision_anchor_lineages),
        );
        if tool.mcp_name == "check_index_coverage" {
            let service = Arc::new(CoverageService::new(tool));
            coverage = Some(service.clone());
            tools.push(Box::new(coverage::CoverageTool(service)));
        } else {
            tools.push(Box::new(tool));
        }
    }

    Ok(CodebaseMemoryToolset::started(
        tools,
        registered_tool_metadata,
        prompt_status,
        lineage_admission,
        coverage,
    ))
}

fn coverage_capability(tool: &McpToolDescriptor) -> bool {
    tool.name == "check_index_coverage"
        && [
            ("project", "string"),
            ("paths", "array"),
            ("scopes", "array"),
            ("scope_limit", "integer"),
            ("scope_offset", "integer"),
        ]
        .iter()
        .all(|(key, kind)| tool.input_schema["properties"][*key]["type"] == *kind)
}

pub(super) fn effective_mcp_call_timeout(
    index_timeout: Duration,
    generic_tool_timeout: Duration,
) -> Duration {
    index_timeout.min(generic_tool_timeout)
}

fn allowed_tool(name: &str) -> Option<&'static AllowedCodebaseMemoryTool> {
    ALLOWLIST.iter().find(|tool| tool.mcp_name == name)
}

pub(super) fn advertised_tool(advertised: &[McpToolDescriptor], name: &str) -> bool {
    advertised.iter().any(|descriptor| descriptor.name == name)
}
