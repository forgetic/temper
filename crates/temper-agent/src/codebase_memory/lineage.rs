//! Process-local provider-result matching for typed decision-anchor lineage.
//!
//! Only bounded, typed MCP result parts are inspected here. Their values never
//! leave the wrapper; the policy receives only the opaque root and canonical
//! target-kind aggregate in `DecisionAnchorLineageV1`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionAnchorLineageV1, DecisionAnchorTargetKindV1,
    GraphCorrelationTargetKindV1, GraphCorrelationV1,
};
use uuid::Uuid;

use crate::mcp::McpToolResultPart;

const MAX_RESULT_TARGETS: usize = 64;

#[derive(Default)]
pub(super) struct DecisionAnchorLineages {
    /// `None` marks an ambiguous value. Once more than one root has offered a
    /// representation, no later model selection may use that representation to
    /// advance either root.
    selectors: BTreeMap<Selector, Option<SelectorBinding>>,
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectorBinding {
    root_binding: String,
    canonical_target_digests: BTreeSet<String>,
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
        let matched = self
            .selector_for_input(correlation.target_kind, input)
            .and_then(|selector| self.selectors.get(&selector).cloned().flatten());
        let (root_binding, stage, canonical_target_digests) = match matched {
            Some(binding) => (
                binding.root_binding,
                DecisionAnchorLineageStageV1::CarryForward,
                binding.canonical_target_digests,
            ),
            None => (
                Uuid::new_v4().to_string(),
                DecisionAnchorLineageStageV1::Root,
                BTreeSet::new(),
            ),
        };

        // Any malformed, duplicate, unsupported, or oversized provider record
        // contributes no carry-forward values. The current successful result is
        // still a root/carry record, but it cannot unlock an additional hop.
        let result_target_kinds = match provider_candidates(typed_parts) {
            Some(candidates) => {
                let kinds = candidates.iter().map(|candidate| candidate.kind).collect();
                self.register(&root_binding, candidates)?;
                kinds
            }
            None => BTreeSet::new(),
        };
        DecisionAnchorLineageV1::new_with_canonical_target_digests(
            root_binding,
            stage,
            target_kind,
            result_target_kinds,
            canonical_target_digests,
        )
    }

    fn selector_for_input(
        &self,
        target_kind: GraphCorrelationTargetKindV1,
        input: &Value,
    ) -> Option<Selector> {
        let selector_kind = DecisionAnchorTargetKindV1::from_graph_correlation(target_kind);
        let value = match target_kind {
            GraphCorrelationTargetKindV1::FunctionName => input
                .get("function_name")
                .and_then(Value::as_str)
                .and_then(canonical_function_name),
            GraphCorrelationTargetKindV1::QualifiedName => input
                .get("qualified_name")
                .and_then(Value::as_str)
                .and_then(|value| {
                    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
                }),
            GraphCorrelationTargetKindV1::Pattern => input
                .get("pattern")
                .and_then(Value::as_str)
                .and_then(|value| {
                    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
                }),
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => None,
        }?;
        Some(Selector {
            kind: selector_kind,
            value,
        })
    }

    fn register(&mut self, root: &str, candidates: BTreeSet<Candidate>) -> Option<()> {
        for candidate in candidates {
            let canonical_target_digests = canonical_target_digests(&candidate.value)?;
            let selector = Selector {
                kind: candidate.kind,
                value: candidate.value,
            };
            match self.selectors.get(&selector) {
                None => {
                    self.selectors.insert(
                        selector,
                        Some(SelectorBinding {
                            root_binding: root.to_string(),
                            canonical_target_digests,
                        }),
                    );
                }
                Some(Some(existing))
                    if existing.root_binding == root
                        && existing.canonical_target_digests == canonical_target_digests => {}
                Some(Some(_)) => {
                    self.selectors.insert(selector, None);
                }
                Some(None) => {}
            }
        }
        Some(())
    }
}

/// Extracts candidates only from the provider-neutral result representations
/// exercised by the benchmark. Arbitrary nested JSON is deliberately ignored:
/// only nested `results`, short symbols, caller lists, related-source
/// references, and source metadata may contribute selectors.
fn provider_candidates(typed_parts: Option<&[McpToolResultPart]>) -> Option<BTreeSet<Candidate>> {
    let typed_parts = typed_parts?;
    let mut candidates = BTreeMap::new();
    let mut content_values = Vec::new();
    for part in typed_parts {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                let value = value.is_object().then(|| Some(value.clone()))?;
                // MCP servers commonly mirror one result as both JSON text
                // content and structuredContent. Skip only that exact
                // cross-representation mirror. Candidate uniqueness is about
                // provider identities rather than equivalent selectors, so
                // representation mirrors cannot make a returned symbol
                // ineligible.
                if value
                    .as_ref()
                    .is_some_and(|value| content_values.contains(value))
                {
                    continue;
                }
                value
            }
            McpToolResultPart::Content(block) => {
                let value = content_part_json(block)?;
                if let Some(value) = &value {
                    content_values.push(value.clone());
                }
                value
            }
        };
        // Non-text blocks and non-JSON text remain fully model-visible, but
        // cannot manufacture a typed lineage candidate. A malformed text
        // block or structured part invalidates the complete typed collection.
        let Some(value) = value else {
            continue;
        };
        collect_result(&value, &mut candidates)?;
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
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

fn collect_result(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
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

fn collect_result_item(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
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
    candidates: &mut BTreeMap<Candidate, u8>,
) -> Option<()> {
    collect_direct_symbol(values, candidates)?;

    for (field, value) in values {
        match field.as_str() {
            "results" => collect_result(value, candidates)?,
            "callers" | "caller_list" | "callerList" | "caller_functions" | "callerFunctions"
            | "symbols" | "short_symbols" | "shortSymbols" => {
                collect_reference_list_or_count(value, candidates)?
            }
            "related_source_references"
            | "relatedSourceReferences"
            | "related_source_refs"
            | "relatedSourceRefs"
            | "related_sources"
            | "relatedSources" => collect_reference_list(value, candidates)?,
            "next_target" | "nextTarget" | "function" => collect_reference(value, candidates)?,
            "source_metadata" | "sourceMetadata" => collect_source_metadata(value, candidates)?,
            "symbol" if value.is_object() => collect_reference(value, candidates)?,
            _ => {}
        }
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn collect_direct_symbol(
    values: &serde_json::Map<String, Value>,
    candidates: &mut BTreeMap<Candidate, u8>,
) -> Option<()> {
    let qualified_field = one_symbol_field(values, &["qualified_name", "qualifiedName"])?;
    let (qualified, short_from_qualified_field, invalid_qualified_field) = match &qualified_field {
        Some(value) => match canonical_qualified_name(value) {
            Some(qualified) => (Some(qualified), None, false),
            // The approved provider shape may label a short implementation
            // symbol as `qualified_name`. Preserve its useful closed
            // function representation instead of rejecting the entire
            // otherwise typed result collection.
            None => match canonical_function_name(value) {
                Some(short) => (None, Some(short), false),
                // Native graph records can prefix a qualified identity with a
                // package name that is not a Rust identifier (for example a
                // hyphenated package). Its adjacent short `name` is the
                // approved identity in that shape; without that field this
                // value remains ineligible.
                None => (None, None, true),
            },
        },
        None => (None, None, false),
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
    let selected_short = match (short_from_qualified_field, short) {
        (Some(from_qualified), Some(short)) if from_qualified == short => Some(short),
        // Explicit provider symbol fields must agree. A display `name` is
        // considered only as a fallback below, so it cannot contradict an
        // otherwise complete qualified identity.
        (Some(_), Some(_)) => return None,
        (Some(short), None) | (None, Some(short)) => Some(short),
        (None, None) => None,
    };
    let selected_short = if qualified.is_none() && selected_short.is_none() {
        // `name` is a provider record's short symbol only beside an explicit
        // qualified field. A standalone display name must not manufacture a
        // decision candidate in arbitrary metadata.
        if qualified_field.is_some() {
            match one_symbol_field(values, &["name"])? {
                Some(value) => Some(canonical_function_name(&value)?),
                None => None,
            }
        } else {
            None
        }
    } else {
        selected_short
    };
    if invalid_qualified_field && selected_short.is_none() {
        return None;
    }

    match (qualified, selected_short) {
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

fn collect_reference_list(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
    for value in value.as_array()? {
        collect_reference(value, candidates)?;
    }
    Some(())
}

fn collect_reference_list_or_count(
    value: &Value,
    candidates: &mut BTreeMap<Candidate, u8>,
) -> Option<()> {
    if value.is_u64() {
        // Source metadata reports caller cardinality under the same field name
        // used by trace results for an actual caller list.
        Some(())
    } else {
        collect_reference_list(value, candidates)
    }
}

fn collect_source_metadata(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
    collect_result_record(value.as_object()?, candidates)
}

fn collect_reference(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
    match value {
        Value::String(value) => insert_reference(candidates, value),
        Value::Object(value) => collect_result_record(value, candidates),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
    }
}

fn insert_reference(candidates: &mut BTreeMap<Candidate, u8>, value: &str) -> Option<()> {
    match canonical_qualified_name(value) {
        Some(value) => insert_qualified(candidates, value),
        None => insert_function(candidates, canonical_function_name(value)?),
    }
}

fn insert_qualified(candidates: &mut BTreeMap<Candidate, u8>, value: String) -> Option<()> {
    let function = terminal_function_name(&value)?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
    )?;
    // Pattern selectors commonly use the terminal symbol returned beside a
    // provider-qualified identity. Retain that closed representation too;
    // the registry's ambiguity handling prevents a shared terminal name from
    // binding across distinct roots.
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::Pattern,
        function.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
    )?;
    // A source-read wrapper accepts a short `qualified_name` selector. Keep
    // the provider record's direct name as that closed representation too.
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        function.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::FunctionName,
        function,
    )
}

fn insert_function(candidates: &mut BTreeMap<Candidate, u8>, value: String) -> Option<()> {
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
        value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
    )
}

fn insert(
    candidates: &mut BTreeMap<Candidate, u8>,
    source_kind: DecisionAnchorTargetKindV1,
    target_kind: DecisionAnchorTargetKindV1,
    value: String,
) -> Option<()> {
    source_kind.can_carry_forward(target_kind).then_some(())?;
    // A provider may report one identity through multiple approved fields
    // (for example `qualified_name`, a terminal `name`, and a `symbol` list).
    // They are equivalent representations of the same result, not ambiguous
    // independently returned candidates. The registry still rejects a
    // selector that later appears under a distinct root.
    candidates
        .entry(Candidate {
            kind: target_kind,
            value,
        })
        .or_insert(0);
    Some(())
}

fn canonical_target_digests(value: &str) -> Option<BTreeSet<String>> {
    let qualified = canonical_qualified_name(value);
    let components = qualified
        .as_deref()
        .unwrap_or(value)
        .split("::")
        .collect::<Vec<_>>();
    let start = components.len().saturating_sub(3);
    components[start..]
        .iter()
        .enumerate()
        .map(|(index, _)| {
            GraphCorrelationV1::target_digest(&components[start + index..].join("::"))
        })
        .collect()
}

fn canonical_qualified_name(value: &str) -> Option<String> {
    let normalized = GraphCorrelationV1::normalize_target(value)?;
    // The production provider uses dotted graph identities while other MCP
    // adapters use Rust-style `::` paths. Normalize both approved qualified
    // representations to one opaque registry key; paths, prose, and mixed
    // punctuation still fail the identifier check below.
    let normalized = normalized.replace("::", ".");
    let components = normalized.split('.').collect::<Vec<_>>();
    (components.len() >= 2
        && components.iter().all(|component| {
            // Provider project identities may be dashed at any namespace
            // level. They are transport/local identity segments, not Rust
            // symbols; accept their bounded ASCII form while retaining strict
            // identifier validation for every other component.
            valid_provider_package_component(component)
        }))
    .then(|| components.join("::"))
}

fn valid_provider_package_component(value: &str) -> bool {
    valid_identifier(value)
        || (value.len() > 2
            && !value.starts_with('-')
            && !value.ends_with('-')
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
}

fn canonical_function_name(value: &str) -> Option<String> {
    let normalized = GraphCorrelationV1::normalize_target(value)?;
    let normalized = normalized.replace("::", ".");
    let terminal = normalized.rsplit('.').next()?;
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
