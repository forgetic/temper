//! Process-local provider-result matching for typed decision-anchor lineage.
//!
//! Values parsed here never leave the wrapper. The policy receives only the
//! opaque root and canonical target-kind aggregate in `DecisionAnchorLineageV1`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use temper_agent_core::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
};
use temper_protocol_activity::{GraphCorrelationTargetKindV1, GraphCorrelationV1};
use uuid::Uuid;

const MAX_RESULT_TARGETS: usize = 64;

#[derive(Default)]
pub(super) struct DecisionAnchorLineages {
    /// `None` marks an ambiguous value. Once more than one root has offered a
    /// representation, no later model selection may use that representation to
    /// advance either root.
    selectors: BTreeMap<Selector, Option<String>>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Selector {
    kind: DecisionAnchorTargetKindV1,
    value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Candidate {
    kind: DecisionAnchorTargetKindV1,
    value: String,
}

impl DecisionAnchorLineages {
    /// Derives one trusted output record after a successful, complete targeted
    /// wrapper result. Callers must not invoke this for provider errors, empty
    /// output, or truncation.
    pub(super) fn record(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        provider_result: &str,
    ) -> Option<DecisionAnchorLineageV1> {
        if !correlation.is_valid() {
            return None;
        }
        let target_kind =
            DecisionAnchorTargetKindV1::from_graph_correlation(correlation.target_kind);
        let root = self
            .selector_for_input(correlation.target_kind, input)
            .and_then(|selector| self.selectors.get(&selector).cloned().flatten());
        let (root_binding, stage) = match root {
            Some(root_binding) => (root_binding, DecisionAnchorLineageStageV1::CarryForward),
            None => (
                Uuid::new_v4().to_string(),
                DecisionAnchorLineageStageV1::Root,
            ),
        };

        // Any malformed, duplicate, unsupported, or oversized provider record
        // contributes no carry-forward values. The current successful result is
        // still a root/carry record, but it cannot unlock an additional hop.
        let result_target_kinds = match provider_candidates(provider_result) {
            Some(candidates) => {
                let kinds = candidates.iter().map(|candidate| candidate.kind).collect();
                self.register(&root_binding, candidates);
                kinds
            }
            None => BTreeSet::new(),
        };
        DecisionAnchorLineageV1::new(root_binding, stage, target_kind, result_target_kinds)
    }

    fn selector_for_input(
        &self,
        target_kind: GraphCorrelationTargetKindV1,
        input: &Value,
    ) -> Option<Selector> {
        match target_kind {
            GraphCorrelationTargetKindV1::FunctionName => input
                .get("function_name")
                .and_then(Value::as_str)
                .and_then(canonical_function_name)
                .map(|value| Selector {
                    kind: DecisionAnchorTargetKindV1::FunctionName,
                    value,
                }),
            GraphCorrelationTargetKindV1::QualifiedName => input
                .get("qualified_name")
                .and_then(Value::as_str)
                .and_then(canonical_qualified_name)
                .map(|value| Selector {
                    kind: DecisionAnchorTargetKindV1::QualifiedName,
                    value,
                }),
            GraphCorrelationTargetKindV1::Pattern => input
                .get("pattern")
                .and_then(Value::as_str)
                .and_then(|value| {
                    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
                })
                .map(|value| Selector {
                    kind: DecisionAnchorTargetKindV1::Pattern,
                    value,
                }),
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => None,
        }
    }

    fn register(&mut self, root: &str, candidates: BTreeSet<Candidate>) {
        for candidate in candidates {
            let selector = Selector {
                kind: candidate.kind,
                value: candidate.value,
            };
            match self.selectors.get(&selector) {
                None => {
                    self.selectors.insert(selector, Some(root.to_string()));
                }
                Some(Some(existing)) if existing == root => {}
                Some(Some(_)) => {
                    self.selectors.insert(selector, None);
                }
                Some(None) => {}
            }
        }
    }
}

fn provider_candidates(result: &str) -> Option<BTreeSet<Candidate>> {
    let value = serde_json::from_str(result).ok()?;
    let mut candidates = BTreeSet::new();
    collect_candidates(&value, &mut candidates)?;
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

fn collect_candidates(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_candidates(value, candidates)?;
            }
        }
        Value::Object(values) => {
            for (field, value) in values {
                match field.as_str() {
                    "qualified_name" | "qualifiedName" => {
                        let value = canonical_qualified_name(value.as_str()?)?;
                        let function = terminal_function_name(&value)?;
                        insert(
                            candidates,
                            DecisionAnchorTargetKindV1::QualifiedName,
                            DecisionAnchorTargetKindV1::Pattern,
                            value.clone(),
                        )?;
                        insert(
                            candidates,
                            DecisionAnchorTargetKindV1::QualifiedName,
                            DecisionAnchorTargetKindV1::QualifiedName,
                            value,
                        )?;
                        insert(
                            candidates,
                            DecisionAnchorTargetKindV1::QualifiedName,
                            DecisionAnchorTargetKindV1::FunctionName,
                            function,
                        )?;
                    }
                    "function_name" | "functionName" => {
                        let value = canonical_function_name(value.as_str()?)?;
                        insert(
                            candidates,
                            DecisionAnchorTargetKindV1::FunctionName,
                            DecisionAnchorTargetKindV1::FunctionName,
                            value,
                        )?;
                    }
                    _ => collect_candidates(value, candidates)?,
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn insert(
    candidates: &mut BTreeSet<Candidate>,
    source_kind: DecisionAnchorTargetKindV1,
    target_kind: DecisionAnchorTargetKindV1,
    value: String,
) -> Option<()> {
    source_kind.can_carry_forward(target_kind).then_some(())?;
    candidates
        .insert(Candidate {
            kind: target_kind,
            value,
        })
        .then_some(())
}

fn canonical_qualified_name(value: &str) -> Option<String> {
    let normalized = GraphCorrelationV1::normalize_target(value)?;
    let components = normalized.split("::").collect::<Vec<_>>();
    (components.len() >= 2
        && components
            .iter()
            .all(|component| valid_identifier(component)))
    .then_some(normalized)
}

fn canonical_function_name(value: &str) -> Option<String> {
    let normalized = GraphCorrelationV1::normalize_target(value)?;
    let terminal = normalized.rsplit("::").next()?;
    valid_identifier(terminal).then(|| terminal.to_string())
}

fn terminal_function_name(qualified_name: &str) -> Option<String> {
    canonical_function_name(qualified_name)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use temper_protocol_activity::{GraphCorrelationTargetKindV1, GraphCorrelationToolV1};

    fn correlation(kind: GraphCorrelationTargetKindV1) -> GraphCorrelationV1 {
        let tool = match kind {
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => {
                GraphCorrelationToolV1::SearchGraph
            }
            GraphCorrelationTargetKindV1::Pattern => GraphCorrelationToolV1::SearchCode,
            GraphCorrelationTargetKindV1::FunctionName => GraphCorrelationToolV1::TracePath,
            GraphCorrelationTargetKindV1::QualifiedName => GraphCorrelationToolV1::GetCodeSnippet,
        };
        GraphCorrelationV1::new(tool, kind, "declared target").unwrap()
    }

    #[test]
    fn qualified_symbols_carry_to_equivalent_function_and_qualified_selectors() {
        let mut lineages = DecisionAnchorLineages::default();
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                r#"[{"qualifiedName":"crate::engine::run","path":"private/src/lib.rs"}]"#,
            )
            .unwrap();
        assert_eq!(root.stage, DecisionAnchorLineageStageV1::Root);
        assert_eq!(
            root.result_target_kinds,
            vec![
                DecisionAnchorTargetKindV1::Pattern,
                DecisionAnchorTargetKindV1::FunctionName,
                DecisionAnchorTargetKindV1::QualifiedName,
            ]
        );

        let function = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "run"}),
                r#"{"qualified_name":"crate::engine::run"}"#,
            )
            .unwrap();
        assert_eq!(function.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(function.root_binding, root.root_binding);

        let qualified = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate::engine::run"}),
                r#"{"function_name":"run"}"#,
            )
            .unwrap();
        assert_eq!(qualified.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(qualified.root_binding, root.root_binding);
    }

    #[test]
    fn malformed_duplicate_or_oversized_provider_records_do_not_create_carry_forwards() {
        let mut cases = vec![
            r#"[{"qualified_name":"crate::engine::run"},{"qualified_name":"crate::engine::run"}]"#
                .to_string(),
            r#"{"qualified_name": ["not a string"]}"#.to_string(),
            r#"{"unknown_target":"crate::engine::run"}"#.to_string(),
        ];
        cases.push(
            serde_json::json!({
                "results": (0..=MAX_RESULT_TARGETS)
                    .map(|index| serde_json::json!({
                        "qualified_name": format!("crate::engine::run_{index}"),
                    }))
                    .collect::<Vec<_>>(),
            })
            .to_string(),
        );
        for result in cases {
            let mut lineages = DecisionAnchorLineages::default();
            let root = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                    &serde_json::json!({"query": "start"}),
                    &result,
                )
                .unwrap();
            assert!(root.result_target_kinds.is_empty());
            let later = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::FunctionName),
                    &serde_json::json!({"function_name": "run"}),
                    r#"{}"#,
                )
                .unwrap();
            assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
            assert_ne!(later.root_binding, root.root_binding);
        }
    }
}
