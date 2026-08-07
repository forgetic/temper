// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use serde_json::Value;
use temper_protocol_activity::ToolStatusV1;

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
        let Some(result) = call.result.as_deref() else {
            diagnostics.push(unavailable(
                call.finish_seq,
                "successful graph result content is omitted or truncated; relevance is unavailable",
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
        let matching_targets = options
            .graph_decision_targets
            .iter()
            .filter(|target| {
                result.contains(
                    target
                        .result_contains
                        .as_deref()
                        .unwrap_or(target.target.as_str()),
                )
            })
            .collect::<Vec<_>>();
        let mut call_evidence = Vec::new();
        let mut correlation_unknown = false;
        for target in matching_targets {
            let (mut target_evidence, target_unknown) =
                target_consumption(call, graph_tool, finish_seq, target, calls, actions);
            call_evidence.append(&mut target_evidence);
            correlation_unknown |= target_unknown;
        }
        if !call_evidence.is_empty() {
            observed = observed.saturating_add(1);
            relevant = relevant.saturating_add(1);
            evidence.extend(call_evidence);
        } else if correlation_unknown {
            diagnostics.push(unavailable(
                Some(finish_seq),
                "a later declared consumer omits arguments needed to classify graph-result consumption",
            ));
        } else {
            // A complete result which has no matching, ordered, same-scope
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

fn target_consumption(
    call: &GraphCall,
    graph_tool: GraphEvidenceToolV1,
    finish_seq: u64,
    target: &crate::GraphDecisionTargetV1,
    calls: &BTreeMap<CallKey, GraphCall>,
    actions: &[Action],
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
        let mut matching_consumers = Vec::new();
        for consumer in calls.values().filter(|consumer| {
            consumer.scope_id == call.scope_id
                && consumer.status == Some(ToolStatusV1::Succeeded)
                && GraphEvidenceToolV1::from_tool_name(&consumer.name) == Some(consumption.tool)
        }) {
            let Some(start_seq) = consumer.start_seq else {
                unknown = true;
                continue;
            };
            if start_seq <= finish_seq {
                continue;
            }
            let Some(arguments) = consumer.arguments.as_deref() else {
                unknown = true;
                continue;
            };
            if graph_arguments_match(consumption.tool, arguments, &consumption.target) {
                matching_consumers.push(consumer);
            }
        }
        matching_consumers.sort_by_key(|consumer| (consumer.start_seq, consumer.finish_seq));
        if let Some(consumer) = matching_consumers.into_iter().next() {
            let mode = match consumption.tool {
                GraphEvidenceToolV1::GetCodeSnippet => GraphConsumptionModeV1::Source,
                GraphEvidenceToolV1::SearchGraph
                | GraphEvidenceToolV1::SearchCode
                | GraphEvidenceToolV1::TracePath => GraphConsumptionModeV1::Graph,
                _ => unreachable!("manifest validation permits only targeted graph consumers"),
            };
            evidence.push(decision_evidence(
                call,
                finish_seq,
                graph_tool,
                consumer.call_id.clone(),
                consumer
                    .start_seq
                    .expect("filtered graph consumer has a start sequence"),
                consumption.tool,
                mode,
                target,
            ));
        }
    }
    (evidence, unknown)
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

fn graph_arguments_match(tool: GraphEvidenceToolV1, arguments: &str, target: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return arguments.trim() == target.trim();
    };
    if value.as_str().is_some_and(|value| value == target) {
        return true;
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let fields: &[&str] = match tool {
        GraphEvidenceToolV1::SearchGraph => &["query", "name_pattern", "qn_pattern"],
        GraphEvidenceToolV1::SearchCode => &["pattern"],
        GraphEvidenceToolV1::TracePath => &["function_name"],
        GraphEvidenceToolV1::GetCodeSnippet => &["qualified_name"],
        _ => return false,
    };
    fields.iter().any(|field| {
        object
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| value == target)
    })
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
