use std::collections::BTreeSet;
use std::path::PathBuf;

use toml::Value as TomlValue;

/// Typed, ordered live actions resolved from a scenario manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestExecutionPlan {
    pub steps: Vec<ManifestStep>,
    pub agents: Vec<AgentFixture>,
    pub convergence: ConvergenceStrategy,
    pub jig_script_path: PathBuf,
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
        validate_required_actions(&steps)?;
        validate_action_links(manifest, &steps, &agents)?;

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
        })
    }

    pub fn uses_codebase_memory(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                step.action,
                ManifestAction::StartCodebaseMemoryMcp | ManifestAction::ConfigureAgentTools
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
    },
    LaunchTemper {
        workflow_path: PathBuf,
    },
    SeedIssue {
        issue_id: String,
        binding: Option<String>,
    },
    SeedPullRequest {
        repo_id: String,
        source_issue_id: String,
    },
    StartCodebaseMemoryMcp,
    ConfigureAgentTools,
    WaitForConvergence {
        strategy: ConvergenceStrategy,
    },
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
            Self::SeedPullRequest { .. } => "pr.seed_existing",
            Self::StartCodebaseMemoryMcp => "mcp.fake_codebase_memory.start",
            Self::ConfigureAgentTools => "agent.tools.configure",
            Self::WaitForConvergence { .. } => "workflow.wait_convergence",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConvergenceStrategy {
    SinglePullRequest,
    CodebaseMemory,
    ImplementationPrHandoff,
    PlanFeatureLanding,
}

impl ConvergenceStrategy {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "single-pull-request" => Some(Self::SinglePullRequest),
            "codebase-memory" => Some(Self::CodebaseMemory),
            "implementation-pr-handoff" => Some(Self::ImplementationPrHandoff),
            "plan-feature-landing" => Some(Self::PlanFeatureLanding),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentFixture {
    pub role: String,
    pub kind: String,
    pub mode: String,
    pub tool: Option<String>,
    pub queues: Vec<String>,
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
            Ok(ManifestAction::StartJig { script_path, roles })
        }
        "temper.launch_standalone" => Ok(ManifestAction::LaunchTemper {
            workflow_path: PathBuf::from(required_table_string(table, "config", &field)?),
        }),
        "issue.seed" => Ok(ManifestAction::SeedIssue {
            issue_id: required_table_string(table, "issue_id", &field)?,
            binding: optional_table_string(table, "bind", &field)?,
        }),
        "pr.seed_existing" => Ok(ManifestAction::SeedPullRequest {
            repo_id: required_table_string(table, "repo", &field)?,
            source_issue_id: required_table_string(table, "source_issue_id", &field)?,
        }),
        "mcp.fake_codebase_memory.start" => Ok(ManifestAction::StartCodebaseMemoryMcp),
        "agent.tools.configure" => Ok(ManifestAction::ConfigureAgentTools),
        "workflow.wait_convergence" => {
            let raw = required_table_string(table, "strategy", &field)?;
            let strategy = ConvergenceStrategy::parse(&raw).ok_or_else(|| {
                format!(
                    "{field}.strategy `{raw}` is unknown; expected single-pull-request, codebase-memory, implementation-pr-handoff, or plan-feature-landing"
                )
            })?;
            Ok(ManifestAction::WaitForConvergence { strategy })
        }
        other => Err(format!(
            "{field}.action `{other}` is not supported by the live manifest executor"
        )),
    }
}

fn validate_required_actions(steps: &[ManifestStep]) -> Result<(), String> {
    let actions = steps
        .iter()
        .map(|step| step.action.kind())
        .collect::<BTreeSet<_>>();
    let missing = [
        "forgejo.provision",
        "forgejo_runner.ready",
        "repo.seed",
        "issue.seed",
        "jig.fake_llm",
        "temper.launch_standalone",
        "workflow.wait_convergence",
    ]
    .into_iter()
    .filter(|action| !actions.contains(action))
    .collect::<Vec<_>>();
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
        .any(|step| matches!(step.action, ManifestAction::StartCodebaseMemoryMcp));
    let has_tool_config = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::ConfigureAgentTools));
    let has_pr_seed = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::SeedPullRequest { .. }));
    match strategy {
        ConvergenceStrategy::CodebaseMemory if !(has_mcp && has_tool_config) => Err(
            "codebase-memory convergence requires mcp.fake_codebase_memory.start and agent.tools.configure actions"
                .to_string(),
        ),
        ConvergenceStrategy::ImplementationPrHandoff if !has_pr_seed => Err(
            "implementation-pr-handoff convergence requires a pr.seed_existing action".to_string(),
        ),
        _ => Ok(()),
    }
}

fn validate_action_links(
    manifest: &TomlValue,
    steps: &[ManifestStep],
    agents: &[AgentFixture],
) -> Result<(), String> {
    let repository_ids = manifest
        .get("repos")
        .and_then(TomlValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(TomlValue::as_table)
        .filter_map(|repo| repo.get("id").and_then(TomlValue::as_str))
        .collect::<BTreeSet<_>>();
    let issue_ids = manifest
        .get("issues")
        .and_then(TomlValue::as_array)
        .into_iter()
        .flatten()
        .filter_map(TomlValue::as_table)
        .filter_map(|issue| issue.get("id").and_then(TomlValue::as_str))
        .collect::<BTreeSet<_>>();
    let issue_bindings = steps
        .iter()
        .filter_map(|step| match &step.action {
            ManifestAction::SeedIssue { binding, .. } => binding.as_deref(),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let agent_roles = agents
        .iter()
        .map(|agent| agent.role.as_str())
        .collect::<BTreeSet<_>>();

    for step in steps {
        match &step.action {
            ManifestAction::SeedRepository { repo_id, .. }
            | ManifestAction::SeedPullRequest { repo_id, .. }
                if !repository_ids.contains(repo_id.as_str()) =>
            {
                return Err(format!(
                    "step `{}` references unknown repository id `{repo_id}`",
                    step.id
                ));
            }
            ManifestAction::SeedIssue { issue_id, .. }
                if !issue_ids.contains(issue_id.as_str()) =>
            {
                return Err(format!(
                    "step `{}` references unknown issue id `{issue_id}`",
                    step.id
                ));
            }
            ManifestAction::SeedPullRequest {
                source_issue_id, ..
            } if !issue_ids.contains(source_issue_id.as_str())
                && !issue_bindings.contains(source_issue_id.as_str()) =>
            {
                return Err(format!(
                    "step `{}` references unknown issue id or binding `{source_issue_id}`",
                    step.id
                ));
            }
            ManifestAction::StartJig { roles, .. } => {
                if let Some(role) = roles
                    .iter()
                    .find(|role| !agent_roles.contains(role.as_str()))
                {
                    return Err(format!(
                        "step `{}` configures Jig for undeclared agent role `{role}`",
                        step.id
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
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
