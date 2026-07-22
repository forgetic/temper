// SPDX-License-Identifier: MPL-2.0

//! Workflow-to-wire verdict output contract derivation.

use std::collections::BTreeSet;

use temper_protocol_worker::JobArtifactSnapshot;
use temper_verdict::{SourceMetadata, TargetBranchRequirement, VerdictContract, VerdictContracts};
use temper_workflow::{
    ArtifactKindId, Effect, RelationKind, TargetBranchPolicy, ToolManifest, ValidatedWorkflow,
    WorkflowMetadata, parse_metadata_block,
};

/// Fresh source/repository facts used to turn a typed workflow policy into one
/// exact branch contract.
pub(crate) struct BranchResolutionContext<'a> {
    pub(crate) source_kind: &'a str,
    pub(crate) source_number: Option<u64>,
    pub(crate) source_metadata: &'a SourceMetadata,
    pub(crate) repository_default: &'a str,
    pub(crate) correlation_key: Option<&'a str>,
}

pub(crate) fn derive_verdict_contracts(
    workflow: &ValidatedWorkflow,
    tool: &ToolManifest,
) -> VerdictContracts {
    tool.outcomes
        .iter()
        .filter_map(|(verdict, routed)| {
            let transition = workflow
                .transitions()
                .iter()
                .find(|transition| transition.id == *routed)?;
            let mut contract = VerdictContract {
                max_children: Some(0),
                ..VerdictContract::default()
            };
            let mut child_kinds = BTreeSet::new();
            let mut has_create_issues = false;
            for effect in &transition.effects {
                match effect {
                    Effect::CreateIssues {
                        min_children,
                        max_children,
                        required_child_metadata,
                        ..
                    } => {
                        if !has_create_issues {
                            contract.min_children = *min_children;
                            contract.max_children = *max_children;
                            has_create_issues = true;
                        } else {
                            contract.min_children += *min_children;
                            contract.max_children = contract
                                .max_children
                                .zip(*max_children)
                                .map(|(left, right)| left + right);
                        }
                        for key in required_child_metadata {
                            push_unique(&mut contract.required_child_metadata, key.as_str());
                        }
                        for relation in workflow.relations().iter().filter(|relation| {
                            relation.kind == RelationKind::Parent
                                && relation.target == transition.artifact
                                && workflow
                                    .artifact_kind(&relation.source)
                                    .is_some_and(|kind| {
                                        kind.target == temper_workflow::ArtifactTarget::Issue
                                    })
                                && child_kind_has_reachable_queue(workflow, &relation.source)
                        }) {
                            child_kinds.insert(relation.source.as_str().to_string());
                        }
                    }
                    Effect::CreatePullRequest { artifact_kind, .. } => {
                        contract.requires_pr_title = true;
                        contract.requires_pr_body = true;
                        if artifact_kind.is_some() {
                            push_unique(&mut contract.required_source_metadata, "target_branch");
                        }
                    }
                    Effect::SetBody { .. } | Effect::AttachReview { .. } => {
                        contract.requires_body = true;
                    }
                    _ => {}
                }
            }
            contract.allowed_child_kinds = child_kinds.into_iter().collect();
            Some((verdict.as_str().to_string(), contract))
        })
        .collect()
}

/// Derives the ordinary result shape and resolves every child branch policy
/// against the supplied source and repository facts.
pub(crate) fn derive_resolved_verdict_contracts(
    workflow: &ValidatedWorkflow,
    tool: &ToolManifest,
    resolution: &BranchResolutionContext<'_>,
) -> Result<VerdictContracts, String> {
    let mut contracts = derive_verdict_contracts(workflow, tool);
    for (verdict, routed) in &tool.outcomes {
        let Some(transition) = workflow
            .transitions()
            .iter()
            .find(|transition| transition.id == *routed)
        else {
            continue;
        };
        let Some(contract) = contracts.get_mut(verdict.as_str()) else {
            continue;
        };
        for policy in transition.effects.iter().filter_map(|effect| match effect {
            Effect::CreateIssues {
                target_branch_policy: Some(policy),
                ..
            } => Some(*policy),
            _ => None,
        }) {
            let requirement = resolve_target_branch_requirement(policy, resolution)?;
            if contract
                .target_branch
                .as_ref()
                .is_some_and(|existing| existing != &requirement)
            {
                return Err(format!(
                    "routed transition `{routed}` resolves conflicting child target branches"
                ));
            }
            contract.target_branch = Some(requirement);
        }
    }
    Ok(contracts)
}

/// Resolves one typed branch policy. Deterministic branch production prefers the
/// fresh source kind/number identity and uses a stable correlation key only when
/// a source number is unavailable to a compatibility caller.
pub(crate) fn resolve_target_branch_requirement(
    policy: TargetBranchPolicy,
    context: &BranchResolutionContext<'_>,
) -> Result<TargetBranchRequirement, String> {
    let repository_default = context.repository_default.trim();
    if repository_default.is_empty() {
        return Err("source repository has a blank default branch".to_string());
    }

    let expected = match policy {
        TargetBranchPolicy::DerivedFeatureBranch => {
            let source_kind = context.source_kind.trim();
            match (
                source_kind.is_empty(),
                context.source_number.filter(|number| *number > 0),
            ) {
                (false, Some(number)) => {
                    format!("agent/pr-for-{}-{number}", safe_fragment(source_kind))
                }
                _ => {
                    let correlation_key = context
                    .source_metadata
                    .get("correlation_key")
                    .map(String::as_str)
                    .or(context.correlation_key)
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| {
                        "derived target-branch policy has neither a source number nor a stable correlation key"
                            .to_string()
                    })?;
                    format!("agent/{}", safe_fragment(correlation_key))
                }
            }
        }
        TargetBranchPolicy::Inherit => context
            .source_metadata
            .get("target_branch")
            .map(String::as_str)
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .ok_or_else(|| {
                "inherited target-branch policy requires non-blank source metadata `target_branch`"
                    .to_string()
            })?
            .to_string(),
        TargetBranchPolicy::RepositoryDefault => repository_default.to_string(),
        TargetBranchPolicy::NonDefault => {
            return Err(
                "non_default is a consuming policy and cannot stamp create_issues children"
                    .to_string(),
            );
        }
    };

    if policy != TargetBranchPolicy::RepositoryDefault && expected == repository_default {
        return Err(format!(
            "target-branch policy `{policy}` resolved to repository default branch `{repository_default}`"
        ));
    }

    Ok(TargetBranchRequirement {
        expected,
        repository_default: repository_default.to_string(),
        // Every policy accepted on create_issues is a production policy: the
        // engine, not the model, owns stamping an omitted child value.
        allow_omission: true,
    })
}

pub(crate) fn child_kind_has_reachable_queue(
    workflow: &ValidatedWorkflow,
    kind: &ArtifactKindId,
) -> bool {
    workflow.queues().iter().any(|queue| {
        queue.artifacts.contains(kind)
            && (queue.automation.is_some()
                || queue.actions.iter().any(|action| {
                    action
                        .artifact
                        .as_ref()
                        .is_none_or(|artifact| artifact == kind)
                })
                || workflow
                    .roles()
                    .iter()
                    .any(|role| role.queues.contains(&queue.id)))
    })
}

pub(crate) fn source_metadata_from_snapshot(
    artifact: Option<&JobArtifactSnapshot>,
) -> SourceMetadata {
    artifact
        .and_then(|artifact| parse_metadata_block(&artifact.body).ok().flatten())
        .map(source_metadata_from_workflow)
        .unwrap_or_default()
}

pub(crate) fn source_metadata_from_workflow(metadata: WorkflowMetadata) -> SourceMetadata {
    let mut values = SourceMetadata::new();
    if let Some(target_branch) = metadata.target_branch {
        values.insert("target_branch".to_string(), target_branch);
    }
    if let Some(correlation_key) = metadata.correlation_key {
        values.insert("correlation_key".to_string(), correlation_key);
    }
    values
}

fn safe_fragment(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        "work".to_string()
    } else {
        safe
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_workflow::RawWorkflowSpec;

    const PLAN_CENTRIC: &str =
        include_str!("../../../scenarios/plan-centric-feature-branch/config/workflow.json");

    fn workflow_and_tool(
        name: &str,
        role: &str,
    ) -> (ValidatedWorkflow, temper_workflow::ToolManifest) {
        let spec: RawWorkflowSpec = serde_json::from_str(PLAN_CENTRIC).expect("parse");
        let workflow = spec.validate().expect("validate");
        let tool = workflow
            .compile()
            .roles()
            .iter()
            .find(|candidate| candidate.id.as_str() == role)
            .and_then(|role| role.tools.iter().find(|tool| tool.name == name))
            .cloned()
            .expect("tool");
        (workflow, tool)
    }

    #[test]
    fn derives_exact_plan_child_and_metadata_driven_pr_contracts() {
        let (workflow, plan_feature) = workflow_and_tool("plan_feature", "architect");
        let contracts = derive_verdict_contracts(&workflow, &plan_feature);
        let needs_plan = &contracts["needs_plan"];
        assert_eq!(needs_plan.min_children, 1);
        assert_eq!(needs_plan.max_children, Some(1));
        assert_eq!(needs_plan.allowed_child_kinds, vec!["plan"]);
        assert_eq!(needs_plan.required_child_metadata, vec!["target_branch"]);
        assert!(needs_plan.target_branch.is_none());
        assert_eq!(contracts["config_only"].max_children, Some(0));

        let (_, decompose_plan) = workflow_and_tool("decompose_plan", "architect");
        let contracts = derive_verdict_contracts(&workflow, &decompose_plan);
        assert_eq!(
            contracts["children_ready"].allowed_child_kinds,
            vec!["code"]
        );

        let (_, validate) = workflow_and_tool("validate_plan", "tester");
        let contracts = derive_verdict_contracts(&workflow, &validate);
        assert!(contracts["validated"].requires_pr_title);
        assert!(contracts["validated"].requires_pr_body);
        assert_eq!(
            contracts["validated"].required_source_metadata,
            vec!["target_branch"]
        );
        assert!(contracts["needs_followup"].min_children >= 1);
        assert!(
            contracts["needs_followup"]
                .allowed_child_kinds
                .contains(&"code".to_string())
        );
    }

    #[test]
    fn resolves_derived_inherited_and_fallback_branch_requirements() {
        let metadata = SourceMetadata::new();
        let context = BranchResolutionContext {
            source_kind: "feature",
            source_number: Some(620),
            source_metadata: &metadata,
            repository_default: "main",
            correlation_key: None,
        };
        let derived =
            resolve_target_branch_requirement(TargetBranchPolicy::DerivedFeatureBranch, &context)
                .expect("derived branch");
        assert_eq!(derived.expected, "agent/pr-for-feature-620");
        assert!(derived.allow_omission);

        let metadata = SourceMetadata::from([(
            "target_branch".to_string(),
            "agent/pr-for-feature-620".to_string(),
        )]);
        let inherited = resolve_target_branch_requirement(
            TargetBranchPolicy::Inherit,
            &BranchResolutionContext {
                source_kind: "plan",
                source_number: Some(651),
                source_metadata: &metadata,
                repository_default: "main",
                correlation_key: None,
            },
        )
        .expect("inherited branch");
        assert_eq!(inherited.expected, derived.expected);

        let fallback = resolve_target_branch_requirement(
            TargetBranchPolicy::DerivedFeatureBranch,
            &BranchResolutionContext {
                source_kind: "feature",
                source_number: None,
                source_metadata: &SourceMetadata::new(),
                repository_default: "main",
                correlation_key: Some("pr-for-feature:legacy/620"),
            },
        )
        .expect("stable fallback");
        assert_eq!(fallback.expected, "agent/pr-for-feature-legacy-620");

        let repository_default = resolve_target_branch_requirement(
            TargetBranchPolicy::RepositoryDefault,
            &BranchResolutionContext {
                source_kind: "feature",
                source_number: Some(620),
                source_metadata: &SourceMetadata::new(),
                repository_default: "trunk",
                correlation_key: None,
            },
        )
        .expect("intentional repository default");
        assert_eq!(repository_default.expected, "trunk");
        assert_eq!(repository_default.repository_default, "trunk");
        assert!(repository_default.allow_omission);
    }

    #[test]
    fn resolved_contract_exposes_exact_branch_and_rejects_default_inheritance() {
        let (workflow, plan_feature) = workflow_and_tool("plan_feature", "architect");
        let metadata = SourceMetadata::new();
        let contracts = derive_resolved_verdict_contracts(
            &workflow,
            &plan_feature,
            &BranchResolutionContext {
                source_kind: "feature",
                source_number: Some(620),
                source_metadata: &metadata,
                repository_default: "main",
                correlation_key: None,
            },
        )
        .expect("resolved contracts");
        assert_eq!(
            contracts["needs_plan"].target_branch,
            Some(TargetBranchRequirement {
                expected: "agent/pr-for-feature-620".to_string(),
                repository_default: "main".to_string(),
                allow_omission: true,
            })
        );

        let (_, decompose_plan) = workflow_and_tool("decompose_plan", "architect");
        let default_metadata =
            SourceMetadata::from([("target_branch".to_string(), "main".to_string())]);
        let error = derive_resolved_verdict_contracts(
            &workflow,
            &decompose_plan,
            &BranchResolutionContext {
                source_kind: "plan",
                source_number: Some(651),
                source_metadata: &default_metadata,
                repository_default: "main",
                correlation_key: None,
            },
        )
        .expect_err("default inheritance is invalid");
        assert!(error.contains("repository default branch `main`"));
    }
}
