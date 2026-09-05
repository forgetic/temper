//! Exact raw-selector matching across retained decision-forest candidates.

use temper_protocol_activity::GraphRecoveryEvidenceKindV1;

use super::*;

#[derive(Debug)]
pub(super) enum ExactRawSelectorError {
    Invalid,
    ActiveRootTraceFallback,
}

pub(super) struct ExactRawSelector {
    pub(super) reference: String,
    pub(super) root_binding: String,
    pub(super) action: GraphRecoveryActionV1,
    pub(super) evidence_kind: Option<DecisionEvidenceKindV1>,
    pub(super) reference_required: bool,
}

impl ExactRawSelector {
    pub(super) fn canonical_arguments(&self, arguments: &Value) -> Value {
        let mut canonical = arguments.clone();
        let field = match self.action.selector_kind {
            DecisionAnchorTargetKindV1::FunctionName => "function_name",
            DecisionAnchorTargetKindV1::QualifiedName => "qualified_name",
            DecisionAnchorTargetKindV1::Pattern
            | DecisionAnchorTargetKindV1::GraphQuery
            | DecisionAnchorTargetKindV1::NamePattern
            | DecisionAnchorTargetKindV1::QualifiedNamePattern => return canonical,
        };
        canonical[field] = Value::String(self.reference.clone());
        if let Some(kind) = self.evidence_kind {
            canonical["decision_evidence_kind"] =
                serde_json::to_value(kind).expect("closed evidence kind serializes");
        }
        canonical
    }
}

impl RecoverySelectorPurpose {
    fn action_for_call(
        self,
        tool: GraphCorrelationToolV1,
        arguments: &serde_json::Map<String, Value>,
        declared: Option<DecisionEvidenceKindV1>,
    ) -> Option<(GraphRecoveryActionV1, Option<DecisionEvidenceKindV1>)> {
        match tool {
            GraphCorrelationToolV1::GetCodeSnippet => {
                let inferred = match self {
                    Self::ImplementationCandidate | Self::ImplementationTrace => {
                        DecisionEvidenceKindV1::Implementation
                    }
                    Self::CallerSource => DecisionEvidenceKindV1::Caller,
                    Self::FocusedTestSource => DecisionEvidenceKindV1::FocusedTest,
                    Self::CallerTestTraversal => return None,
                };
                declared.is_none_or(|kind| kind == inferred).then(|| {
                    (
                        GraphRecoveryActionV1::for_evidence(match inferred {
                            DecisionEvidenceKindV1::Implementation => {
                                GraphRecoveryEvidenceKindV1::Implementation
                            }
                            DecisionEvidenceKindV1::Caller => GraphRecoveryEvidenceKindV1::Caller,
                            DecisionEvidenceKindV1::FocusedTest => {
                                GraphRecoveryEvidenceKindV1::FocusedTest
                            }
                        }),
                        Some(inferred),
                    )
                })
            }
            GraphCorrelationToolV1::TracePath => {
                if declared.is_some()
                    || arguments
                        .get("mode")
                        .and_then(Value::as_str)
                        .is_some_and(|mode| mode != "calls")
                    || arguments
                        .get("direction")
                        .and_then(Value::as_str)
                        .is_some_and(|direction| direction != "inbound")
                {
                    return None;
                }
                match (self, arguments.get("include_tests")) {
                    (Self::ImplementationTrace, None | Some(Value::Bool(false))) => Some((
                        GraphRecoveryActionV1::for_evidence(GraphRecoveryEvidenceKindV1::Trace),
                        None,
                    )),
                    (Self::CallerTestTraversal, Some(Value::Bool(true))) => {
                        Some((GraphRecoveryActionV1::focused_test_traversal(), None))
                    }
                    _ => None,
                }
            }
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => None,
        }
    }
}

impl DecisionAnchorLineages {
    pub(super) fn exact_raw_selector(
        &self,
        tool_name: &str,
        arguments: &Value,
        declared_override: Option<DecisionEvidenceKindV1>,
        allow_reserved_trace: bool,
    ) -> Result<Option<ExactRawSelector>, ExactRawSelectorError> {
        let Some(tool) = GraphCorrelationToolV1::from_public_name(tool_name) else {
            return Ok(None);
        };
        if matches!(
            tool,
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode
        ) {
            return Ok(None);
        }
        let object = arguments
            .as_object()
            .ok_or(ExactRawSelectorError::Invalid)?;
        let declared = match declared_override {
            Some(kind) => Some(kind),
            None => match object.get("decision_evidence_kind") {
                Some(value) if tool == GraphCorrelationToolV1::GetCodeSnippet => Some(
                    serde_json::from_value(value.clone())
                        .map_err(|_| ExactRawSelectorError::Invalid)?,
                ),
                Some(_) => return Err(ExactRawSelectorError::Invalid),
                None => None,
            },
        };
        let selector_fields = [
            "query",
            "name_pattern",
            "qn_pattern",
            "pattern",
            "function_name",
            "qualified_name",
        ];
        let present = selector_fields
            .into_iter()
            .filter(|field| object.contains_key(*field))
            .collect::<Vec<_>>();
        if present.len() != 1 || object[present[0]].as_str().is_none() {
            return Err(ExactRawSelectorError::Invalid);
        }
        let expected_field = match tool {
            GraphCorrelationToolV1::GetCodeSnippet => "qualified_name",
            GraphCorrelationToolV1::TracePath => "function_name",
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {
                return Ok(None);
            }
        };
        let raw = object[present[0]]
            .as_str()
            .ok_or(ExactRawSelectorError::Invalid)?;
        if raw.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX) {
            return Ok(None);
        }
        let expected_kind = match tool {
            GraphCorrelationToolV1::GetCodeSnippet => DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::TracePath => DecisionAnchorTargetKindV1::FunctionName,
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {
                unreachable!()
            }
        };
        let exact_candidates = self
            .recovery_reference_selectors
            .iter()
            .filter(|(_, candidate)| {
                candidate.presented && candidate.provider_value(expected_kind) == Some(raw)
            })
            .collect::<Vec<_>>();
        if present[0] != expected_field {
            return exact_candidates
                .is_empty()
                .then_some(None)
                .ok_or(ExactRawSelectorError::Invalid);
        }
        let compatible = exact_candidates
            .iter()
            .filter_map(|(reference, candidate)| {
                (candidate.state == RecoverySelectorState::Available
                    || allow_reserved_trace
                        && tool == GraphCorrelationToolV1::TracePath
                        && candidate.state == RecoverySelectorState::Reserved)
                    .then(|| candidate.purpose.action_for_call(tool, object, declared))
                    .flatten()
                    .and_then(|(action, evidence_kind)| {
                        let selector = candidate.selector(action.selector_kind)?;
                        self.root_selectors
                            .get(&(candidate.root_binding.clone(), selector))
                            .map(|binding| binding.recovery_reference_required)
                            .map(|reference_required| ExactRawSelector {
                                reference: (*reference).clone(),
                                root_binding: candidate.root_binding.clone(),
                                action,
                                evidence_kind,
                                reference_required,
                            })
                    })
            })
            .collect::<Vec<_>>();
        match compatible.as_slice() {
            [selected] => Ok(Some(ExactRawSelector {
                reference: selected.reference.clone(),
                root_binding: selected.root_binding.clone(),
                action: selected.action,
                evidence_kind: selected.evidence_kind,
                reference_required: selected.reference_required,
            })),
            [] if exact_candidates.is_empty() => {
                let normalized_conflict =
                    self.recovery_reference_selectors.values().any(|candidate| {
                        if !candidate.presented {
                            return false;
                        }
                        if raw.trim() != raw
                            && candidate.provider_value(expected_kind) == Some(raw.trim())
                        {
                            return true;
                        }
                        let canonical = match expected_kind {
                            DecisionAnchorTargetKindV1::FunctionName => {
                                canonical_function_name(raw)
                            }
                            DecisionAnchorTargetKindV1::QualifiedName => {
                                canonical_qualified_name(raw)
                                    .or_else(|| canonical_function_name(raw))
                            }
                            _ => None,
                        };
                        canonical.is_some_and(|value| {
                            candidate.selector(expected_kind).is_some_and(|selector| {
                                self.root_selectors
                                    .get(&(candidate.root_binding.clone(), selector.clone()))
                                    .is_some_and(|binding| binding.recovery_reference_required)
                                    && selector.value == value
                            })
                        })
                    });
                if normalized_conflict {
                    Err(ExactRawSelectorError::Invalid)
                } else {
                    Ok(None)
                }
            }
            [] if tool == GraphCorrelationToolV1::TracePath
                && exact_candidates.len() == 1
                && exact_candidates[0].1.state == RecoverySelectorState::Available
                && exact_candidates[0].1.purpose
                    == RecoverySelectorPurpose::ImplementationCandidate
                && object
                    .get("mode")
                    .and_then(Value::as_str)
                    .is_none_or(|mode| mode == "calls")
                && object
                    .get("direction")
                    .and_then(Value::as_str)
                    .is_none_or(|direction| direction == "inbound")
                && object
                    .get("include_tests")
                    .is_none_or(|include_tests| include_tests.as_bool() == Some(false)) =>
            {
                Err(ExactRawSelectorError::ActiveRootTraceFallback)
            }
            [] | [_, ..] => Err(ExactRawSelectorError::Invalid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = "temper-v1-private.src.route.worker_slot";

    fn reference(root: &str) -> RecoverySelectorReference {
        RecoverySelectorReference {
            root_binding: root.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationCandidate,
            selector: Selector {
                kind: DecisionAnchorTargetKindV1::FunctionName,
                value: "worker_slot".to_string(),
            },
            provider_value: "worker_slot".to_string(),
            provider_result_order: 1,
            source_selector: Some(Selector {
                kind: DecisionAnchorTargetKindV1::QualifiedName,
                value: "temper_v1_private::src::route::worker_slot".to_string(),
            }),
            source_provider_value: Some(RAW.to_string()),
            state: RecoverySelectorState::Available,
            presented: true,
        }
    }

    fn install(lineages: &mut DecisionAnchorLineages, key: &str, root: &str) {
        let reference = reference(root);
        lineages.root_selectors.insert(
            (root.to_string(), reference.source_selector.clone().unwrap()),
            SelectorBinding {
                recovery_reference_required: true,
                ..SelectorBinding::new(root.to_string(), BTreeSet::new())
            },
        );
        lineages
            .recovery_reference_selectors
            .insert(key.to_string(), reference);
    }

    #[test]
    fn exact_provider_selector_is_unique_while_normalized_and_ambiguous_values_fail_closed() {
        let source = GraphCorrelationToolV1::GetCodeSnippet.public_name();
        let mut lineages = DecisionAnchorLineages::default();
        install(&mut lineages, "selected", "active");

        let omitted = lineages
            .exact_raw_selector(
                source,
                &serde_json::json!({"qualified_name": RAW}),
                None,
                false,
            )
            .unwrap()
            .expect("one exact candidate");
        assert_eq!(omitted.root_binding, "active");
        assert_eq!(
            omitted.evidence_kind,
            Some(DecisionEvidenceKindV1::Implementation)
        );
        assert!(
            lineages
                .exact_raw_selector(
                    source,
                    &serde_json::json!({
                        "qualified_name": RAW,
                        "decision_evidence_kind": "caller"
                    }),
                    None,
                    false,
                )
                .is_err()
        );
        assert!(
            lineages
                .exact_raw_selector(
                    source,
                    &serde_json::json!({
                        "qualified_name": "  temper-v1-private.src.route.worker_slot  "
                    }),
                    None,
                    false,
                )
                .is_err()
        );

        assert!(matches!(
            lineages.exact_raw_selector(
                GraphCorrelationToolV1::TracePath.public_name(),
                &serde_json::json!({"function_name": "worker_slot"}),
                None,
                false,
            ),
            Err(ExactRawSelectorError::ActiveRootTraceFallback)
        ));
        assert!(matches!(
            lineages.exact_raw_selector(
                GraphCorrelationToolV1::TracePath.public_name(),
                &serde_json::json!({
                    "function_name": "worker_slot",
                    "mode": "data_flow"
                }),
                None,
                false,
            ),
            Err(ExactRawSelectorError::Invalid)
        ));

        install(&mut lineages, "sibling", "sibling");
        assert!(
            lineages
                .exact_raw_selector(
                    source,
                    &serde_json::json!({"qualified_name": RAW}),
                    None,
                    false,
                )
                .is_err()
        );
        lineages
            .recovery_reference_selectors
            .get_mut("selected")
            .unwrap()
            .state = RecoverySelectorState::Consumed;
        lineages.recovery_reference_selectors.remove("sibling");
        assert!(
            lineages
                .exact_raw_selector(
                    source,
                    &serde_json::json!({"qualified_name": RAW}),
                    None,
                    false,
                )
                .is_err()
        );
    }
}
