// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use temper_protocol_activity::{
    DecisionAnchorLineageStageV1, DecisionEvidenceKindV1, FocusedTestDiscoveryOutcomeV1,
    GraphCorrelationToolV1, GraphCorrelationV1, ToolStatusV1,
};

use super::{correlation_matches_expected, decision_evidence};
use crate::{
    GraphConsumptionModeV1, GraphDecisionEvidenceV1, GraphDecisionKindV1, GraphDecisionTargetV1,
    GraphEvidenceToolV1,
};

use super::super::{CallKey, GraphCall};

/// Proves the closed wrapper-declared focused-test producer-to-source edge.
/// Provider output, source, paths, and names are deliberately unavailable here.
pub(super) fn producer_to_source_evidence(
    producer: &GraphCall,
    producer_tool: GraphEvidenceToolV1,
    producer_correlation: &GraphCorrelationV1,
    calls: &BTreeMap<CallKey, GraphCall>,
    targets: &[GraphDecisionTargetV1],
) -> Option<GraphDecisionEvidenceV1> {
    let producer_finish = producer.finish_seq?;
    let producer_lineage = producer
        .decision_anchor_lineage
        .as_ref()
        .filter(|lineage| {
            lineage.is_valid_for(producer_correlation)
                && lineage.focused_test_discovery
                    == Some(FocusedTestDiscoveryOutcomeV1::EligibleSelectorReturned)
        })?;

    let mut declaration_matches = Vec::new();
    for target in targets
        .iter()
        .filter(|target| target.kind == GraphDecisionKindV1::FocusedTest)
    {
        for consumption in &target.consumption {
            if consumption.tool != GraphCorrelationToolV1::GetCodeSnippet {
                continue;
            }
            let expected_consumer = consumption.correlation()?;
            let mut consumers = Vec::new();
            for consumer in calls.values() {
                let Some((consumer_start, consumer_correlation)) =
                    typed_source_consumer(producer, producer_finish, producer_lineage, consumer)
                else {
                    continue;
                };
                if correlation_matches_expected(consumer, consumer_correlation, &expected_consumer)
                {
                    consumers.push((consumer, consumer_start, consumer_correlation));
                }
            }
            if !consumers.is_empty() {
                declaration_matches.push((target, consumers));
            }
        }
    }

    if declaration_matches.len() != 1 {
        return None;
    }
    let (target, mut consumers) = declaration_matches.pop()?;
    let candidate_digests = consumers
        .iter()
        .map(|(_, _, correlation)| &correlation.target_digest)
        .collect::<std::collections::BTreeSet<_>>();
    if candidate_digests.len() != 1 {
        return None;
    }
    consumers.sort_by_key(|(consumer, start, _)| (*start, consumer.finish_seq, &consumer.call_id));
    let (consumer, consumer_start, _) = consumers.into_iter().next()?;
    Some(decision_evidence(
        producer,
        producer_finish,
        producer_tool,
        consumer.call_id.clone(),
        consumer_start,
        GraphEvidenceToolV1::GetCodeSnippet,
        GraphConsumptionModeV1::Source,
        target,
    ))
}

fn typed_source_consumer<'a>(
    producer: &GraphCall,
    producer_finish: u64,
    producer_lineage: &temper_protocol_activity::DecisionAnchorLineageV1,
    consumer: &'a GraphCall,
) -> Option<(u64, &'a GraphCorrelationV1)> {
    if consumer.scope_id != producer.scope_id
        || consumer.status != Some(ToolStatusV1::Succeeded)
        || consumer.name != GraphCorrelationToolV1::GetCodeSnippet.public_name()
    {
        return None;
    }
    let consumer_start = consumer
        .start_seq
        .filter(|start| *start > producer_finish)?;
    let correlation = consumer.graph_correlation.as_ref().filter(|correlation| {
        correlation.is_valid()
            && correlation.tool == GraphCorrelationToolV1::GetCodeSnippet
            && correlation.tool.public_name() == consumer.name
    })?;
    consumer
        .decision_anchor_lineage
        .as_ref()
        .filter(|lineage| {
            lineage.is_valid_for(correlation)
                && lineage.stage == DecisionAnchorLineageStageV1::CarryForward
                && lineage.root_binding == producer_lineage.root_binding
                && lineage.decision_evidence_kind == Some(DecisionEvidenceKindV1::FocusedTest)
        })?;
    Some((consumer_start, correlation))
}
