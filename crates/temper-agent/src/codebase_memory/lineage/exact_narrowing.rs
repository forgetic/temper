//! Exact graph narrowing over provider-returned symbol identities and labels.

use super::*;

const MAX_GRAPH_SYMBOL_LABEL_BYTES: usize = 64;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ExactGraphSelector {
    selector: Selector,
    label: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct ExactGraphIdentity {
    function_name: String,
    qualified_name: Option<String>,
    label: String,
    canonical_target_digests: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PendingExactGraphNarrowing {
    pub(super) root_binding: String,
    pub(super) canonical_target_digests: BTreeSet<String>,
}

impl DecisionAnchorLineages {
    pub(super) fn resolve_exact_graph_narrowing(
        &mut self,
        input: &Value,
        active_root: Option<&str>,
    ) -> Result<
        temper_agent_core::EligibleLineageAdmission,
        temper_agent_core::LineageAdmissionStatus,
    > {
        use temper_agent_core::LineageAdmissionStatus::{
            AmbiguousSelector, BroadSelector, UnknownSelector,
        };

        let selector = exact_graph_selector(input).ok_or(BroadSelector)?;
        let active_root = active_root.ok_or(BroadSelector)?;
        let roots = self
            .exact_graph_selectors
            .get(&selector)
            .ok_or(UnknownSelector)?;
        let canonical_target_digests = roots
            .get(active_root)
            .ok_or(UnknownSelector)?
            .clone()
            .ok_or(AmbiguousSelector)?;
        let pending = PendingExactGraphNarrowing {
            root_binding: active_root.to_string(),
            canonical_target_digests,
        };
        match self.pending_exact_graph_narrowings.get(&selector) {
            None => {
                self.pending_exact_graph_narrowings
                    .insert(selector.clone(), Some(pending.clone()));
            }
            Some(Some(existing)) if existing == &pending => {}
            Some(Some(_)) => {
                self.pending_exact_graph_narrowings
                    .insert(selector.clone(), None);
                return Err(AmbiguousSelector);
            }
            Some(None) => return Err(AmbiguousSelector),
        }
        temper_agent_core::EligibleLineageAdmission::exact_graph_narrowing(
            pending.root_binding,
            selector.selector.kind,
        )
        .ok_or(UnknownSelector)
    }

    pub(super) fn consume_exact_graph_narrowing(
        &mut self,
        correlation: &GraphCorrelationV1,
        input: &Value,
        identities: Option<&BTreeSet<ExactGraphIdentity>>,
    ) -> Option<PendingExactGraphNarrowing> {
        if correlation.tool != GraphCorrelationToolV1::SearchGraph
            || !matches!(
                correlation.target_kind,
                GraphCorrelationTargetKindV1::NamePattern
                    | GraphCorrelationTargetKindV1::QualifiedNamePattern
            )
        {
            return None;
        }
        let selector = exact_graph_selector(input)?;
        let pending = self
            .pending_exact_graph_narrowings
            .remove(&selector)
            .flatten()?;
        let mut identities = identities?.iter();
        let identity = identities.next()?;
        if identities.next().is_some() {
            return None;
        }
        (identity.matches(&selector)
            && identity.canonical_target_digests == pending.canonical_target_digests)
            .then_some(pending)
    }

    pub(super) fn register_exact_graph_identities(
        &mut self,
        root: &str,
        identities: &BTreeSet<ExactGraphIdentity>,
    ) {
        for identity in identities {
            for selector in identity.selectors() {
                let roots = self.exact_graph_selectors.entry(selector).or_default();
                match roots.get(root) {
                    None => {
                        roots.insert(
                            root.to_string(),
                            Some(identity.canonical_target_digests.clone()),
                        );
                    }
                    Some(Some(existing)) if existing == &identity.canonical_target_digests => {}
                    Some(Some(_)) => {
                        roots.insert(root.to_string(), None);
                    }
                    Some(None) => {}
                }
            }
        }
    }

    pub(super) fn promote_exact_graph_identity(
        &mut self,
        root: &str,
        identity_digests: &BTreeSet<String>,
        candidates: &BTreeSet<Candidate>,
    ) {
        for candidate in candidates {
            let Some(candidate_digests) = canonical_target_digests(&candidate.value) else {
                continue;
            };
            if candidate_digests.is_disjoint(identity_digests) {
                continue;
            }
            self.selectors.insert(
                Selector {
                    kind: candidate.kind,
                    value: candidate.value.clone(),
                },
                Some(SelectorBinding::new(root.to_string(), candidate_digests)),
            );
        }
    }
}

impl ExactGraphIdentity {
    fn selectors(&self) -> Vec<ExactGraphSelector> {
        let mut selectors = vec![ExactGraphSelector {
            selector: Selector {
                kind: DecisionAnchorTargetKindV1::NamePattern,
                value: self.function_name.clone(),
            },
            label: self.label.clone(),
        }];
        if let Some(qualified_name) = &self.qualified_name {
            selectors.push(ExactGraphSelector {
                selector: Selector {
                    kind: DecisionAnchorTargetKindV1::QualifiedNamePattern,
                    value: qualified_name.clone(),
                },
                label: self.label.clone(),
            });
        }
        selectors
    }

    fn matches(&self, selector: &ExactGraphSelector) -> bool {
        if self.label != selector.label {
            return false;
        }
        match selector.selector.kind {
            DecisionAnchorTargetKindV1::NamePattern => {
                self.function_name == selector.selector.value
            }
            DecisionAnchorTargetKindV1::QualifiedNamePattern => self
                .qualified_name
                .as_ref()
                .is_some_and(|qualified| qualified == &selector.selector.value),
            DecisionAnchorTargetKindV1::GraphQuery
            | DecisionAnchorTargetKindV1::Pattern
            | DecisionAnchorTargetKindV1::FunctionName
            | DecisionAnchorTargetKindV1::QualifiedName => false,
        }
    }
}

fn exact_graph_selector(input: &Value) -> Option<ExactGraphSelector> {
    let object = input.as_object()?;
    let label = canonical_graph_label(object.get("label")?.as_str()?)?;
    let (kind, value) = match (
        object.get("name_pattern").and_then(Value::as_str),
        object.get("qn_pattern").and_then(Value::as_str),
    ) {
        (Some(value), None) => {
            let canonical = canonical_function_name(value)?;
            (canonical == value).then_some((DecisionAnchorTargetKindV1::NamePattern, canonical))?
        }
        (None, Some(value)) => (
            DecisionAnchorTargetKindV1::QualifiedNamePattern,
            canonical_qualified_name(value)?,
        ),
        (Some(_), Some(_)) | (None, None) => return None,
    };
    Some(ExactGraphSelector {
        selector: Selector { kind, value },
        label,
    })
}

fn canonical_graph_label(value: &str) -> Option<String> {
    (!value.is_empty()
        && value.len() <= MAX_GRAPH_SYMBOL_LABEL_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    .then(|| value.to_string())
}

pub(super) fn provider_exact_graph_identities(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<BTreeSet<ExactGraphIdentity>> {
    let mut identities = BTreeSet::new();
    let mut content_values = Vec::new();
    for part in typed_parts? {
        let value = match part {
            McpToolResultPart::StructuredContent(value) => {
                let value = value.is_object().then(|| Some(value.clone()))?;
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
        if let Some(value) = value {
            collect_exact_graph_identities(&value, &mut identities)?;
        }
    }
    (identities.len() <= MAX_RESULT_TARGETS).then_some(identities)
}

fn collect_exact_graph_identities(
    value: &Value,
    identities: &mut BTreeSet<ExactGraphIdentity>,
) -> Option<()> {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_exact_graph_identities(value, identities)?;
            }
        }
        Value::Object(values) => {
            let has_symbol_identity = [
                "qualified_name",
                "qualifiedName",
                "function_name",
                "functionName",
                "short_symbol",
                "shortSymbol",
                "short_name",
                "shortName",
                "symbol_name",
                "symbolName",
                "symbol",
            ]
            .iter()
            .any(|field| values.contains_key(*field));
            if has_symbol_identity {
                if !identities.insert(exact_graph_identity(values)?) {
                    return None;
                }
            }
            for field in ["results", "semantic_results", "semanticResults"] {
                if let Some(value) = values.get(field) {
                    collect_exact_graph_identities(value, identities)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
    }
    (identities.len() <= MAX_RESULT_TARGETS).then_some(())
}

fn exact_graph_identity(values: &serde_json::Map<String, Value>) -> Option<ExactGraphIdentity> {
    let label = canonical_graph_label(values.get("label")?.as_str()?)?;
    let mut candidates = BTreeMap::new();
    collect_direct_symbol(values, &mut candidates)?;
    let function_names = candidates
        .keys()
        .filter(|candidate| candidate.kind == DecisionAnchorTargetKindV1::FunctionName)
        .map(|candidate| candidate.value.clone())
        .collect::<BTreeSet<_>>();
    let qualified_names = candidates
        .keys()
        .filter(|candidate| {
            candidate.kind == DecisionAnchorTargetKindV1::QualifiedName
                && canonical_qualified_name(&candidate.value).is_some()
        })
        .map(|candidate| candidate.value.clone())
        .collect::<BTreeSet<_>>();
    let mut function_names = function_names.into_iter();
    let function_name = function_names.next()?;
    if function_names.next().is_some() {
        return None;
    }
    let mut qualified_names = qualified_names.into_iter();
    let qualified_name = qualified_names.next();
    if qualified_names.next().is_some() {
        return None;
    }
    let canonical_target_digests =
        canonical_target_digests(qualified_name.as_deref().unwrap_or(&function_name))?;
    Some(ExactGraphIdentity {
        function_name,
        qualified_name,
        label,
        canonical_target_digests,
    })
}
