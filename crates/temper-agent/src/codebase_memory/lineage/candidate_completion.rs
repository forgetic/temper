//! Completion and retirement of bounded source-candidate references.

use super::recovery_selector::MAX_VISIBLE_RECOVERY_CANDIDATES_PER_PURPOSE;
use super::*;

impl DecisionAnchorLineages {
    pub(in crate::codebase_memory) fn complete_candidate_reference(
        &mut self,
        expanded: &ExpandedRecoverySelector,
        preserve_alternatives: bool,
    ) -> Option<CandidateRecovery> {
        if expanded.candidate_preview {
            return None;
        }
        let candidate = self
            .recovery_reference_selectors
            .get(&expanded.reference)
            .filter(|reference| reference.purpose.is_source_candidate())?
            .clone();
        let key = RecoverySelectorKey {
            root_binding: candidate.root_binding.clone(),
            purpose: candidate.purpose,
        };
        if !preserve_alternatives
            && candidate.purpose == RecoverySelectorPurpose::ImplementationCandidate
            && candidate.previewed
        {
            self.retain_provisional_implementation_corrections(&key, &expanded.reference);
        } else {
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
        }
        if !preserve_alternatives
            && candidate.purpose == RecoverySelectorPurpose::ImplementationCorrection
        {
            self.provisional_implementation_roots
                .remove(&candidate.root_binding);
            self.corrected_implementation_roots
                .insert(candidate.root_binding.clone());
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

    fn retain_provisional_implementation_corrections(
        &mut self,
        implementation_key: &RecoverySelectorKey,
        selected: &str,
    ) {
        let correction_key = RecoverySelectorKey {
            root_binding: implementation_key.root_binding.clone(),
            purpose: RecoverySelectorPurpose::ImplementationCorrection,
        };
        let mut retained = Vec::new();
        for reference in self
            .recovery_references
            .remove(implementation_key)
            .unwrap_or_default()
        {
            if reference == selected {
                self.recovery_reference_selectors.remove(&reference);
                continue;
            }
            let Some(candidate) = self.recovery_reference_selectors.get_mut(&reference) else {
                continue;
            };
            if candidate.presented && candidate.state == RecoverySelectorState::Available {
                candidate.purpose = RecoverySelectorPurpose::ImplementationCorrection;
                candidate.correction_supported = false;
                retained.push(reference);
            } else {
                self.recovery_reference_selectors.remove(&reference);
            }
        }
        if retained.is_empty() {
            return;
        }
        self.recovery_references.insert(correction_key, retained);
        self.provisional_implementation_roots
            .insert(implementation_key.root_binding.clone());
    }
}
