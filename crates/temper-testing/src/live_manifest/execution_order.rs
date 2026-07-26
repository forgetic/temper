use std::collections::BTreeSet;

use super::{AgentFixture, ManifestAction, ManifestStep};

pub(super) fn validate_action_order(
    steps: &[ManifestStep],
    agents: &[AgentFixture],
) -> Result<(), String> {
    let required_jig_roles = agents
        .iter()
        .filter(|agent| agent.kind == "llm")
        .map(|agent| agent.role.as_str())
        .collect::<BTreeSet<_>>();
    let requires_tool_config = steps
        .iter()
        .any(|step| matches!(step.action, ManifestAction::ConfigureAgentTools { .. }));
    let mut completed = BTreeSet::<&str>::new();
    let mut provisioned = false;
    let mut runner_ready = false;
    let mut repository_seeded = false;
    let mut jig_roles = BTreeSet::<&str>::new();
    let mut mcp_started = false;
    let mut tools_configured = false;
    let mut temper_started = false;
    let mut issue_bindings = BTreeSet::<&str>::new();

    for (index, step) in steps.iter().enumerate() {
        let prerequisite = |required: bool, message: &str| {
            required.then_some(()).ok_or_else(|| {
                format!(
                    "step `{}` ({}) cannot run before {message}",
                    step.id,
                    step.action.kind()
                )
            })
        };
        match &step.action {
            ManifestAction::ProvisionForgejo => {
                if provisioned {
                    return Err(format!(
                        "step `{}` provisions Forgejo more than once",
                        step.id
                    ));
                }
                provisioned = true;
            }
            ManifestAction::AwaitForgejoRunner => {
                prerequisite(provisioned, "forgejo.provision")?;
                runner_ready = true;
            }
            ManifestAction::SeedRepository { .. } => {
                prerequisite(temper_started, "temper.launch_standalone")?;
                repository_seeded = true;
            }
            ManifestAction::StartCodebaseMemoryMcp { .. } => {
                prerequisite(provisioned, "forgejo.provision")?;
                mcp_started = true;
            }
            ManifestAction::ConfigureAgentTools { server_step, .. } => {
                prerequisite(mcp_started, "mcp.fake_codebase_memory.start")?;
                let referenced = server_step.strip_prefix("$step:").ok_or_else(|| {
                    format!(
                        "step `{}` agent.tools.configure server must use $step:<id>, got `{server_step}`",
                        step.id
                    )
                })?;
                prerequisite(
                    completed.contains(referenced),
                    &format!("referenced MCP step `{referenced}`"),
                )?;
                tools_configured = true;
            }
            ManifestAction::StartJig { roles, .. } => {
                prerequisite(provisioned, "forgejo.provision")?;
                jig_roles.extend(roles.iter().map(String::as_str));
            }
            ManifestAction::LaunchTemper { .. } => {
                prerequisite(runner_ready, "forgejo_runner.ready")?;
                if let Some(role) = required_jig_roles
                    .iter()
                    .find(|role| !jig_roles.contains(**role))
                {
                    return Err(format!(
                        "step `{}` cannot launch Temper before Jig is configured for declared LLM role `{role}`",
                        step.id
                    ));
                }
                prerequisite(
                    !requires_tool_config || tools_configured,
                    "agent.tools.configure",
                )?;
                temper_started = true;
            }
            ManifestAction::SeedIssue {
                issue_id,
                binding,
                after_pr_binding,
                ..
            } => {
                prerequisite(temper_started, "temper.launch_standalone")?;
                prerequisite(repository_seeded, "repo.seed")?;
                if let Some(binding) = after_pr_binding {
                    prerequisite(
                        issue_bindings.contains(binding.as_str()),
                        &format!("earlier issue.seed binding `{binding}`"),
                    )?;
                }
                issue_bindings.insert(issue_id.as_str());
                if let Some(binding) = binding {
                    if binding != issue_id && !issue_bindings.insert(binding.as_str()) {
                        return Err(format!(
                            "step `{}` reuses issue id or binding `{binding}`",
                            step.id
                        ));
                    }
                }
            }
            ManifestAction::SeedPullRequest {
                source_issue_id, ..
            } => {
                prerequisite(temper_started, "temper.launch_standalone")?;
                prerequisite(
                    issue_bindings.contains(source_issue_id.as_str()),
                    &format!("issue.seed binding `{source_issue_id}`"),
                )?;
            }
            ManifestAction::Stimulus(_) => {
                prerequisite(temper_started, "temper.launch_standalone")?;
                prerequisite(!issue_bindings.is_empty(), "issue.seed")?;
            }
            ManifestAction::WaitForConvergence { .. } => {
                prerequisite(temper_started, "temper.launch_standalone")?;
                prerequisite(!issue_bindings.is_empty(), "issue.seed")?;
                if index + 1 != steps.len() {
                    return Err(format!(
                        "step `{}` must be the final runtime action; found {} action(s) after convergence",
                        step.id,
                        steps.len() - index - 1
                    ));
                }
            }
        }
        completed.insert(&step.id);
    }
    Ok(())
}
