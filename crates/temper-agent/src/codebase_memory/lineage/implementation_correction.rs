//! One bounded replacement of provisional implementation authority.

use super::*;

impl DecisionAnchorLineages {
    pub(super) fn clear_implementation_selection(&mut self, root: &str) {
        for binding in self.selectors.values_mut().flatten() {
            if binding.root_binding == root {
                binding.implementation_evidence_result = false;
            }
        }
        for ((binding_root, _), binding) in &mut self.root_selectors {
            if binding_root == root {
                binding.implementation_evidence_result = false;
            }
        }
    }

    pub(super) fn activate_implementation_corrections(
        &mut self,
        root_binding: &str,
        related: &BTreeSet<Candidate>,
    ) -> bool {
        if !self.provisional_implementation_roots.contains(root_binding)
            || self.corrected_implementation_roots.contains(root_binding)
        {
            return false;
        }
        let related_digests = related
            .iter()
            .filter_map(|candidate| canonical_target_digests(&candidate.value))
            .flatten()
            .collect::<BTreeSet<_>>();
        let key = RecoverySelectorKey {
            root_binding: root_binding.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationCorrection,
        };
        for reference in self.recovery_references.get(&key).into_iter().flatten() {
            let Some(candidate) = self.recovery_reference_selectors.get_mut(reference) else {
                continue;
            };
            let supported = candidate
                .source_selector
                .as_ref()
                .and_then(|selector| canonical_target_digests(&selector.value))
                .is_some_and(|digests| !digests.is_disjoint(&related_digests));
            candidate.correction_supported |= supported;
        }
        self.recovery_references
            .get(&key)
            .is_some_and(|references| {
                references.iter().any(|reference| {
                    self.recovery_reference_selectors
                        .get(reference)
                        .is_some_and(|candidate| candidate.correction_supported)
                })
            })
    }
}

pub(super) fn provider_trace_implementation_candidates(
    input: &Value,
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<BTreeSet<Candidate>> {
    if input
        .get("mode")
        .and_then(Value::as_str)
        .is_some_and(|mode| mode != "calls")
        || input
            .get("include_tests")
            .and_then(Value::as_bool)
            .is_some_and(|include| include)
    {
        return None;
    }
    let direction = input
        .get("direction")
        .and_then(Value::as_str)
        .unwrap_or("inbound");
    let (inbound, outbound) = match direction {
        "inbound" => (true, false),
        "outbound" => (false, true),
        "both" => (true, true),
        _ => return None,
    };
    let mut candidates = BTreeMap::new();
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then(|| Some(value.clone()))?
            }
            McpToolResultPart::Content(block) => content_part_json(block)?,
        };
        if let Some(value) = value {
            collect_trace_relation_result(&value, &mut candidates, inbound, outbound)?;
        }
    }
    let candidates = candidates.into_keys().collect::<BTreeSet<_>>();
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(candidates)
}

fn collect_trace_relation_result(
    value: &Value,
    candidates: &mut BTreeMap<Candidate, u8>,
    inbound: bool,
    outbound: bool,
) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_trace_relation_result(value, candidates, inbound, outbound)?;
            }
        }
        Value::Object(values) => {
            for (field, value) in values {
                let selected = inbound
                    && matches!(
                        field.as_str(),
                        "callers"
                            | "caller_list"
                            | "callerList"
                            | "caller_functions"
                            | "callerFunctions"
                    )
                    || outbound
                        && matches!(
                            field.as_str(),
                            "callees"
                                | "callee_list"
                                | "calleeList"
                                | "callee_functions"
                                | "calleeFunctions"
                        );
                if selected {
                    collect_reference_list_or_count(value, candidates)?;
                } else if matches!(
                    field.as_str(),
                    "results" | "source_metadata" | "sourceMetadata"
                ) {
                    collect_trace_relation_result(value, candidates, inbound, outbound)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (candidates.len() <= MAX_RESULT_TARGETS).then_some(())
}
