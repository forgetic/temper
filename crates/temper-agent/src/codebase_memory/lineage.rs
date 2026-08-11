//! Process-local provider-result matching for typed decision-anchor lineage.
//!
//! Only bounded, typed MCP result parts are inspected here. Their values never
//! leave the wrapper; the policy receives only the opaque root and canonical
//! target-kind aggregate in `DecisionAnchorLineageV1`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use temper_agent_core::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
};
use temper_protocol_activity::{GraphCorrelationTargetKindV1, GraphCorrelationV1};
use uuid::Uuid;

use crate::mcp::McpToolResultPart;

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
        typed_parts: Option<&[McpToolResultPart]>,
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
        let result_target_kinds = match provider_candidates(typed_parts) {
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

/// Extracts candidates only from the provider-neutral result representations
/// exercised by the benchmark. Arbitrary nested JSON is deliberately ignored:
/// only nested `results`, short symbols, caller lists, related-source
/// references, and source metadata may contribute selectors.
fn provider_candidates(typed_parts: Option<&[McpToolResultPart]>) -> Option<BTreeSet<Candidate>> {
    let typed_parts = typed_parts?;
    let mut candidates = BTreeSet::new();
    for part in typed_parts {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        // Non-text blocks and non-JSON text remain fully model-visible, but
        // cannot manufacture a typed lineage candidate. A malformed text
        // block or structured part invalidates the complete typed collection.
        let Some(value) = value else {
            continue;
        };
        collect_result(&value, &mut candidates)?;
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

/// Only valid MCP text blocks can provide JSON lineage candidates.
fn content_part_json(block: &Value) -> Option<Option<Value>> {
    let block = block.as_object()?;
    match block.get("type")?.as_str()? {
        "text" => block
            .get("text")?
            .as_str()
            .map(|text| serde_json::from_str(text).ok()),
        _ => Some(None),
    }
}

fn collect_result(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_result_item(value, candidates)?;
            }
        }
        Value::Object(values) => collect_result_record(values, candidates)?,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn collect_result_item(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_result_item(value, candidates)?;
            }
        }
        Value::Object(values) => collect_result_record(values, candidates)?,
        Value::String(value) => insert_reference(candidates, value)?,
        Value::Null | Value::Bool(_) | Value::Number(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn collect_result_record(
    values: &serde_json::Map<String, Value>,
    candidates: &mut BTreeSet<Candidate>,
) -> Option<()> {
    collect_direct_symbol(values, candidates)?;

    for (field, value) in values {
        match field.as_str() {
            "results" => collect_result(value, candidates)?,
            "callers" | "caller_list" | "callerList" | "caller_functions" | "callerFunctions"
            | "symbols" | "short_symbols" | "shortSymbols" => {
                collect_reference_list(value, candidates)?
            }
            "related_source_references"
            | "relatedSourceReferences"
            | "related_source_refs"
            | "relatedSourceRefs"
            | "related_sources"
            | "relatedSources" => collect_reference_list(value, candidates)?,
            "next_target" | "nextTarget" => collect_reference(value, candidates)?,
            "source_metadata" | "sourceMetadata" => collect_source_metadata(value, candidates)?,
            "symbol" if value.is_object() => collect_reference(value, candidates)?,
            _ => {}
        }
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn collect_direct_symbol(
    values: &serde_json::Map<String, Value>,
    candidates: &mut BTreeSet<Candidate>,
) -> Option<()> {
    let qualified = match one_symbol_field(values, &["qualified_name", "qualifiedName"])? {
        Some(value) => Some(canonical_qualified_name(&value)?),
        None => None,
    };
    let short = match one_symbol_field(
        values,
        &[
            "function_name",
            "functionName",
            "short_symbol",
            "shortSymbol",
            "short_name",
            "shortName",
            "symbol_name",
            "symbolName",
            "symbol",
        ],
    )? {
        Some(value) => Some(canonical_function_name(&value)?),
        None => None,
    };

    match (qualified, short) {
        (Some(qualified), Some(short)) if terminal_function_name(&qualified)? != short => None,
        (Some(qualified), _) => insert_qualified(candidates, qualified),
        (None, Some(short)) => insert_function(candidates, short),
        (None, None) => Some(()),
    }
}

fn one_symbol_field(
    values: &serde_json::Map<String, Value>,
    fields: &[&str],
) -> Option<Option<String>> {
    let mut value = None;
    for field in fields {
        let Some(candidate) = values.get(*field) else {
            continue;
        };
        // `symbol` may itself be a structured reference; its object form is
        // handled by `collect_result_record` rather than being coerced.
        if *field == "symbol" && candidate.is_object() {
            continue;
        }
        let candidate = candidate.as_str()?.to_string();
        if value.replace(candidate).is_some() {
            return None;
        }
    }
    Some(value)
}

fn collect_reference_list(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    for value in value.as_array()? {
        collect_reference(value, candidates)?;
    }
    Some(())
}

fn collect_source_metadata(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    collect_result_record(value.as_object()?, candidates)
}

fn collect_reference(value: &Value, candidates: &mut BTreeSet<Candidate>) -> Option<()> {
    match value {
        Value::String(value) => insert_reference(candidates, value),
        Value::Object(value) => collect_result_record(value, candidates),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
    }
}

fn insert_reference(candidates: &mut BTreeSet<Candidate>, value: &str) -> Option<()> {
    match canonical_qualified_name(value) {
        Some(value) => insert_qualified(candidates, value),
        None => insert_function(candidates, canonical_function_name(value)?),
    }
}

fn insert_qualified(candidates: &mut BTreeSet<Candidate>, value: String) -> Option<()> {
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
    )
}

fn insert_function(candidates: &mut BTreeSet<Candidate>, value: String) -> Option<()> {
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::FunctionName,
        value,
    )
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

    fn text_parts(value: Value) -> Vec<McpToolResultPart> {
        vec![McpToolResultPart::Content(serde_json::json!({
            "type": "text",
            "text": value.to_string(),
        }))]
    }

    fn structured_parts(value: Value) -> Vec<McpToolResultPart> {
        vec![McpToolResultPart::StructuredContent(value)]
    }

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
        let root_parts = text_parts(serde_json::json!([
            {"qualifiedName":"crate::engine::run","path":"private/src/lib.rs"}
        ]));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
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

        let function_parts = text_parts(serde_json::json!({"qualified_name":"crate::engine::run"}));
        let function = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "run"}),
                Some(&function_parts),
            )
            .unwrap();
        assert_eq!(function.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(function.root_binding, root.root_binding);

        let qualified_parts = text_parts(serde_json::json!({"function_name":"run"}));
        let qualified = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::QualifiedName),
                &serde_json::json!({"qualified_name": "crate::engine::run"}),
                Some(&qualified_parts),
            )
            .unwrap();
        assert_eq!(qualified.stage, DecisionAnchorLineageStageV1::CarryForward);
        assert_eq!(qualified.root_binding, root.root_binding);
    }

    #[test]
    fn approved_structured_result_parts_carry_nested_symbols_callers_sources_and_metadata() {
        let mut lineages = DecisionAnchorLineages::default();
        let root_parts = structured_parts(serde_json::json!({
            "results": [
                {"results": [{"symbol": "run"}]},
                {"callers": [{"qualifiedName": "crate::engine::caller"}]},
                {"related_source_references": [{"qualified_name": "crate::engine::source"}]},
                {"source_metadata": {
                    "next_target": {"qualifiedName": "crate::engine::behavior"},
                    "source": "PRIVATE-SOURCE"
                }}
            ]
        }));
        let root = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "start"}),
                Some(&root_parts),
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

        for (kind, input) in [
            (
                GraphCorrelationTargetKindV1::FunctionName,
                serde_json::json!({"function_name": "run"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::caller"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::source"}),
            ),
            (
                GraphCorrelationTargetKindV1::QualifiedName,
                serde_json::json!({"qualified_name": "crate::engine::behavior"}),
            ),
        ] {
            let no_candidates = structured_parts(serde_json::json!({}));
            let carried = lineages
                .record(&correlation(kind), &input, Some(&no_candidates))
                .unwrap();
            assert_eq!(carried.stage, DecisionAnchorLineageStageV1::CarryForward);
            assert_eq!(carried.root_binding, root.root_binding);
        }

        let rendered = serde_json::to_string(&root).unwrap();
        for private in ["crate::engine", "PRIVATE-SOURCE"] {
            assert!(!rendered.contains(private));
        }
    }

    #[test]
    fn malformed_duplicate_or_oversized_provider_records_do_not_create_carry_forwards() {
        let mut cases = vec![
            serde_json::json!([
                {"qualified_name":"crate::engine::run"},
                {"qualified_name":"crate::engine::run"}
            ]),
            serde_json::json!({"qualified_name": ["not a string"]}),
            serde_json::json!({"unknown_target":"crate::engine::run"}),
            serde_json::json!({"callers": "not a list"}),
        ];
        cases.push(serde_json::json!({
            "results": (0..=MAX_RESULT_TARGETS)
                .map(|index| serde_json::json!({
                    "qualified_name": format!("crate::engine::run_{index}"),
                }))
                .collect::<Vec<_>>(),
        }));
        for result in cases {
            let mut lineages = DecisionAnchorLineages::default();
            let root_parts = text_parts(result);
            let root = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                    &serde_json::json!({"query": "start"}),
                    Some(&root_parts),
                )
                .unwrap();
            assert!(root.result_target_kinds.is_empty());
            let no_candidates = structured_parts(serde_json::json!({}));
            let later = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::FunctionName),
                    &serde_json::json!({"function_name": "run"}),
                    Some(&no_candidates),
                )
                .unwrap();
            assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
            assert_ne!(later.root_binding, root.root_binding);
        }
    }

    #[test]
    fn malformed_or_nontext_result_parts_cannot_create_carry_forwards() {
        let cases = [
            McpToolResultPart::Content(serde_json::json!({
                "text": r#"{\"symbol\":\"run\"}"#,
            })),
            McpToolResultPart::Content(serde_json::json!({
                "type": "text",
                "text": {"symbol": "run"},
            })),
            McpToolResultPart::Content(serde_json::json!({
                "type": "resource",
                "text": r#"{\"symbol\":\"run\"}"#,
            })),
            McpToolResultPart::StructuredContent(serde_json::json!([{"symbol": "run"}])),
        ];

        for part in cases {
            let mut lineages = DecisionAnchorLineages::default();
            let root_parts = vec![part];
            let root = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                    &serde_json::json!({"query": "start"}),
                    Some(&root_parts),
                )
                .unwrap();
            assert!(root.result_target_kinds.is_empty());

            let no_candidates = structured_parts(serde_json::json!({}));
            let later = lineages
                .record(
                    &correlation(GraphCorrelationTargetKindV1::FunctionName),
                    &serde_json::json!({"function_name": "run"}),
                    Some(&no_candidates),
                )
                .unwrap();
            assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
        }
    }

    #[test]
    fn duplicate_cross_root_candidates_become_ineligible() {
        let mut lineages = DecisionAnchorLineages::default();
        let first_parts = structured_parts(serde_json::json!({"symbol": "shared"}));
        let first = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "first"}),
                Some(&first_parts),
            )
            .unwrap();
        let second_parts = structured_parts(serde_json::json!({"symbol": "shared"}));
        let second = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::GraphQuery),
                &serde_json::json!({"query": "second"}),
                Some(&second_parts),
            )
            .unwrap();
        assert_ne!(first.root_binding, second.root_binding);

        let no_candidates = structured_parts(serde_json::json!({}));
        let later = lineages
            .record(
                &correlation(GraphCorrelationTargetKindV1::FunctionName),
                &serde_json::json!({"function_name": "shared"}),
                Some(&no_candidates),
            )
            .unwrap();
        assert_eq!(later.stage, DecisionAnchorLineageStageV1::Root);
        assert_ne!(later.root_binding, first.root_binding);
        assert_ne!(later.root_binding, second.root_binding);
    }
}
