// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use serde_json::Value;
use temper_protocol_activity::{DecisionAnchorLineageStageV1, ToolStatusV1};

use super::super::CallKey;
use super::{Action, GraphCall, selection_matches_target, unavailable};
use crate::{
    AnalyzeOptions, GraphConsumptionModeV1, GraphDecisionEvidenceV1, GraphEvidenceToolV1,
    TraceDiagnosticV1,
};

pub(super) struct RelevanceAnalysis {
    pub(super) relevant: Option<u64>,
    pub(super) irrelevant: Option<u64>,
    pub(super) observed: u64,
    pub(super) evidence: Vec<GraphDecisionEvidenceV1>,
    pub(super) diagnostics: Vec<TraceDiagnosticV1>,
}

pub(super) fn classify_relevance(
    options: &AnalyzeOptions,
    calls: &BTreeMap<CallKey, GraphCall>,
    actions: &[Action],
) -> RelevanceAnalysis {
    let successful = calls
        .values()
        .filter(|call| call.status == Some(ToolStatusV1::Succeeded))
        .count() as u64;
    if options.graph_decision_targets.is_empty() {
        return RelevanceAnalysis {
            relevant: None,
            irrelevant: None,
            observed: 0,
            evidence: Vec::new(),
            diagnostics: vec![unavailable(
                None,
                "graph decision relevance is unavailable because no decision targets were declared",
            )],
        };
    }

    let mut observed = 0_u64;
    let mut relevant = 0_u64;
    let mut irrelevant = 0_u64;
    let mut evidence = Vec::new();
    let mut diagnostics = Vec::new();
    let mut lineage_relevant_calls = std::collections::BTreeSet::new();
    let has_typed_lineage = calls
        .values()
        .any(|call| call.decision_anchor_lineage.is_some());
    let mut successful_calls = calls
        .values()
        .filter(|call| call.status == Some(ToolStatusV1::Succeeded))
        .collect::<Vec<_>>();
    successful_calls.sort_by_key(|call| call.finish_seq);
    for call in successful_calls {
        let Some(graph_tool) =
            GraphEvidenceToolV1::from_tool_name(&call.name).filter(|tool| tool.is_targeted_graph())
        else {
            // Broad architecture/index and unknown graph calls cannot become
            // relevant simply because a later action happens to name a target.
            observed = observed.saturating_add(1);
            irrelevant = irrelevant.saturating_add(1);
            continue;
        };
        let Some(producer_correlation) = call
            .graph_correlation
            .as_ref()
            .filter(|value| value.is_valid() && value.tool.public_name() == call.name)
        else {
            diagnostics.push(unavailable(
                call.finish_seq,
                "successful targeted graph call lacks a complete trusted correlation record; relevance is unavailable",
            ));
            continue;
        };
        let Some(finish_seq) = call.finish_seq else {
            diagnostics.push(unavailable(
                None,
                "successful graph call lacks completion ordering needed to classify relevance",
            ));
            continue;
        };
        let lineage_evidence =
            forest_lineage_evidence(call, graph_tool, calls, &options.graph_decision_targets);
        lineage_relevant_calls.extend(
            lineage_evidence
                .iter()
                .map(|item| item.consumer_call_id.clone()),
        );
        let matching_targets = options
            .graph_decision_targets
            .iter()
            .filter(|target| {
                target
                    .producer
                    .correlation()
                    .as_ref()
                    .is_some_and(|expected| {
                        correlation_matches_expected(call, producer_correlation, expected)
                    })
            })
            .collect::<Vec<_>>();
        let mut call_evidence = lineage_evidence;
        let mut correlation_unknown = false;
        for target in matching_targets {
            let (mut target_evidence, target_unknown) = target_consumption(
                call,
                graph_tool,
                finish_seq,
                producer_correlation,
                target,
                calls,
                actions,
                has_typed_lineage,
            );
            call_evidence.append(&mut target_evidence);
            correlation_unknown |= target_unknown;
        }
        if !call_evidence.is_empty() || lineage_relevant_calls.contains(&call.call_id) {
            observed = observed.saturating_add(1);
            relevant = relevant.saturating_add(1);
            evidence.extend(call_evidence);
        } else if correlation_unknown {
            diagnostics.push(unavailable(
                Some(finish_seq),
                "a later declared consumer lacks a complete trusted correlation record needed to classify graph-result consumption",
            ));
        } else {
            // A complete typed producer which has no matching, ordered, same-scope
            // consumer is explicitly irrelevant rather than assumed useful.
            observed = observed.saturating_add(1);
            irrelevant = irrelevant.saturating_add(1);
        }
    }
    let complete = observed == successful;
    RelevanceAnalysis {
        relevant: complete.then_some(relevant),
        irrelevant: complete.then_some(irrelevant),
        observed,
        evidence,
        diagnostics,
    }
}

fn forest_lineage_evidence(
    call: &GraphCall,
    graph_tool: GraphEvidenceToolV1,
    calls: &BTreeMap<CallKey, GraphCall>,
    targets: &[crate::GraphDecisionTargetV1],
) -> Vec<GraphDecisionEvidenceV1> {
    let Some(producer) = call
        .decision_anchor_lineage
        .as_ref()
        .filter(|lineage| lineage.is_valid())
    else {
        return Vec::new();
    };
    let Some(finish_seq) = call.finish_seq else {
        return Vec::new();
    };

    let mut evidence = Vec::new();
    for target in targets {
        let mut consumers = calls
            .values()
            .filter(|consumer| {
                consumer.scope_id == call.scope_id
                    && consumer.status == Some(ToolStatusV1::Succeeded)
                    && consumer.start_seq.is_some_and(|start| start > finish_seq)
                    && consumer
                        .decision_anchor_lineage
                        .as_ref()
                        .is_some_and(|lineage| {
                            lineage.is_valid()
                                && lineage.stage == DecisionAnchorLineageStageV1::CarryForward
                                && lineage.root_binding == producer.root_binding
                                && lineage
                                    .canonical_target_digests
                                    .binary_search(
                                        &temper_protocol_activity::GraphCorrelationV1::target_digest(
                                            &target.target,
                                        )
                                        .expect("validated benchmark target has a digest"),
                                    )
                                    .is_ok()
                        })
            })
            .collect::<Vec<_>>();
        consumers.sort_by_key(|consumer| (consumer.start_seq, consumer.finish_seq));
        if let Some(consumer) = consumers.into_iter().next() {
            let consumer_tool = GraphEvidenceToolV1::from_tool_name(&consumer.name)
                .expect("typed lineage is present only on graph tools");
            evidence.push(decision_evidence(
                call,
                finish_seq,
                graph_tool,
                consumer.call_id.clone(),
                consumer
                    .start_seq
                    .expect("filtered consumer has start order"),
                consumer_tool,
                if consumer_tool == GraphEvidenceToolV1::GetCodeSnippet {
                    GraphConsumptionModeV1::Source
                } else {
                    GraphConsumptionModeV1::Graph
                },
                target,
            ));
            continue;
        }
    }
    evidence
}

fn target_consumption(
    call: &GraphCall,
    graph_tool: GraphEvidenceToolV1,
    finish_seq: u64,
    producer_correlation: &temper_protocol_activity::GraphCorrelationV1,
    target: &crate::GraphDecisionTargetV1,
    calls: &BTreeMap<CallKey, GraphCall>,
    actions: &[Action],
    has_typed_lineage: bool,
) -> (Vec<GraphDecisionEvidenceV1>, bool) {
    let mut evidence = Vec::new();
    let mut unknown = false;

    let mut matching_actions = actions
        .iter()
        .filter(|action| {
            action.scope_id == call.scope_id
                && action.start_seq > finish_seq
                && GraphEvidenceToolV1::from_tool_name(&action.name).is_some()
        })
        .filter_map(|action| {
            let tool = GraphEvidenceToolV1::from_tool_name(&action.name)?;
            let mode = match tool {
                GraphEvidenceToolV1::Read
                | GraphEvidenceToolV1::Edit
                | GraphEvidenceToolV1::Write => GraphConsumptionModeV1::Selection,
                GraphEvidenceToolV1::ApplyPatch => GraphConsumptionModeV1::Mutation,
                _ => return None,
            };
            let Some(arguments) = action.arguments.as_deref() else {
                unknown = true;
                return None;
            };
            let matches = match mode {
                GraphConsumptionModeV1::Selection => {
                    selection_matches_target(arguments, &target.target)
                }
                GraphConsumptionModeV1::Mutation => {
                    mutation_matches_target(arguments, &target.target)
                }
                GraphConsumptionModeV1::Graph | GraphConsumptionModeV1::Source => false,
            };
            matches.then_some((action, tool, mode))
        })
        .collect::<Vec<_>>();
    matching_actions.sort_by_key(|(action, _, _)| (action.start_seq, action.finish_seq));
    if let Some((action, tool, mode)) = matching_actions.into_iter().next() {
        evidence.push(decision_evidence(
            call,
            finish_seq,
            graph_tool,
            action.call_id.clone(),
            action.start_seq,
            tool,
            mode,
            target,
        ));
    }

    for consumption in &target.consumption {
        let Some(expected_correlation) = consumption.correlation() else {
            unknown = true;
            continue;
        };
        let mut matching_consumers = Vec::new();
        for consumer in calls.values().filter(|consumer| {
            consumer.scope_id == call.scope_id
                && consumer.status == Some(ToolStatusV1::Succeeded)
                && consumer.name == consumption.tool.public_name()
        }) {
            let Some(start_seq) = consumer.start_seq else {
                unknown = true;
                continue;
            };
            if start_seq <= finish_seq {
                continue;
            }
            let Some(correlation) = consumer
                .graph_correlation
                .as_ref()
                .filter(|value| value.is_valid() && value.tool.public_name() == consumer.name)
            else {
                unknown = true;
                continue;
            };
            let matches_expected =
                correlation_matches_expected(consumer, correlation, &expected_correlation);
            if matches_expected
                && lineage_consumes(
                    call,
                    producer_correlation,
                    consumer,
                    correlation,
                    has_typed_lineage,
                )
            {
                matching_consumers.push(consumer);
            } else if has_typed_lineage && matches_expected {
                unknown = true;
            }
        }
        matching_consumers.sort_by_key(|consumer| (consumer.start_seq, consumer.finish_seq));
        if let Some(consumer) = matching_consumers.into_iter().next() {
            let consumer_tool = GraphEvidenceToolV1::from_tool_name(&consumer.name)
                .expect("closed correlation tool has an evidence-tool name");
            let mode = match consumer_tool {
                GraphEvidenceToolV1::GetCodeSnippet => GraphConsumptionModeV1::Source,
                GraphEvidenceToolV1::SearchGraph
                | GraphEvidenceToolV1::SearchCode
                | GraphEvidenceToolV1::TracePath => GraphConsumptionModeV1::Graph,
                _ => unreachable!("closed correlation tools are targeted graph tools"),
            };
            evidence.push(decision_evidence(
                call,
                finish_seq,
                graph_tool,
                consumer.call_id.clone(),
                consumer
                    .start_seq
                    .expect("filtered graph consumer has a start sequence"),
                consumer_tool,
                mode,
                target,
            ));
        }
    }
    (evidence, unknown)
}

fn lineage_consumes(
    producer: &GraphCall,
    producer_correlation: &temper_protocol_activity::GraphCorrelationV1,
    consumer: &GraphCall,
    consumer_correlation: &temper_protocol_activity::GraphCorrelationV1,
    has_typed_lineage: bool,
) -> bool {
    match (
        producer.decision_anchor_lineage.as_ref(),
        consumer.decision_anchor_lineage.as_ref(),
    ) {
        (Some(producer), Some(consumer)) => {
            producer.is_valid_for(producer_correlation)
                && consumer.is_valid_for(consumer_correlation)
                && consumer.stage == DecisionAnchorLineageStageV1::CarryForward
                && consumer.root_binding == producer.root_binding
        }
        // Legacy durable traces never carried a lineage record. Once a run
        // has any typed record, every graph-to-graph/source edge must carry it.
        (None, None) if !has_typed_lineage => true,
        _ => false,
    }
}

fn correlation_matches_expected(
    call: &GraphCall,
    actual: &temper_protocol_activity::GraphCorrelationV1,
    expected: &temper_protocol_activity::GraphCorrelationV1,
) -> bool {
    actual.tool == expected.tool
        && actual.target_kind == expected.target_kind
        && (actual.target_digest == expected.target_digest
            || call
                .decision_anchor_lineage
                .as_ref()
                .filter(|lineage| lineage.is_valid_for(actual))
                .is_some_and(|lineage| {
                    lineage
                        .canonical_target_digests
                        .binary_search(&expected.target_digest)
                        .is_ok()
                }))
}

fn decision_evidence(
    call: &GraphCall,
    finish_seq: u64,
    graph_tool: GraphEvidenceToolV1,
    consumer_call_id: String,
    consumer_start_seq: u64,
    consumer_tool: GraphEvidenceToolV1,
    consumption_mode: GraphConsumptionModeV1,
    target: &crate::GraphDecisionTargetV1,
) -> GraphDecisionEvidenceV1 {
    GraphDecisionEvidenceV1 {
        graph_call_id: call.call_id.clone(),
        graph_finish_seq: finish_seq,
        graph_tool,
        consumer_call_id,
        consumer_start_seq,
        consumer_tool,
        consumption_mode,
        target: target.target.clone(),
        kind: target.kind,
    }
}

fn mutation_matches_target(arguments: &str, target: &str) -> bool {
    let patch = serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("patch")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| value.as_str().map(ToOwned::to_owned))
        })
        .unwrap_or_else(|| arguments.to_string());
    let expected = format!("diff --git a/{target} b/{target}");
    patch.lines().any(|line| line == expected)
}
