//! Provider-order retention for complete typed graph results.

use super::*;

pub(super) struct ProviderCandidates {
    pub(super) candidates: BTreeSet<Candidate>,
    pub(super) provider_order: Vec<Candidate>,
    pub(super) projected: bool,
}

#[derive(Default)]
struct OrderedCandidateProjection {
    candidates: BTreeSet<Candidate>,
    provider_order: Vec<Candidate>,
    projected: bool,
}

pub(super) trait CandidateCollection {
    fn insert_candidate(&mut self, candidate: Candidate);
    fn within_result_limit(&self) -> bool;
}

impl CandidateCollection for BTreeMap<Candidate, u8> {
    fn insert_candidate(&mut self, candidate: Candidate) {
        self.entry(candidate).or_insert(0);
    }

    fn within_result_limit(&self) -> bool {
        self.len() <= MAX_RESULT_TARGETS
    }
}

/// Extracts candidates only from the provider-neutral result representations
/// exercised by the benchmark. The complete typed shape and provider order are
/// retained for run-local admission. Results beyond the direct-selector bound
/// are still marked projected so only opaque references can authorize them.
pub(super) fn provider_candidates(
    typed_parts: Option<&[McpToolResultPart]>,
) -> Option<ProviderCandidates> {
    let typed_parts = typed_parts?;
    let mut projection = OrderedCandidateProjection::default();
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
        collect_result(&value, &mut projection)?;
    }
    Some(ProviderCandidates {
        candidates: projection.candidates,
        provider_order: projection.provider_order,
        projected: projection.projected,
    })
}

impl CandidateCollection for OrderedCandidateProjection {
    fn insert_candidate(&mut self, candidate: Candidate) {
        if self.candidates.contains(&candidate) {
            return;
        }
        if self.candidates.len() >= MAX_RESULT_TARGETS {
            self.projected = true;
        }
        self.candidates.insert(candidate.clone());
        self.provider_order.push(candidate);
    }

    fn within_result_limit(&self) -> bool {
        true
    }
}
