//! Exact selector registration and staged caller extraction.

use super::*;

impl DecisionAnchorLineages {
    pub(super) fn selector_for_input(
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

    pub(super) fn register(&mut self, root: &str, candidates: BTreeSet<Candidate>) -> Option<()> {
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
                            implementation_evidence_result: false,
                            caller_traversal_result: false,
                            caller_evidence_result: false,
                            focused_test_result: false,
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

    pub(super) fn mark_input_selector(
        &mut self,
        target_kind: GraphCorrelationTargetKindV1,
        input: &Value,
        root: &str,
        origin: SelectorOrigin,
    ) -> Option<()> {
        let selector = self.selector_for_input(target_kind, input)?;
        let binding = self.selectors.get(&selector)?.as_ref()?;
        (binding.root_binding == root).then_some(())?;
        let identity_digests = binding.canonical_target_digests.clone();
        let equivalents = self
            .selectors
            .iter()
            .filter_map(|(selector, binding)| {
                binding.as_ref().and_then(|binding| {
                    (binding.root_binding == root
                        && !binding
                            .canonical_target_digests
                            .is_disjoint(&identity_digests))
                    .then_some(Candidate {
                        kind: selector.kind,
                        value: selector.value.clone(),
                    })
                })
            })
            .collect();
        self.mark_candidates(root, equivalents, origin)
    }
}

pub(super) fn provider_caller_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<BTreeSet<Candidate>> {
    let mut candidates = BTreeMap::new();
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_caller_result(&value, &mut candidates)?;
        }
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

fn collect_caller_result(value: &Value, candidates: &mut BTreeMap<Candidate, u8>) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_caller_result(value, candidates)?;
            }
        }
        Value::Object(values) => {
            for (field, value) in values {
                match field.as_str() {
                    "callers" | "caller_list" | "callerList" | "caller_functions"
                    | "callerFunctions" => collect_reference_list_or_count(value, candidates)?,
                    "results" | "source_metadata" | "sourceMetadata" => {
                        collect_caller_result(value, candidates)?
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}
