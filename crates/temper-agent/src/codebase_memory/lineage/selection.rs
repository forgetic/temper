//! Exact selector registration and staged caller extraction.

use super::*;

pub(super) fn implementation_root_candidates(
    candidates: &[Candidate],
    focused_tests: Option<&BTreeSet<Candidate>>,
) -> Vec<Candidate> {
    let Some(focused_tests) = focused_tests else {
        return candidates.to_vec();
    };
    candidates
        .iter()
        .filter(|candidate| {
            let Some(candidate_digests) = canonical_target_digests(&candidate.value) else {
                return false;
            };
            focused_tests.iter().all(|focused_test| {
                canonical_target_digests(&focused_test.value)
                    .is_none_or(|focused_digests| candidate_digests.is_disjoint(&focused_digests))
            })
        })
        .cloned()
        .collect()
}

impl SelectorBinding {
    pub(super) fn new(root_binding: String, canonical_target_digests: BTreeSet<String>) -> Self {
        Self {
            root_binding,
            canonical_target_digests,
            implementation_evidence_result: false,
            caller_traversal_result: false,
            caller_evidence_result: false,
            focused_test_result: false,
            focused_test_confirmation_required: false,
            recovery_reference_required: false,
            implementation_traversal_readiness: ImplementationTraversalReadiness::Ready,
        }
    }
}

impl DecisionAnchorLineages {
    pub(super) fn selector_for_input(
        &self,
        target_kind: GraphCorrelationTargetKindV1,
        input: &Value,
    ) -> Option<Selector> {
        let selector_kind = DecisionAnchorTargetKindV1::from_graph_correlation(target_kind);
        let raw_value = match target_kind {
            GraphCorrelationTargetKindV1::FunctionName => input.get("function_name"),
            GraphCorrelationTargetKindV1::QualifiedName => input.get("qualified_name"),
            GraphCorrelationTargetKindV1::Pattern => input.get("pattern"),
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => None,
        }
        .and_then(Value::as_str)?;
        if raw_value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX) {
            return self
                .recovery_reference_selectors
                .get(raw_value)
                .and_then(|reference| reference.selector(selector_kind));
        }
        let value = match target_kind {
            GraphCorrelationTargetKindV1::FunctionName => canonical_function_name(raw_value),
            GraphCorrelationTargetKindV1::QualifiedName | GraphCorrelationTargetKindV1::Pattern => {
                canonical_qualified_name(raw_value).or_else(|| canonical_function_name(raw_value))
            }
            GraphCorrelationTargetKindV1::GraphQuery
            | GraphCorrelationTargetKindV1::NamePattern
            | GraphCorrelationTargetKindV1::QualifiedNamePattern => unreachable!(),
        }?;
        Some(Selector {
            kind: selector_kind,
            value,
        })
    }

    pub(super) fn admitted_evidence_kind(
        &self,
        requested: Option<DecisionEvidenceKindV1>,
        selector: Option<&Selector>,
        binding: Option<&SelectorBinding>,
        typed_parts: Option<&[McpToolResultPart]>,
    ) -> Option<DecisionEvidenceKindV1> {
        requested.filter(|kind| {
            binding.is_some_and(|binding| match kind {
                DecisionEvidenceKindV1::Implementation => {
                    binding.implementation_evidence_result
                        || !self.root_selectors.values().any(|candidate| {
                            candidate.root_binding == binding.root_binding
                                && candidate.implementation_evidence_result
                        })
                }
                DecisionEvidenceKindV1::Caller => binding.caller_traversal_result,
                DecisionEvidenceKindV1::FocusedTest => {
                    binding.focused_test_result
                        && (!binding.focused_test_confirmation_required
                            || selector.is_some_and(|selector| {
                                focused_test::source_confirms_exact_test(selector, typed_parts)
                            }))
                }
            })
        })
    }

    pub(super) fn register(
        &mut self,
        root: &str,
        candidates: BTreeSet<Candidate>,
        recovery_reference_required: bool,
    ) -> Option<()> {
        for candidate in candidates {
            let canonical_target_digests = canonical_target_digests(&candidate.value)?;
            let selector = Selector {
                kind: candidate.kind,
                value: candidate.value,
            };
            let mut root_binding =
                SelectorBinding::new(root.to_string(), canonical_target_digests.clone());
            root_binding.recovery_reference_required = recovery_reference_required;
            match self
                .root_selectors
                .entry((root.to_string(), selector.clone()))
            {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(root_binding);
                }
                std::collections::btree_map::Entry::Occupied(entry)
                    if entry.get().canonical_target_digests == canonical_target_digests => {}
                std::collections::btree_map::Entry::Occupied(_) => return None,
            }
            match self.selectors.get(&selector) {
                None => {
                    let mut binding =
                        SelectorBinding::new(root.to_string(), canonical_target_digests);
                    binding.recovery_reference_required = recovery_reference_required;
                    self.selectors.insert(selector, Some(binding));
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
        result_candidates: &BTreeSet<Candidate>,
        origin: SelectorOrigin,
        trace_provider_value: Option<&str>,
    ) -> Option<()> {
        let selector = self.selector_for_input(target_kind, input)?;
        let binding = self
            .root_selectors
            .get(&(root.to_string(), selector.clone()))?;
        let identity_digests = binding.canonical_target_digests.clone();
        let equivalents = self
            .root_selectors
            .iter()
            .filter_map(|((binding_root, selector), binding)| {
                (binding_root == root
                    && !binding
                        .canonical_target_digests
                        .is_disjoint(&identity_digests))
                .then(|| {
                    let candidate = result_candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.kind == selector.kind && candidate.value == selector.value
                        })
                        .min_by_key(|candidate| candidate.provider_kind != candidate.kind);
                    Candidate {
                        kind: selector.kind,
                        provider_kind: candidate
                            .map(|candidate| candidate.provider_kind)
                            .unwrap_or_else(|| {
                                if selector.kind == DecisionAnchorTargetKindV1::QualifiedName
                                    && canonical_qualified_name(&selector.value).is_none()
                                {
                                    DecisionAnchorTargetKindV1::FunctionName
                                } else {
                                    selector.kind
                                }
                            }),
                        value: selector.value.clone(),
                        provider_value: candidate
                            .map(|candidate| candidate.provider_value.clone())
                            .unwrap_or_else(|| selector.value.clone()),
                    }
                })
            })
            .collect();
        self.mark_candidates(root, equivalents, origin)?;
        if matches!(origin, SelectorOrigin::ImplementationEvidenceResult { .. })
            && target_kind == GraphCorrelationTargetKindV1::QualifiedName
        {
            let provider_value = input.get("qualified_name")?.as_str()?;
            (!provider_value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX)).then_some(())?;
            let key = RecoverySelectorKey {
                root_binding: root.to_string(),
                purpose: RecoverySelectorPurpose::ImplementationTrace,
            };
            let references = self.recovery_references.get(&key)?.clone();
            let reference = references
                .iter()
                .find(|reference| {
                    self.recovery_reference_selectors
                        .get(*reference)
                        .and_then(|recovery| recovery.source_selector.as_ref())
                        == Some(&selector)
                })
                .or_else(|| references.first())?;
            let recovery = self.recovery_reference_selectors.get_mut(reference)?;
            recovery.provider_value = trace_provider_value.unwrap_or(provider_value).to_string();
            recovery.source_selector = Some(selector);
            recovery.source_provider_value = Some(provider_value.to_string());
        }
        Some(())
    }
}

pub(super) fn provider_function_name_for_source(
    typed_parts: Option<&[McpToolResultPart]>,
    input: &Value,
) -> Option<String> {
    let selected = input
        .get("qualified_name")
        .and_then(Value::as_str)
        .and_then(|value| {
            canonical_qualified_name(value).or_else(|| canonical_function_name(value))
        })?;
    provider_explicit_source_names(typed_parts).remove(&selected)
}

pub(super) fn provider_explicit_source_names(
    typed_parts: Option<&[McpToolResultPart]>,
) -> BTreeMap<String, String> {
    let mut names = BTreeMap::<String, Option<String>>::new();
    for part in typed_parts.unwrap_or_default() {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then_some(value.clone())
            }
            McpToolResultPart::Content(block) => content_part_json(block).flatten(),
        };
        if let Some(value) = value {
            collect_explicit_source_names(&value, &mut names);
        }
    }
    names
        .into_iter()
        .filter_map(|(qualified, name)| name.map(|name| (qualified, name)))
        .collect()
}

fn collect_explicit_source_names(value: &Value, names: &mut BTreeMap<String, Option<String>>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_explicit_source_names(value, names);
            }
        }
        Value::Object(values) => {
            let qualified = values
                .get("qualified_name")
                .or_else(|| values.get("qualifiedName"))
                .and_then(Value::as_str)
                .and_then(|value| {
                    canonical_qualified_name(value).or_else(|| canonical_function_name(value))
                });
            let name = values
                .get("name")
                .and_then(Value::as_str)
                .and_then(canonical_function_name);
            if let (Some(qualified), Some(name)) = (qualified, name) {
                names
                    .entry(qualified)
                    .and_modify(|existing| {
                        if existing.as_ref() != Some(&name) {
                            *existing = None;
                        }
                    })
                    .or_insert(Some(name));
            }
            for field in ["results", "source_metadata", "sourceMetadata"] {
                if let Some(value) = values.get(field) {
                    collect_explicit_source_names(value, names);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ImplementationTraversalEvidence {
    Ready,
    Partial,
    CallerIdentity,
}

pub(super) fn implementation_traversal_evidence(
    typed_parts: Option<&[McpToolResultPart]>,
) -> ImplementationTraversalEvidence {
    let mut positive_caller_count = false;
    let mut caller_identity = false;
    for part in typed_parts.unwrap_or_default() {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                value.is_object().then_some(value.clone())
            }
            McpToolResultPart::Content(block) => content_part_json(block).flatten(),
        };
        if let Some(value) = value {
            collect_traversal_readiness(&value, &mut positive_caller_count, &mut caller_identity);
        }
    }
    if caller_identity {
        ImplementationTraversalEvidence::CallerIdentity
    } else if positive_caller_count {
        ImplementationTraversalEvidence::Partial
    } else {
        ImplementationTraversalEvidence::Ready
    }
}

fn collect_traversal_readiness(
    value: &Value,
    positive_caller_count: &mut bool,
    caller_identity: &mut bool,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_traversal_readiness(value, positive_caller_count, caller_identity);
            }
        }
        Value::Object(values) => {
            for (field, value) in values {
                match field.as_str() {
                    "callers" | "caller_list" | "callerList" | "caller_functions"
                    | "callerFunctions" => {
                        if value.as_u64().is_some_and(|count| count > 0) {
                            *positive_caller_count = true;
                        } else if let Some(values) = value.as_array() {
                            let mut candidates = BTreeMap::new();
                            if collect_reference_list(value, &mut candidates).is_some()
                                && !values.is_empty()
                                && !candidates.is_empty()
                            {
                                *caller_identity = true;
                            }
                        }
                    }
                    "caller_names" | "callerNames" => {
                        if value.as_array().is_some_and(|names| {
                            !names.is_empty()
                                && names.iter().all(|name| {
                                    name.as_str().and_then(canonical_function_name).is_some()
                                })
                        }) {
                            *caller_identity = true;
                        }
                    }
                    "results" | "source_metadata" | "sourceMetadata" => {
                        collect_traversal_readiness(value, positive_caller_count, caller_identity);
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
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

pub(super) fn canonical_target_digests(value: &str) -> Option<BTreeSet<String>> {
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

pub(super) fn canonical_qualified_name(value: &str) -> Option<String> {
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

pub(super) fn canonical_function_name(value: &str) -> Option<String> {
    let normalized = GraphCorrelationV1::normalize_target(value)?;
    let normalized = normalized.replace("::", ".");
    let terminal = normalized.rsplit('.').next()?;
    valid_identifier(terminal).then(|| terminal.to_string())
}

pub(super) fn terminal_function_name(qualified_name: &str) -> Option<String> {
    canonical_function_name(qualified_name)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
