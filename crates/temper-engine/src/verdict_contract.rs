// SPDX-License-Identifier: MPL-2.0

//! Workflow-to-wire verdict output contract derivation.

use std::collections::BTreeSet;

use temper_protocol_worker::JobArtifactSnapshot;
use temper_verdict::{SourceMetadata, VerdictContract, VerdictContracts};
use temper_workflow::{
    Effect, RelationKind, ToolManifest, ValidatedWorkflow, WorkflowMetadata, parse_metadata_block,
};

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
    values
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

    #[test]
    fn derives_exact_plan_child_and_metadata_driven_pr_contracts() {
        let spec: RawWorkflowSpec = serde_json::from_str(PLAN_CENTRIC).expect("parse");
        let workflow = spec.validate().expect("validate");
        let compiled = workflow.compile();
        let architect = compiled
            .roles()
            .iter()
            .find(|role| role.id.as_str() == "architect")
            .expect("architect");
        let plan_feature = architect
            .tools
            .iter()
            .find(|tool| tool.name == "plan_feature")
            .expect("plan feature");
        let contracts = derive_verdict_contracts(&workflow, plan_feature);
        let needs_plan = &contracts["needs_plan"];
        assert_eq!(needs_plan.min_children, 1);
        assert_eq!(needs_plan.max_children, Some(1));
        assert_eq!(needs_plan.allowed_child_kinds, vec!["plan"]);
        assert_eq!(needs_plan.required_child_metadata, vec!["target_branch"]);
        assert_eq!(contracts["config_only"].max_children, Some(0));

        let tester = compiled
            .roles()
            .iter()
            .find(|role| role.id.as_str() == "tester")
            .expect("tester");
        let validate = tester
            .tools
            .iter()
            .find(|tool| tool.name == "validate_plan")
            .expect("validate plan");
        let contracts = derive_verdict_contracts(&workflow, validate);
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
}
