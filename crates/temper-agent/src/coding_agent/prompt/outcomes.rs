// SPDX-License-Identifier: MPL-2.0

//! Rendering workflow-derived verdict requirements in the role prompt.

use temper_verdict::{VerdictContract, VerdictContracts};

use super::super::Capability;

pub(super) fn render_workflow_outcomes(
    prompt: &mut String,
    capability: Capability,
    allowed_verdicts: &[String],
    contracts: &VerdictContracts,
) {
    prompt.push_str("\nWORKFLOW OUTCOMES:\n");
    if matches!(capability, Capability::CodingWorkspace) {
        prompt.push_str(
            "The no-verdict engineer success path remains available. If emitting a verdict, emit exactly one workflow-declared verdict below and no other verdict.\n",
        );
    } else {
        prompt.push_str("Emit exactly one workflow-declared verdict below and no other verdict.\n");
    }

    for verdict in allowed_verdicts {
        let Some(contract) = contracts.get(verdict) else {
            prompt.push_str(&format!("- Verdict `{verdict}`.\n"));
            continue;
        };
        prompt.push_str(&format!(
            "- Verdict `{verdict}` {}.\n",
            child_requirement(contract)
        ));
        if contract.min_children > 0 {
            prompt.push_str(
                "  Each child must include non-blank `slug`, `title`, and `body`; sibling slugs must be unique and `depends_on` must be acyclic.\n",
            );
        }
        for requirement in &contract.child_kind_requirements {
            prompt.push_str(&format!(
                "  The child set {} of kind `{}`.\n",
                child_kind_count_requirement(requirement),
                requirement.kind
            ));
            if !requirement.depends_on_all_kinds.is_empty() {
                prompt.push_str(&format!(
                    "  Every `{}` child must depend on every child of kind(s): {}.\n",
                    requirement.kind,
                    requirement.depends_on_all_kinds.join(", ")
                ));
            }
        }
        for key in &contract.required_child_metadata {
            if key == "target_branch" && contract.target_branch.is_some() {
                continue;
            }
            prompt.push_str(&format!(
                "  Each child body must contain non-blank workflow metadata `{key}` inside a `<!-- temper:workflow ... -->` JSON block.\n"
            ));
        }
        if let Some(requirement) = &contract.target_branch {
            if requirement.allow_omission {
                prompt.push_str(&format!(
                    "  Each child's target branch is exactly `{}`. Omit `target_branch` to let Temper stamp that value; if supplied explicitly, it must match exactly and must not be blank.\n",
                    requirement.expected
                ));
            } else {
                prompt.push_str(&format!(
                    "  Each child body must explicitly set workflow metadata `target_branch` to exactly `{}`; omission or a blank value is rejected.\n",
                    requirement.expected
                ));
            }
            if requirement.expected != requirement.repository_default {
                prompt.push_str(&format!(
                    "  The repository default branch `{}` is not valid for these children.\n",
                    requirement.repository_default
                ));
            }
        }
        if contract.requires_pr_title {
            prompt.push_str("  It requires a non-blank pull-request `title`.\n");
        }
        if contract.requires_pr_body {
            prompt.push_str("  It requires a non-blank pull-request `body`.\n");
        } else if contract.requires_body {
            prompt.push_str("  It requires a non-blank authored `body` (or `review_body`).\n");
        }
        for key in &contract.required_source_metadata {
            prompt.push_str(&format!(
                "  The source artifact must contain non-blank workflow metadata `{key}`.\n"
            ));
        }
    }
}

fn child_kind_count_requirement(requirement: &temper_verdict::ChildKindRequirement) -> String {
    match requirement.max_children {
        Some(max) if max == requirement.min_children => {
            format!(
                "requires exactly {} child product(s)",
                requirement.min_children
            )
        }
        Some(max) => format!(
            "requires {}..={max} child product(s)",
            requirement.min_children
        ),
        None => format!(
            "requires at least {} child product(s)",
            requirement.min_children
        ),
    }
}

fn child_requirement(contract: &VerdictContract) -> String {
    let count = match contract.max_children {
        Some(max) if max == contract.min_children => {
            format!(
                "requires exactly {} child product(s)",
                contract.min_children
            )
        }
        Some(max) => format!(
            "requires {}..={max} child product(s)",
            contract.min_children
        ),
        None if contract.min_children > 0 => {
            format!(
                "requires at least {} child product(s)",
                contract.min_children
            )
        }
        None => "allows any number of child products".to_string(),
    };
    if contract.allowed_child_kinds.is_empty() || contract.max_children == Some(0) {
        count
    } else {
        format!(
            "{count} of kind(s): {}",
            contract.allowed_child_kinds.join(", ")
        )
    }
}
