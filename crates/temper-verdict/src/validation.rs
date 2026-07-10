// SPDX-License-Identifier: MPL-2.0

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{SourceMetadata, VerdictChildView, VerdictContracts, VerdictResultView};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerdictValidationError {
    verdict: String,
    problems: Vec<String>,
}

impl VerdictValidationError {
    pub fn verdict(&self) -> &str {
        &self.verdict
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }
}

impl fmt::Display for VerdictValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Invalid verdict result for `{}`: {}",
            self.verdict,
            self.problems.join("; ")
        )
    }
}

impl std::error::Error for VerdictValidationError {}

pub fn validate_verdict_result<R: VerdictResultView>(
    result: &R,
    contracts: &VerdictContracts,
    source_metadata: &SourceMetadata,
) -> Result<(), VerdictValidationError> {
    // Additive wire compatibility: older engine/worker contexts have no map,
    // so vocabulary-only behavior remains authoritative for those jobs.
    if contracts.is_empty() {
        return Ok(());
    }
    let Some(verdict) = result
        .verdict()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        // The shared contract governs verdict payloads only. Writable actions
        // may legitimately take the no-verdict head-product path; role/capability
        // validation decides whether that path is available.
        return Ok(());
    };
    let Some(contract) = contracts.get(verdict) else {
        return Err(error(
            verdict,
            format!(
                "verdict is not declared; allowed verdicts: {}",
                contracts.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        ));
    };

    let mut problems = Vec::new();
    validate_cardinality(result, contract, &mut problems);
    validate_required_text(result, contract, &mut problems);
    validate_source_metadata(contract, source_metadata, &mut problems);
    validate_children(result.children(), contract, &mut problems);

    if problems.is_empty() {
        Ok(())
    } else {
        Err(VerdictValidationError {
            verdict: verdict.to_string(),
            problems,
        })
    }
}

fn validate_cardinality<R: VerdictResultView>(
    result: &R,
    contract: &crate::VerdictContract,
    problems: &mut Vec<String>,
) {
    let count = result.children().len();
    if count < contract.min_children {
        problems.push(match contract.max_children {
            Some(max) if max == contract.min_children => format!(
                "requires exactly {} child product(s), received {count}",
                contract.min_children
            ),
            _ => format!(
                "requires at least {} child product(s), received {count}",
                contract.min_children
            ),
        });
    }
    if contract.max_children.is_some_and(|max| count > max) {
        let max = contract.max_children.expect("checked");
        problems.push(if max == contract.min_children {
            format!("requires exactly {max} child product(s), received {count}")
        } else {
            format!("allows at most {max} child product(s), received {count}")
        });
    }
}

fn validate_required_text<R: VerdictResultView>(
    result: &R,
    contract: &crate::VerdictContract,
    problems: &mut Vec<String>,
) {
    if contract.requires_pr_title && blank(result.title()) {
        problems.push("requires a non-blank pull-request title".to_string());
    }
    if contract.requires_pr_body && blank(result.body()) {
        problems.push("requires a non-blank pull-request body".to_string());
    }
    if contract.requires_body && blank(result.body()) {
        problems.push("requires a non-blank authored body".to_string());
    }
}

fn validate_source_metadata(
    contract: &crate::VerdictContract,
    source_metadata: &SourceMetadata,
    problems: &mut Vec<String>,
) {
    for key in &contract.required_source_metadata {
        if source_metadata
            .get(key)
            .is_none_or(|value| value.trim().is_empty())
        {
            problems.push(format!("requires non-blank source metadata `{key}`"));
        }
    }
}

fn validate_children<C: VerdictChildView>(
    children: &[C],
    contract: &crate::VerdictContract,
    problems: &mut Vec<String>,
) {
    let mut slugs = BTreeSet::new();
    for (index, child) in children.iter().enumerate() {
        let slug = child.slug().trim();
        if slug.is_empty() {
            problems.push(format!("child #{} has a blank slug", index + 1));
        } else if !slugs.insert(slug.to_string()) {
            problems.push(format!("child slug `{slug}` is duplicated"));
        }
        if child.title().trim().is_empty() {
            problems.push(format!(
                "child `{}` has a blank title",
                display_slug(child, index)
            ));
        }
        if child.body().trim().is_empty() {
            problems.push(format!(
                "child `{}` has a blank body",
                display_slug(child, index)
            ));
        }
        validate_child_kind(child, index, contract, problems);
        validate_child_metadata(child, index, contract, problems);
    }
    validate_dependencies(children, &slugs, problems);
}

fn validate_child_kind<C: VerdictChildView>(
    child: &C,
    index: usize,
    contract: &crate::VerdictContract,
    problems: &mut Vec<String>,
) {
    let kind = child.kind().unwrap_or("code").trim();
    if kind.is_empty() {
        problems.push(format!(
            "child `{}` has a blank artifact kind",
            display_slug(child, index)
        ));
    } else if !contract.allowed_child_kinds.is_empty()
        && !contract
            .allowed_child_kinds
            .iter()
            .any(|allowed| allowed == kind)
    {
        problems.push(format!(
            "child `{}` has kind `{kind}`; allowed kinds: {}",
            display_slug(child, index),
            contract.allowed_child_kinds.join(", ")
        ));
    }
}

fn validate_child_metadata<C: VerdictChildView>(
    child: &C,
    index: usize,
    contract: &crate::VerdictContract,
    problems: &mut Vec<String>,
) {
    if contract.required_child_metadata.is_empty() {
        return;
    }
    let metadata = match parse_workflow_metadata(child.body()) {
        Ok(metadata) => metadata,
        Err(reason) => {
            problems.push(format!(
                "child `{}` has malformed workflow metadata: {reason}",
                display_slug(child, index)
            ));
            return;
        }
    };
    for key in &contract.required_child_metadata {
        if metadata
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_none_or(|value| value.trim().is_empty())
        {
            problems.push(format!(
                "child `{}` requires non-blank workflow metadata `{key}` in its body",
                display_slug(child, index)
            ));
        }
    }
}

fn parse_workflow_metadata(body: &str) -> Result<BTreeMap<String, serde_json::Value>, String> {
    const BEGIN: &str = "<!-- temper:workflow";
    const END: &str = "-->";
    let Some(start) = body.find(BEGIN) else {
        return Ok(BTreeMap::new());
    };
    let json_and_after = &body[start + BEGIN.len()..];
    let Some(end) = json_and_after.find(END) else {
        return Err("block was not terminated with `-->`".to_string());
    };
    serde_json::from_str(json_and_after[..end].trim())
        .map_err(|error| format!("block contained invalid JSON: {error}"))
}

fn validate_dependencies<C: VerdictChildView>(
    children: &[C],
    slugs: &BTreeSet<String>,
    problems: &mut Vec<String>,
) {
    let mut graph = BTreeMap::new();
    for (index, child) in children.iter().enumerate() {
        let slug = child.slug().trim();
        if slug.is_empty() {
            continue;
        }
        let mut dependencies = Vec::new();
        for dependency in child.depends_on() {
            let dependency = dependency.trim();
            if dependency == slug {
                problems.push(format!("child `{slug}` depends on itself"));
            } else if dependency.is_empty() || !slugs.contains(dependency) {
                problems.push(format!(
                    "child `{slug}` depends on unknown sibling `{dependency}`"
                ));
            } else {
                dependencies.push(dependency.to_string());
            }
        }
        graph
            .entry(display_slug(child, index))
            .or_insert_with(Vec::new)
            .extend(dependencies);
    }
    if graph_has_cycle(&graph) {
        problems.push("child dependency graph contains a cycle".to_string());
    }
}

fn graph_has_cycle(graph: &BTreeMap<String, Vec<String>>) -> bool {
    fn visit(
        node: &str,
        graph: &BTreeMap<String, Vec<String>>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> bool {
        if visited.contains(node) {
            return false;
        }
        if !visiting.insert(node.to_string()) {
            return true;
        }
        if graph.get(node).is_some_and(|dependencies| {
            dependencies
                .iter()
                .any(|dependency| visit(dependency, graph, visiting, visited))
        }) {
            return true;
        }
        visiting.remove(node);
        visited.insert(node.to_string());
        false
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    graph
        .keys()
        .any(|node| visit(node, graph, &mut visiting, &mut visited))
}

fn display_slug<C: VerdictChildView>(child: &C, index: usize) -> String {
    let slug = child.slug().trim();
    if slug.is_empty() {
        format!("#{}", index + 1)
    } else {
        slug.to_string()
    }
}

fn blank(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.trim().is_empty())
}

fn error(verdict: &str, problem: impl Into<String>) -> VerdictValidationError {
    VerdictValidationError {
        verdict: verdict.to_string(),
        problems: vec![problem.into()],
    }
}

#[cfg(test)]
mod tests;
