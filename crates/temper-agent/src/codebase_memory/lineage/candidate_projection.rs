//! Provider-order retention for complete typed graph results.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct Candidate {
    pub(super) kind: DecisionAnchorTargetKindV1,
    pub(super) provider_kind: DecisionAnchorTargetKindV1,
    pub(super) value: String,
    /// The exact provider spelling is not part of candidate identity.
    pub(super) provider_value: String,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.provider_kind == other.provider_kind
            && self.value == other.value
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.kind, self.provider_kind, &self.value).cmp(&(
            other.kind,
            other.provider_kind,
            &other.value,
        ))
    }
}

pub(super) fn insert_reference<C: CandidateCollection>(
    candidates: &mut C,
    value: &str,
) -> Option<()> {
    match canonical_qualified_name(value) {
        Some(canonical) => insert_qualified(candidates, canonical, value.to_string()),
        None => insert_function(
            candidates,
            canonical_function_name(value)?,
            value.to_string(),
        ),
    }
}

pub(super) fn insert_qualified<C: CandidateCollection>(
    candidates: &mut C,
    value: String,
    provider_value: String,
) -> Option<()> {
    let function = terminal_function_name(&value)?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
        provider_value.clone(),
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
        provider_value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
        provider_value.clone(),
    )?;
    // A source-read wrapper accepts a short `qualified_name` selector. Keep
    // the provider record's direct name as that closed representation too.
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::QualifiedName,
        function.clone(),
        provider_value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::QualifiedName,
        DecisionAnchorTargetKindV1::FunctionName,
        function,
        provider_value,
    )
}

pub(super) fn insert_function<C: CandidateCollection>(
    candidates: &mut C,
    value: String,
    provider_value: String,
) -> Option<()> {
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::Pattern,
        value.clone(),
        provider_value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::FunctionName,
        value.clone(),
        provider_value.clone(),
    )?;
    insert(
        candidates,
        DecisionAnchorTargetKindV1::FunctionName,
        DecisionAnchorTargetKindV1::QualifiedName,
        value,
        provider_value,
    )
}

fn insert<C: CandidateCollection>(
    candidates: &mut C,
    source_kind: DecisionAnchorTargetKindV1,
    target_kind: DecisionAnchorTargetKindV1,
    value: String,
    provider_value: String,
) -> Option<()> {
    source_kind.can_carry_forward(target_kind).then_some(())?;
    // A provider may report one identity through multiple approved fields
    // (for example `qualified_name`, a terminal `name`, and a `symbol` list).
    // They are equivalent representations of the same result, not ambiguous
    // independently returned candidates. The registry still rejects a
    // selector that later appears under a distinct root.
    candidates.insert_candidate(Candidate {
        kind: target_kind,
        provider_kind: source_kind,
        value,
        provider_value,
    });
    Some(())
}

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
