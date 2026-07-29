use std::collections::BTreeSet;

use toml::Value as TomlValue;

use super::{AgentFixture, ManifestAction, ManifestStep};
use crate::live_manifest::{StimulusKind, StimulusSpec};

pub(super) fn validate_action_links(
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
            | ManifestAction::SeedIssue { repo_id, .. }
            | ManifestAction::SeedPullRequest { repo_id, .. }
                if !repository_ids.contains(repo_id.as_str()) =>
            {
                return Err(format!(
                    "step `{}` references unknown repository id `{repo_id}`",
                    step.id
                ));
            }
            ManifestAction::Stimulus(StimulusSpec {
                kind:
                    StimulusKind::CiFailure { repo_id, .. } | StimulusKind::CiRecovery { repo_id, .. },
                ..
            }) if !repository_ids.contains(repo_id.as_str()) => {
                return Err(format!(
                    "stimulus step `{}` references unknown repository id `{repo_id}`",
                    step.id
                ));
            }
            ManifestAction::Stimulus(StimulusSpec {
                kind:
                    StimulusKind::RepeatDelivery { artifact, .. }
                    | StimulusKind::WaitProviderDeferred { artifact, .. }
                    | StimulusKind::ProviderHealthWake { artifact, .. },
                ..
            }) => {
                let Some(issue_id) = artifact.strip_prefix("issue:") else {
                    return Err(format!(
                        "stimulus step `{}` provider/delivery artifact must use issue:<id>, got `{artifact}`",
                        step.id
                    ));
                };
                if !issue_ids.contains(issue_id) && !issue_bindings.contains(issue_id) {
                    return Err(format!(
                        "stimulus step `{}` references unknown issue id or binding `{issue_id}`",
                        step.id
                    ));
                }
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
