//! Opaque provider-derived selector references used only inside one agent run.

use super::*;
use temper_agent_core::EligibleLineageAdmission;

pub(super) const RECOVERY_SELECTOR_REFERENCE_PREFIX: &str = "temper-recovery-selector:";
pub(super) const MAX_VISIBLE_RECOVERY_CANDIDATES_PER_PURPOSE: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum RecoverySelectorPurpose {
    ImplementationCandidate,
    ImplementationTrace,
    CallerSource,
    CallerTestTraversal,
    FocusedTestSource,
}

impl RecoverySelectorPurpose {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ImplementationCandidate => "implementation_candidate",
            Self::ImplementationTrace => "implementation_evidence_result",
            Self::CallerSource => "caller_traversal_result",
            Self::CallerTestTraversal => "caller_evidence_result",
            Self::FocusedTestSource => "focused_test_result",
        }
    }

    pub(super) const fn selector_kind(self) -> DecisionAnchorTargetKindV1 {
        match self {
            Self::ImplementationCandidate
            | Self::ImplementationTrace
            | Self::CallerTestTraversal => DecisionAnchorTargetKindV1::FunctionName,
            Self::CallerSource | Self::FocusedTestSource => {
                DecisionAnchorTargetKindV1::QualifiedName
            }
        }
    }

    const fn is_source_candidate(self) -> bool {
        matches!(
            self,
            Self::ImplementationCandidate | Self::CallerSource | Self::FocusedTestSource
        )
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct RecoverySelectorKey {
    pub(super) root_binding: String,
    pub(super) purpose: RecoverySelectorPurpose,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoverySelectorState {
    Available,
    Reserved,
    Consumed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecoverySelectorReference {
    pub(super) root_binding: String,
    pub(super) purpose: RecoverySelectorPurpose,
    pub(super) selector: Selector,
    pub(super) provider_value: String,
    /// Stable order of the provider result mapped to this opaque identity.
    pub(super) provider_result_order: usize,
    pub(super) source_selector: Option<Selector>,
    pub(super) source_provider_value: Option<String>,
    pub(super) state: RecoverySelectorState,
    pub(super) presented: bool,
}

impl RecoverySelectorReference {
    pub(super) fn selector(&self, kind: DecisionAnchorTargetKindV1) -> Option<Selector> {
        if self.selector.kind == kind {
            Some(self.selector.clone())
        } else {
            self.source_selector
                .as_ref()
                .filter(|selector| selector.kind == kind)
                .cloned()
        }
    }

    pub(super) fn provider_value(&self, kind: DecisionAnchorTargetKindV1) -> Option<&str> {
        if self.selector.kind == kind {
            Some(&self.provider_value)
        } else if self.source_selector.as_ref()?.kind == kind {
            self.source_provider_value.as_deref()
        } else {
            None
        }
    }
}

pub(in crate::codebase_memory) struct CandidateRecovery {
    pub(in crate::codebase_memory) guidance: Option<String>,
    pub(in crate::codebase_memory) has_alternative: bool,
}

impl DecisionAnchorLineages {
    pub(super) fn replace_recovery_references(
        &mut self,
        root: &str,
        purpose: RecoverySelectorPurpose,
        candidates: &[Candidate],
        provider_ordered: bool,
    ) {
        let key = RecoverySelectorKey {
            root_binding: root.to_string(),
            purpose,
        };
        if let Some(stale) = self.recovery_references.remove(&key) {
            for reference in stale {
                self.recovery_reference_selectors.remove(&reference);
            }
        }

        let mut ordered = candidates.iter().collect::<Vec<_>>();
        if !provider_ordered {
            ordered.sort_by_key(|candidate| {
                let qualified = canonical_qualified_name(&candidate.value).is_some();
                if candidate.kind == DecisionAnchorTargetKindV1::QualifiedName
                    && candidate.provider_kind == DecisionAnchorTargetKindV1::QualifiedName
                    && qualified
                {
                    0
                } else if candidate.kind == purpose.selector_kind()
                    && candidate.provider_kind == purpose.selector_kind()
                {
                    1
                } else if candidate.kind == DecisionAnchorTargetKindV1::QualifiedName && qualified {
                    2
                } else if candidate.kind == purpose.selector_kind() {
                    3
                } else {
                    4
                }
            });
        }

        let mut retained = Vec::new();
        let mut identities = BTreeSet::new();
        for candidate in ordered {
            let Some(selector_reference) = self.reference_for_candidate(root, purpose, candidate)
            else {
                continue;
            };
            let identity = (
                selector_reference.selector.clone(),
                selector_reference.source_selector.clone(),
            );
            let exact_selector = identity.1.as_ref().unwrap_or(&identity.0);
            let is_short_alias = canonical_qualified_name(&exact_selector.value).is_none();
            if is_short_alias
                && identities.iter().any(
                    |(selector, source_selector): &(Selector, Option<Selector>)| {
                        let existing = source_selector.as_ref().unwrap_or(selector);
                        canonical_function_name(&existing.value)
                            == canonical_function_name(&exact_selector.value)
                            && canonical_qualified_name(&existing.value).is_some()
                    },
                )
            {
                continue;
            }
            if !identities.insert(identity) {
                continue;
            }
            let reference = format!(
                "{RECOVERY_SELECTOR_REFERENCE_PREFIX}{}",
                uuid::Uuid::new_v4()
            );
            self.recovery_reference_selectors
                .insert(reference.clone(), selector_reference);
            retained.push(reference);
            if !provider_ordered && retained.len() == MAX_VISIBLE_RECOVERY_CANDIDATES_PER_PURPOSE {
                break;
            }
        }
        if !retained.is_empty() {
            self.recovery_references.insert(key, retained);
        }
    }

    pub(super) fn prefer_explicit_source_names(
        &mut self,
        root: &str,
        purpose: RecoverySelectorPurpose,
        explicit_names: &BTreeMap<String, String>,
    ) {
        let key = RecoverySelectorKey {
            root_binding: root.to_string(),
            purpose,
        };
        for reference in self.recovery_references.get(&key).into_iter().flatten() {
            let Some(reference) = self.recovery_reference_selectors.get_mut(reference) else {
                continue;
            };
            if let Some(name) = explicit_names.get(&reference.selector.value) {
                reference.provider_value = name.clone();
            }
        }
    }

    fn reference_for_candidate(
        &self,
        root: &str,
        purpose: RecoverySelectorPurpose,
        candidate: &Candidate,
    ) -> Option<RecoverySelectorReference> {
        let qualified = canonical_qualified_name(&candidate.value);
        let function = canonical_function_name(&candidate.value)?;
        let (selector, provider_value, source_selector, source_provider_value) = match purpose {
            RecoverySelectorPurpose::ImplementationCandidate => {
                let source_value = qualified.clone().unwrap_or_else(|| function.clone());
                let source_selector = Selector {
                    kind: DecisionAnchorTargetKindV1::QualifiedName,
                    value: source_value,
                };
                (
                    Selector {
                        kind: DecisionAnchorTargetKindV1::FunctionName,
                        value: function.clone(),
                    },
                    function,
                    Some(source_selector),
                    Some(candidate.provider_value.clone()),
                )
            }
            RecoverySelectorPurpose::ImplementationTrace
            | RecoverySelectorPurpose::CallerTestTraversal => {
                let source_selector = qualified.map(|value| Selector {
                    kind: DecisionAnchorTargetKindV1::QualifiedName,
                    value,
                });
                (
                    Selector {
                        kind: DecisionAnchorTargetKindV1::FunctionName,
                        value: function.clone(),
                    },
                    function,
                    source_selector.clone(),
                    source_selector.map(|_| candidate.value.clone()),
                )
            }
            RecoverySelectorPurpose::CallerSource | RecoverySelectorPurpose::FocusedTestSource => {
                let selector = Selector {
                    kind: DecisionAnchorTargetKindV1::QualifiedName,
                    value: qualified.unwrap_or(function),
                };
                (selector, candidate.provider_value.clone(), None, None)
            }
        };
        let required_selector = if purpose == RecoverySelectorPurpose::ImplementationCandidate {
            source_selector.as_ref()?
        } else {
            &selector
        };
        self.root_selectors
            .contains_key(&(root.to_string(), required_selector.clone()))
            .then_some(RecoverySelectorReference {
                root_binding: root.to_string(),
                purpose,
                selector,
                provider_value,
                provider_result_order: candidate.provider_result_order,
                source_selector,
                source_provider_value,
                state: RecoverySelectorState::Available,
                presented: false,
            })
    }

    /// Reserves one exact current-root implementation trace reference. This
    /// check deliberately precedes canonical selector lookup and traversal
    /// readiness: possession of the live reference is the run-local authority
    /// for exactly one provider dispatch.
    pub(in crate::codebase_memory) fn reserve_implementation_trace_reference(
        &mut self,
        input: &Value,
        active_root: Option<&str>,
    ) -> Result<Option<EligibleLineageAdmission>, ()> {
        let Some(reference_value) = input.get("function_name").and_then(Value::as_str) else {
            return Ok(None);
        };
        if !reference_value.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX) {
            return Ok(None);
        }
        let reference = self
            .recovery_reference_selectors
            .get(reference_value)
            .ok_or(())?;
        if reference.purpose != RecoverySelectorPurpose::ImplementationTrace {
            return (reference.purpose == RecoverySelectorPurpose::CallerTestTraversal)
                .then_some(None)
                .ok_or(());
        }
        let object = input.as_object().ok_or(())?;
        if [
            "query",
            "name_pattern",
            "qn_pattern",
            "pattern",
            "function_name",
            "qualified_name",
        ]
        .into_iter()
        .filter(|field| object.contains_key(*field))
        .count()
            != 1
            || object
                .get("include_tests")
                .is_some_and(|value| value.as_bool() != Some(false))
            || object
                .get("direction")
                .is_some_and(|value| value.as_str() != Some("inbound"))
            || object
                .get("mode")
                .is_some_and(|value| value.as_str() != Some("calls"))
        {
            return Err(());
        }
        let active_root = active_root.ok_or(())?;
        let key = RecoverySelectorKey {
            root_binding: active_root.to_string(),
            purpose: RecoverySelectorPurpose::ImplementationTrace,
        };
        if !self
            .recovery_references
            .get(&key)
            .is_some_and(|references| references.iter().any(|value| value == reference_value))
        {
            return Err(());
        }
        let reference = self
            .recovery_reference_selectors
            .get_mut(reference_value)
            .ok_or(())?;
        if reference.root_binding != active_root
            || reference.purpose != RecoverySelectorPurpose::ImplementationTrace
            || reference.selector.kind != DecisionAnchorTargetKindV1::FunctionName
            || reference.state != RecoverySelectorState::Available
            || !reference.presented
        {
            return Err(());
        }
        reference.state = RecoverySelectorState::Reserved;
        EligibleLineageAdmission::implementation_caller_traversal(
            active_root.to_string(),
            DecisionAnchorTargetKindV1::FunctionName,
        )
        .map(Some)
        .ok_or(())
    }

    pub(in crate::codebase_memory) fn expand_recovery_selector(
        &mut self,
        tool_name: &str,
        input: &mut Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<ExpandedRecoverySelector>, ()> {
        let Some((field, reference)) = self.recovery_selector(tool_name, input, evidence_kind)?
        else {
            return Ok(None);
        };
        let kind = match field {
            "function_name" => DecisionAnchorTargetKindV1::FunctionName,
            "qualified_name" => DecisionAnchorTargetKindV1::QualifiedName,
            _ => return Err(()),
        };
        let public_reference = input[field].as_str().ok_or(())?.to_string();
        let provider_value = reference.provider_value(kind).ok_or(())?.to_string();
        let is_trace_reference = GraphCorrelationToolV1::from_public_name(tool_name)
            == Some(GraphCorrelationToolV1::TracePath);
        if is_trace_reference && reference.state != RecoverySelectorState::Reserved {
            return Err(());
        }
        let root_binding = reference.root_binding.clone();
        let reference = self
            .recovery_reference_selectors
            .get_mut(&public_reference)
            .ok_or(())?;
        if is_trace_reference {
            reference.state = RecoverySelectorState::Consumed;
        } else if reference.purpose.is_source_candidate() {
            if reference.state != RecoverySelectorState::Available {
                return Err(());
            }
            reference.state = RecoverySelectorState::Reserved;
        }
        input[field] = Value::String(provider_value);
        Ok(Some(ExpandedRecoverySelector {
            reference: public_reference,
            root_binding,
            decision_evidence_kind: evidence_kind,
        }))
    }

    pub(in crate::codebase_memory) fn complete_candidate_reference(
        &mut self,
        expanded: &ExpandedRecoverySelector,
        preserve_alternatives: bool,
    ) -> Option<CandidateRecovery> {
        let candidate = self
            .recovery_reference_selectors
            .get(&expanded.reference)
            .filter(|reference| reference.purpose.is_source_candidate())?;
        let key = RecoverySelectorKey {
            root_binding: candidate.root_binding.clone(),
            purpose: candidate.purpose,
        };
        self.recovery_reference_selectors
            .remove(&expanded.reference);
        if preserve_alternatives {
            let remove_key = self
                .recovery_references
                .get_mut(&key)
                .is_some_and(|references| {
                    references.retain(|reference| reference != &expanded.reference);
                    references.is_empty()
                });
            if remove_key {
                self.recovery_references.remove(&key);
            }
        } else if let Some(references) = self.recovery_references.remove(&key) {
            for reference in references {
                self.recovery_reference_selectors.remove(&reference);
            }
        }
        let has_alternative = self
            .recovery_references
            .get(&key)
            .is_some_and(|references| {
                references.iter().any(|reference| {
                    self.recovery_reference_selectors
                        .get(reference)
                        .is_some_and(|reference| {
                            reference.state == RecoverySelectorState::Available
                        })
                })
            });
        let guidance = has_alternative.then(|| {
            let references = self
                .recovery_references
                .get(&key)
                .into_iter()
                .flatten()
                .filter(|reference| {
                    self.recovery_reference_selectors
                        .get(*reference)
                        .is_some_and(|reference| {
                            reference.state == RecoverySelectorState::Available
                        })
                })
                .take(MAX_VISIBLE_RECOVERY_CANDIDATES_PER_PURPOSE)
                .cloned()
                .collect::<Vec<_>>();
            for reference in &references {
                if let Some(selector) = self.recovery_reference_selectors.get_mut(reference) {
                    selector.presented = true;
                }
            }
            let labels = references
                .iter()
                .enumerate()
                .map(|(index, reference)| {
                    let label = if references.len() == 1 {
                        key.purpose.label().to_string()
                    } else {
                        format!("{}_{}", key.purpose.label(), index + 1)
                    };
                    let provider_order = self
                        .recovery_reference_selectors
                        .get(reference)
                        .map(|selector| selector.provider_result_order)
                        .unwrap_or_default();
                    if key.purpose == RecoverySelectorPurpose::ImplementationCandidate {
                        format!(
                            "{label}={reference} (provider_result_order={provider_order})"
                        )
                    } else {
                        format!("{label}={reference}")
                    }
                })
                .collect::<Vec<_>>();
            format!(
                "[Candidate recovery: selected current-root candidate missed; no evidence or conventional mutation authority was earned; remaining compatible references: {}. Copy one reference exactly into the same selector field in a later model turn.]",
                labels.join(", ")
            )
        });
        Some(CandidateRecovery {
            guidance,
            has_alternative,
        })
    }

    pub(super) fn is_implementation_trace_reference(&self, input: &Value) -> bool {
        input
            .get("qualified_name")
            .and_then(Value::as_str)
            .and_then(|reference| self.recovery_reference_selectors.get(reference))
            .is_some_and(|reference| {
                reference.purpose == RecoverySelectorPurpose::ImplementationTrace
            })
    }

    pub(super) fn validate_recovery_selector(
        &self,
        tool_name: &str,
        input: &Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<(), ()> {
        self.recovery_selector(tool_name, input, evidence_kind)
            .map(|_| ())
    }

    pub(super) fn recovery_selector<'a>(
        &'a self,
        tool_name: &str,
        input: &Value,
        evidence_kind: Option<DecisionEvidenceKindV1>,
    ) -> Result<Option<(&'static str, &'a RecoverySelectorReference)>, ()> {
        let Some(tool) = GraphCorrelationToolV1::from_public_name(tool_name) else {
            return Ok(None);
        };
        let field = match tool {
            GraphCorrelationToolV1::SearchGraph => "query",
            GraphCorrelationToolV1::SearchCode => "pattern",
            GraphCorrelationToolV1::TracePath => "function_name",
            GraphCorrelationToolV1::GetCodeSnippet => "qualified_name",
        };
        let Some(public_reference) = input.get(field).and_then(Value::as_str) else {
            return Ok(None);
        };
        if !public_reference.starts_with(RECOVERY_SELECTOR_REFERENCE_PREFIX) {
            return Ok(None);
        }
        let expected_selector_kind = match tool {
            GraphCorrelationToolV1::TracePath => DecisionAnchorTargetKindV1::FunctionName,
            GraphCorrelationToolV1::GetCodeSnippet => DecisionAnchorTargetKindV1::QualifiedName,
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {
                return Err(());
            }
        };
        let reference = self
            .recovery_reference_selectors
            .get(public_reference)
            .ok_or(())?;
        let purpose_matches = match tool {
            GraphCorrelationToolV1::TracePath => match input.get("include_tests") {
                Some(Value::Bool(true)) => {
                    reference.purpose == RecoverySelectorPurpose::CallerTestTraversal
                }
                Some(Value::Bool(false)) | None => {
                    reference.purpose == RecoverySelectorPurpose::ImplementationTrace
                }
                Some(_) => return Err(()),
            },
            GraphCorrelationToolV1::GetCodeSnippet => match evidence_kind {
                Some(DecisionEvidenceKindV1::Implementation) => matches!(
                    reference.purpose,
                    RecoverySelectorPurpose::ImplementationCandidate
                        | RecoverySelectorPurpose::ImplementationTrace
                ),
                Some(DecisionEvidenceKindV1::Caller) => {
                    reference.purpose == RecoverySelectorPurpose::CallerSource
                }
                Some(DecisionEvidenceKindV1::FocusedTest) => {
                    reference.purpose == RecoverySelectorPurpose::FocusedTestSource
                }
                None => return Err(()),
            },
            GraphCorrelationToolV1::SearchGraph | GraphCorrelationToolV1::SearchCode => {
                unreachable!()
            }
        };
        (purpose_matches
            && reference.presented
            && reference.selector(expected_selector_kind).is_some()
            && reference.state != RecoverySelectorState::Consumed)
            .then_some(Some((field, reference)))
            .ok_or(())
    }
}
