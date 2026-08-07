use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

use toml::Value as TomlValue;

use super::stimuli::{StimulusKind, StimulusSpec};

#[path = "execution_links.rs"]
mod execution_links;
#[path = "execution_order.rs"]
mod execution_order;
#[path = "execution_plan/terminal_history.rs"]
mod terminal_history;

use execution_links::validate_action_links;
use execution_order::validate_action_order;

/// Typed, ordered live actions resolved from a scenario manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestExecutionPlan {
    pub steps: Vec<ManifestStep>,
    pub agents: Vec<AgentFixture>,
    pub convergence: ConvergenceStrategy,
    pub jig_script_path: PathBuf,
    pub stimuli: Vec<StimulusSpec>,
}

impl ManifestExecutionPlan {
    pub fn from_manifest(manifest: &TomlValue) -> Result<Self, String> {
        validate_live_topology(manifest)?;
        let agents = agent_fixtures(manifest)?;
        let raw_steps = manifest
            .get("steps")
            .and_then(TomlValue::as_array)
            .ok_or_else(|| {
                "manifest runner requires declarative [[steps]]; no scenario-name fallback plan will be substituted"
                    .to_string()
            })?;
        let mut ids = BTreeSet::new();
        let mut steps = Vec::with_capacity(raw_steps.len());
        for (index, value) in raw_steps.iter().enumerate() {
            let table = value
                .as_table()
                .ok_or_else(|| format!("steps[{index}] must be a table"))?;
            let id = required_step_string(table, "id", index)?;
            if !ids.insert(id.clone()) {
                return Err(format!("duplicate manifest step id `{id}`"));
            }
            let action_name = required_step_string(table, "action", index)?;
            let action = parse_action(&action_name, table, index)?;
            steps.push(ManifestStep { id, action });
        }
        apply_jig_script_override(manifest, &mut steps)?;
        validate_required_actions(&steps)?;
        validate_action_links(manifest, &steps, &agents)?;
        validate_action_order(&steps, &agents)?;
        validate_stimulus_placement(&steps)?;
        let stimuli = steps
            .iter()
            .filter_map(|step| match &step.action {
                ManifestAction::Stimulus(stimulus) => Some(stimulus.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        validate_stimulus_sequence(&stimuli)?;

        let convergence = steps
            .iter()
            .filter_map(|step| match step.action {
                ManifestAction::WaitForConvergence { strategy } => Some(strategy),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [convergence] = convergence.as_slice() else {
            return Err(format!(
                "manifest execution requires exactly one workflow.wait_convergence step, found {}",
                convergence.len()
            ));
        };

        validate_strategy_actions(*convergence, &steps)?;

        let jig_paths = steps
            .iter()
            .filter_map(|step| match &step.action {
                ManifestAction::StartJig { script_path, .. } => Some(script_path.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let jig_path_refs = jig_paths.iter().collect::<Vec<_>>();
        let [jig_script_path] = jig_path_refs.as_slice() else {
            return Err(format!(
                "manifest execution requires one scenario-owned Jig script_path, found {} distinct paths",
                jig_paths.len()
            ));
        };
        if !jig_script_path.is_file() {
            return Err(format!(
                "scenario-owned Jig script is not a file: {}",
                jig_script_path.display()
            ));
        }

        Ok(Self {
            steps,
            agents,
            convergence: *convergence,
            jig_script_path: (*jig_script_path).clone(),
            stimuli,
        })
    }

    pub fn uses_codebase_memory(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                step.action,
                ManifestAction::StartCodebaseMemoryMcp { .. }
                    | ManifestAction::ConfigureAgentTools { .. }
            )
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestStep {
    pub id: String,
    pub action: ManifestAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestAction {
    ProvisionForgejo,
    AwaitForgejoRunner,
    SeedRepository {
        repo_id: String,
        seed_path: PathBuf,
        ci_source_path: PathBuf,
    },
    StartJig {
        script_path: PathBuf,
        roles: Vec<String>,
        late_stream_failure: Option<LateStreamFailureFixture>,
    },
    LaunchTemper {
        workflow_path: PathBuf,
    },
    SeedIssue {
        issue_id: String,
        repo_id: String,
        binding: Option<String>,
        after_pr_binding: Option<String>,
    },
    SeedTerminalHistory {
        fixture: TerminalHistorySeedFixture,
    },
    SeedPullRequest {
        repo_id: String,
        source_issue_id: String,
        title: String,
        body: String,
        metadata_kind: String,
        correlation_key: String,
    },
    StartCodebaseMemoryMcp {
        project: String,
        fixture: Option<String>,
        safe_tools: Vec<String>,
        hidden_tools: Vec<String>,
        readiness_delay_ms: u64,
        forced_systemic_failure: Option<ForcedSystemicFailureFixture>,
    },
    ConfigureAgentTools {
        role: String,
        tool: String,
        mode: String,
        index: String,
        tool_timeout_secs: Option<u64>,
        server_step: String,
    },
    WaitForConvergence {
        strategy: ConvergenceStrategy,
    },
    Stimulus(StimulusSpec),
}

impl ManifestAction {
    fn kind(&self) -> &'static str {
        match self {
            Self::ProvisionForgejo => "forgejo.provision",
            Self::AwaitForgejoRunner => "forgejo_runner.ready",
            Self::SeedRepository { .. } => "repo.seed",
            Self::StartJig { .. } => "jig.fake_llm",
            Self::LaunchTemper { .. } => "temper.launch_standalone",
            Self::SeedIssue { .. } => "issue.seed",
            Self::SeedTerminalHistory { .. } => "history.seed_terminal",
            Self::SeedPullRequest { .. } => "pr.seed_existing",
            Self::StartCodebaseMemoryMcp { .. } => "mcp.fake_codebase_memory.start",
            Self::ConfigureAgentTools { .. } => "agent.tools.configure",
            Self::WaitForConvergence { .. } => "workflow.wait_convergence",
            Self::Stimulus(stimulus) => stimulus.action(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConvergenceStrategy {
    SinglePullRequest,
    ImplementationPrTerminalCi,
    CiPollExactHeadRepair,
    CodebaseMemory,
    ImplementationPrHandoff,
    PlanFeatureLanding,
    HistoryIndependentTerminalRecovery,
}

impl ConvergenceStrategy {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "single-pull-request" => Some(Self::SinglePullRequest),
            "implementation-pr-terminal-ci" => Some(Self::ImplementationPrTerminalCi),
            "ci-poll-exact-head-repair" => Some(Self::CiPollExactHeadRepair),
            "codebase-memory" => Some(Self::CodebaseMemory),
            "implementation-pr-handoff" => Some(Self::ImplementationPrHandoff),
            "plan-feature-landing" => Some(Self::PlanFeatureLanding),
            "history-independent-terminal-recovery" => {
                Some(Self::HistoryIndependentTerminalRecovery)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LateStreamFailureFixture {
    /// Agent role whose provider requests receive the injected SSE failures.
    pub role: String,
    /// Absolute, non-overlapping role-request ranges that receive failures.
    pub bursts: Vec<LateStreamFailureBurst>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LateStreamFailureBurst {
    /// Number of matching-role requests completed before this burst starts.
    pub after_requests: u32,
    /// Number of consecutive unclassified late failures in this burst.
    pub failures: u32,
}

/// A bounded provider-side failure used to exercise model-visible fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForcedSystemicFailureFixture {
    /// Model-callable MCP tool that returns one systemic provider error.
    pub tool: String,
    /// Successful calls to allow before injecting the failure.
    pub after_calls: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFixture {
    pub role: String,
    pub kind: String,
    pub mode: String,
    pub tool: Option<String>,
    pub queues: Vec<String>,
}

/// Bounded bulk terminal-history fixture owned by one manifest action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalHistorySeedFixture {
    pub repo_id: String,
    pub actionable_issue_id: String,
    pub target_closed_issues: usize,
    pub target_closed_pull_requests: usize,
    pub inert_issue_labels: Vec<String>,
    pub inert_pull_request_labels: Vec<String>,
    pub sibling_repo_slug: String,
    pub sibling_closed_issues: usize,
    pub sibling_issue_labels: Vec<String>,
    pub omit_webhooks: bool,
}

fn validate_live_topology(manifest: &TomlValue) -> Result<(), String> {
    const SUMMARY: &str =
        "real Forgejo + real forgejo-runner CI + real Temper standalone + Jig fake LLM";
    let topology = manifest
        .get("topology")
        .and_then(TomlValue::as_table)
        .ok_or_else(|| format!("manifest runner requires [topology] declaring {SUMMARY}"))?;
    for (field, expected) in [
        ("forge", "forgejo"),
        ("runner", "forgejo-actions-host"),
        ("temper", "standalone"),
        ("agent_model", "scripted-fake-llm"),
    ] {
        let actual = topology.get(field).and_then(TomlValue::as_str);
        if actual != Some(expected) {
            return Err(format!(
                "manifest runner supports only the validation-grade live stack ({SUMMARY}); topology.{field} must be `{expected}`, got `{}`",
                actual.unwrap_or("<missing>")
            ));
        }
    }
    Ok(())
}

fn agent_fixtures(manifest: &TomlValue) -> Result<Vec<AgentFixture>, String> {
    let agents = manifest
        .get("agents")
        .and_then(TomlValue::as_array)
        .ok_or_else(|| "manifest execution requires [[agents]]".to_string())?;
    agents
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let table = value
                .as_table()
                .ok_or_else(|| format!("agents[{index}] must be a table"))?;
            Ok(AgentFixture {
                role: required_table_string(table, "role", &format!("agents[{index}]"))?,
                kind: required_table_string(table, "kind", &format!("agents[{index}]"))?,
                mode: required_table_string(table, "mode", &format!("agents[{index}]"))?,
                tool: optional_table_string(table, "tool", &format!("agents[{index}]"))?,
                queues: string_array(table, "queues", &format!("agents[{index}]"))?,
            })
        })
        .collect()
}

fn parse_action(name: &str, table: &toml::Table, index: usize) -> Result<ManifestAction, String> {
    let field = format!("steps[{index}]");
    match name {
        "forgejo.provision" => Ok(ManifestAction::ProvisionForgejo),
        "forgejo_runner.ready" => Ok(ManifestAction::AwaitForgejoRunner),
        "repo.seed" => Ok(ManifestAction::SeedRepository {
            repo_id: required_table_string(table, "repo", &field)?,
            seed_path: PathBuf::from(required_table_string(table, "seed_path", &field)?),
            ci_source_path: PathBuf::from(required_table_string(table, "ci_source", &field)?),
        }),
        "jig.fake_llm" => {
            let script_path = PathBuf::from(required_table_string(table, "script_path", &field)?);
            let mut roles = string_array(table, "roles", &field)?;
            if let Some(role) = optional_table_string(table, "role", &field)? {
                roles.push(role);
            }
            roles.sort();
            roles.dedup();
            if roles.is_empty() {
                return Err(format!("{field}.role or {field}.roles is required"));
            }
            let late_stream_failure = parse_late_stream_failure(table, &field)?;
            if let Some(failure) = &late_stream_failure {
                if !roles.contains(&failure.role) {
                    return Err(format!(
                        "{field}.late_stream_failure.role `{}` must be included in the Jig roles",
                        failure.role
                    ));
                }
            }
            Ok(ManifestAction::StartJig {
                script_path,
                roles,
                late_stream_failure,
            })
        }
        "temper.launch_standalone" => Ok(ManifestAction::LaunchTemper {
            workflow_path: PathBuf::from(required_table_string(table, "config", &field)?),
        }),
        "issue.seed" => Ok(ManifestAction::SeedIssue {
            issue_id: required_table_string(table, "issue_id", &field)?,
            repo_id: required_table_string(table, "repo", &field)?,
            binding: optional_table_string(table, "bind", &field)?,
            after_pr_binding: optional_table_string(table, "after_pr_binding", &field)?,
        }),
        "history.seed_terminal" => terminal_history::parse(table, index),
        "pr.seed_existing" => Ok(ManifestAction::SeedPullRequest {
            repo_id: required_table_string(table, "repo", &field)?,
            source_issue_id: required_table_string(table, "source_issue_id", &field)?,
            title: required_table_string(table, "title", &field)?,
            body: required_table_string(table, "stale_body", &field)?,
            metadata_kind: required_table_string(table, "metadata_kind", &field)?,
            correlation_key: required_table_string(table, "correlation_key", &field)?,
        }),
        "mcp.fake_codebase_memory.start" => Ok(ManifestAction::StartCodebaseMemoryMcp {
            project: required_table_string(table, "project", &field)?,
            fixture: optional_table_string(table, "lifecycle_profile", &field)?,
            safe_tools: string_array(table, "advertises_safe_tools", &field)?,
            hidden_tools: string_array(table, "advertises_hidden_tools", &field)?,
            readiness_delay_ms: bounded_integer(table, "readiness_delay_ms", &field, 0, 0, 5_000)?,
            forced_systemic_failure: parse_forced_systemic_failure(table, &field)?,
        }),
        "agent.tools.configure" => Ok(ManifestAction::ConfigureAgentTools {
            role: required_table_string(table, "role", &field)?,
            tool: required_table_string(table, "tool", &field)?,
            mode: required_table_string(table, "mode", &field)?,
            index: required_table_string(table, "index", &field)?,
            tool_timeout_secs: table
                .contains_key("tool_timeout_secs")
                .then(|| bounded_integer(table, "tool_timeout_secs", &field, 1, 1, 600))
                .transpose()?,
            server_step: required_table_string(table, "server", &field)?,
        }),
        "workflow.wait_convergence" => {
            let raw = required_table_string(table, "strategy", &field)?;
            let strategy = ConvergenceStrategy::parse(&raw).ok_or_else(|| {
                format!(
                    "{field}.strategy `{raw}` is unknown; expected single-pull-request, implementation-pr-terminal-ci, ci-poll-exact-head-repair, codebase-memory, implementation-pr-handoff, plan-feature-landing, or history-independent-terminal-recovery"
                )
            })?;
            Ok(ManifestAction::WaitForConvergence { strategy })
        }
        "temper.restart"
        | "forgejo_runner.restart"
        | "ci.fail"
        | "ci.recover"
        | "delivery.repeat"
        | "discovery.wait_warm"
        | "provider.wait_deferred"
        | "provider.health_wake" => Ok(ManifestAction::Stimulus(parse_stimulus(
            name, table, index,
        )?)),
        other => Err(format!(
            "{field}.action `{other}` is not supported by the live manifest executor"
        )),
    }
}

fn parse_forced_systemic_failure(
    table: &toml::Table,
    field: &str,
) -> Result<Option<ForcedSystemicFailureFixture>, String> {
    let Some(value) = table.get("forced_systemic_failure") else {
        return Ok(None);
    };
    let failure = value
        .as_table()
        .ok_or_else(|| format!("{field}.forced_systemic_failure must be an inline table"))?;
    let failure_field = format!("{field}.forced_systemic_failure");
    let tool = required_table_string(failure, "tool", &failure_field)?;
    let after_calls = bounded_integer(failure, "after_calls", &failure_field, 1, 1, 16)?;
    Ok(Some(ForcedSystemicFailureFixture {
        tool,
        after_calls: usize::try_from(after_calls).expect("bounded forced failure count fits usize"),
    }))
}

fn parse_late_stream_failure(
    table: &toml::Table,
    field: &str,
) -> Result<Option<LateStreamFailureFixture>, String> {
    let Some(value) = table.get("late_stream_failure") else {
        return Ok(None);
    };
    let failure = value
        .as_table()
        .ok_or_else(|| format!("{field}.late_stream_failure must be an inline table"))?;
    let role = required_table_string(failure, "role", &format!("{field}.late_stream_failure"))?;
    let has_single = failure.contains_key("after_requests") || failure.contains_key("failures");
    let bursts = match failure.get("bursts") {
        Some(_) if has_single => {
            return Err(format!(
                "{field}.late_stream_failure must use either after_requests/failures or bursts, not both"
            ));
        }
        Some(value) => {
            let values = value.as_array().ok_or_else(|| {
                format!("{field}.late_stream_failure.bursts must be an array of inline tables")
            })?;
            if values.is_empty() || values.len() > 8 {
                return Err(format!(
                    "{field}.late_stream_failure.bursts must contain 1 through 8 entries"
                ));
            }
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let burst = value.as_table().ok_or_else(|| {
                        format!(
                            "{field}.late_stream_failure.bursts[{index}] must be an inline table"
                        )
                    })?;
                    parse_late_stream_burst(
                        burst,
                        &format!("{field}.late_stream_failure.bursts[{index}]"),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        None => vec![parse_late_stream_burst(
            failure,
            &format!("{field}.late_stream_failure"),
        )?],
    };
    for pair in bursts.windows(2) {
        let prior_end = pair[0]
            .after_requests
            .checked_add(pair[0].failures)
            .ok_or_else(|| format!("{field}.late_stream_failure burst range overflow"))?;
        if pair[1].after_requests < prior_end {
            return Err(format!(
                "{field}.late_stream_failure bursts must be ordered and non-overlapping"
            ));
        }
    }
    Ok(Some(LateStreamFailureFixture { role, bursts }))
}

fn parse_late_stream_burst(
    table: &toml::Table,
    field: &str,
) -> Result<LateStreamFailureBurst, String> {
    for required in ["after_requests", "failures"] {
        if !table.contains_key(required) {
            return Err(format!("{field}.{required} is required"));
        }
    }
    let after_requests = bounded_integer(table, "after_requests", field, 1, 0, 100)? as u32;
    let failures = bounded_integer(table, "failures", field, 1, 1, 32)? as u32;
    after_requests
        .checked_add(failures)
        .filter(|end| *end <= 128)
        .ok_or_else(|| format!("{field} request range must end at or before 128"))?;
    Ok(LateStreamFailureBurst {
        after_requests,
        failures,
    })
}

fn apply_jig_script_override(
    manifest: &TomlValue,
    steps: &mut [ManifestStep],
) -> Result<(), String> {
    let Some(jig) = manifest.get("jig") else {
        return Ok(());
    };
    let table = jig
        .as_table()
        .ok_or_else(|| "jig must be a table".to_string())?;
    let path = PathBuf::from(required_table_string(table, "script_path", "jig")?);
    for step in steps {
        if let ManifestAction::StartJig { script_path, .. } = &mut step.action {
            *script_path = path.clone();
        }
    }
    Ok(())
}

fn validate_stimulus_placement(steps: &[ManifestStep]) -> Result<(), String> {
    let convergence_index = steps
        .iter()
        .position(|step| matches!(step.action, ManifestAction::WaitForConvergence { .. }))
        .unwrap_or(steps.len());
    if let Some(step) = steps[convergence_index.saturating_add(1)..]
        .iter()
        .find(|step| matches!(step.action, ManifestAction::Stimulus(_)))
    {
        return Err(format!(
            "stimulus step `{}` must run before workflow.wait_convergence; assertions are the only after-convergence hooks",
            step.id
        ));
    }
    Ok(())
}

fn parse_stimulus(action: &str, table: &toml::Table, index: usize) -> Result<StimulusSpec, String> {
    const MAX_TIMEOUT_MS: u64 = 600_000;
    const MAX_ATTEMPTS: u64 = 3;
    const MAX_DELIVERIES: u64 = 10;
    let field = format!("steps[{index}]");
    let timeout_ms = bounded_integer(table, "timeout_ms", &field, 30_000, 1, MAX_TIMEOUT_MS)?;
    let max_attempts = bounded_integer(table, "max_attempts", &field, 1, 1, MAX_ATTEMPTS)?;
    let kind = match action {
        "temper.restart" => StimulusKind::RestartTemper,
        "forgejo_runner.restart" => StimulusKind::RestartRunner,
        "ci.fail" => StimulusKind::CiFailure {
            repo_id: required_table_string(table, "repo", &field)?,
            workflow_path: required_stimulus_file(table, "fixture", &field)?,
        },
        "ci.recover" => StimulusKind::CiRecovery {
            repo_id: required_table_string(table, "repo", &field)?,
            workflow_path: required_stimulus_file(table, "fixture", &field)?,
        },
        "delivery.repeat" => StimulusKind::RepeatDelivery {
            artifact: required_table_string(table, "artifact", &field)?,
            deliveries: bounded_integer(table, "deliveries", &field, 2, 2, MAX_DELIVERIES)?,
        },
        "discovery.wait_warm" => StimulusKind::WaitDiscoveryWarm {
            role_passes: bounded_integer(table, "role_passes", &field, 2, 1, 10)? as usize,
            mechanical_passes: bounded_integer(table, "mechanical_passes", &field, 2, 1, 10)?
                as usize,
        },
        "provider.wait_deferred" => StimulusKind::WaitProviderDeferred {
            artifact: required_table_string(table, "artifact", &field)?,
            generation: bounded_integer(table, "generation", &field, 1, 1, 1_000_000)? as u32,
        },
        "provider.health_wake" => StimulusKind::ProviderHealthWake {
            artifact: required_table_string(table, "artifact", &field)?,
            expected_generation: bounded_integer(
                table,
                "expected_generation",
                &field,
                1,
                1,
                1_000_000,
            )? as u32,
            event_id: required_table_string(table, "event_id", &field)?,
        },
        _ => unreachable!("caller filters stimulus actions"),
    };
    Ok(StimulusSpec {
        id: required_table_string(table, "id", &field)?,
        kind,
        timeout: Duration::from_millis(timeout_ms),
        max_attempts,
    })
}

fn required_stimulus_file(table: &toml::Table, key: &str, field: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required_table_string(table, key, field)?);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{field}.{key} must resolve to a readable fixture file: {}",
            path.display()
        ))
    }
}

fn bounded_integer(
    table: &toml::Table,
    key: &str,
    field: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let Some(value) = table.get(key) else {
        return Ok(default);
    };
    let value = value
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{field}.{key} must be an integer from {min} through {max}"))?;
    Ok(value)
}

fn validate_stimulus_sequence(stimuli: &[StimulusSpec]) -> Result<(), String> {
    let mut pending_ci_failures = BTreeMap::<&str, &str>::new();
    let mut observed_deferrals = BTreeSet::<(&str, u32)>::new();
    for stimulus in stimuli {
        match &stimulus.kind {
            StimulusKind::CiFailure { repo_id, .. } => {
                if pending_ci_failures.insert(repo_id, &stimulus.id).is_some() {
                    return Err(format!(
                        "stimulus `{}` declares another CI failure for `{repo_id}` before recovery",
                        stimulus.id
                    ));
                }
            }
            StimulusKind::CiRecovery { repo_id, .. } => {
                if pending_ci_failures.remove(repo_id.as_str()).is_none() {
                    return Err(format!(
                        "stimulus `{}` recovers CI for `{repo_id}` without a preceding ci.fail stimulus",
                        stimulus.id
                    ));
                }
            }
            StimulusKind::WaitProviderDeferred {
                artifact,
                generation,
            } => {
                observed_deferrals.insert((artifact.as_str(), *generation));
            }
            StimulusKind::ProviderHealthWake {
                artifact,
                expected_generation,
                ..
            } => {
                if !observed_deferrals.contains(&(artifact.as_str(), *expected_generation)) {
                    return Err(format!(
                        "provider-health wake stimulus `{}` requires an earlier provider.wait_deferred for `{artifact}` generation {expected_generation}",
                        stimulus.id
                    ));
                }
            }
            _ => {}
        }
    }
    if let Some((repo, stimulus)) = pending_ci_failures.first_key_value() {
        return Err(format!(
            "CI failure stimulus `{stimulus}` for `{repo}` has no bounded ci.recover stimulus"
        ));
    }
    Ok(())
}

fn validate_required_actions(steps: &[ManifestStep]) -> Result<(), String> {
    let actions = steps
        .iter()
        .map(|step| step.action.kind())
        .collect::<BTreeSet<_>>();
    let mut missing = [
        "forgejo.provision",
        "forgejo_runner.ready",
        "repo.seed",
        "jig.fake_llm",
        "temper.launch_standalone",
        "workflow.wait_convergence",
    ]
    .into_iter()
    .filter(|action| !actions.contains(action))
    .collect::<Vec<_>>();
    if !actions.contains("issue.seed") && !actions.contains("history.seed_terminal") {
        missing.push("issue.seed or history.seed_terminal");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "manifest execution plan is missing required action(s): {}",
            missing.join(", ")
        ))
    }
}

fn validate_strategy_actions(
    strategy: ConvergenceStrategy,
    steps: &[ManifestStep],
) -> Result<(), String> {
    let has_mcp = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::StartCodebaseMemoryMcp { .. }));
    let has_tool_config = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::ConfigureAgentTools { .. }));
    let has_pr_seed = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::SeedPullRequest { .. }));
    let has_terminal_history = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::SeedTerminalHistory { .. }));
    let has_restart = steps.iter().any(|step| {
        matches!(
            step.action,
            ManifestAction::Stimulus(StimulusSpec {
                kind: StimulusKind::RestartTemper,
                ..
            })
        )
    });
    match strategy {
        ConvergenceStrategy::CodebaseMemory if !(has_mcp && has_tool_config) => Err(
            "codebase-memory convergence requires mcp.fake_codebase_memory.start and agent.tools.configure actions"
                .to_string(),
        ),
        ConvergenceStrategy::ImplementationPrHandoff if !has_pr_seed => Err(
            "implementation-pr-handoff convergence requires a pr.seed_existing action".to_string(),
        ),
        ConvergenceStrategy::HistoryIndependentTerminalRecovery
            if !(has_terminal_history && has_restart) =>
        {
            Err("history-independent-terminal-recovery convergence requires history.seed_terminal and temper.restart actions".to_string())
        }
        _ => Ok(()),
    }
}

fn required_step_string(table: &toml::Table, key: &str, index: usize) -> Result<String, String> {
    required_table_string(table, key, &format!("steps[{index}]"))
}

fn required_table_string(table: &toml::Table, key: &str, field: &str) -> Result<String, String> {
    optional_table_string(table, key, field)?.ok_or_else(|| format!("{field}.{key} is required"))
}

fn optional_table_string(
    table: &toml::Table,
    key: &str,
    field: &str,
) -> Result<Option<String>, String> {
    let Some(value) = table.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("{field}.{key} must be a non-empty string"))?
        .trim();
    if value.is_empty() {
        Err(format!("{field}.{key} must be a non-empty string"))
    } else {
        Ok(Some(value.to_string()))
    }
}

fn string_array(table: &toml::Table, key: &str, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = table.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| format!("{field}.{key} must be an array of strings"))?;
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| format!("{field}.{key} must contain only strings"))?
                .trim();
            if value.is_empty() {
                Err(format!("{field}.{key} must not contain empty strings"))
            } else {
                Ok(value.to_string())
            }
        })
        .collect()
}
