//! Privacy-safe extraction of typed graph output used by decision-anchor policy.

use super::*;

pub(super) fn graph_tool_for_name(name: &str) -> Option<GraphCorrelationToolV1> {
    match name {
        "codebase_memory_search_graph" => Some(GraphCorrelationToolV1::SearchGraph),
        "codebase_memory_search_code" => Some(GraphCorrelationToolV1::SearchCode),
        "codebase_memory_trace_path" => Some(GraphCorrelationToolV1::TracePath),
        "codebase_memory_get_code_snippet" => Some(GraphCorrelationToolV1::GetCodeSnippet),
        _ => None,
    }
}

pub(super) fn successful_graph_batch(finished: &[FinishedCodebaseCall<'_>]) -> bool {
    finished.iter().any(|finished| !finished.output.is_error)
}

fn valid_graph_correlation(name: &str, output: &ToolOutput) -> bool {
    if output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return false;
    }
    output
        .details
        .as_ref()
        .and_then(|details| details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY))
        .and_then(|value| serde_json::from_value::<GraphCorrelationV1>(value.clone()).ok())
        .is_some_and(|correlation| correlation.is_valid() && correlation.tool.public_name() == name)
}

pub(super) fn has_incompatible_targeted_result(
    finished: &[FinishedCodebaseCall<'_>],
    active: &AnchorForest,
) -> bool {
    finished.iter().any(|finished| {
        let output = anchor_output(finished.name, finished.output);
        match output {
            Some(output) if output.lineage.stage == DecisionAnchorLineageStageV1::CarryForward => {
                !active.accepts(&finished.call, &output.lineage)
            }
            Some(_) => false,
            None => valid_graph_correlation(finished.name, finished.output),
        }
    })
}

pub(super) fn trusted_unavailable_provider_output(name: &str, output: &ToolOutput) -> bool {
    if !output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return false;
    }
    let Some(marker) = output
        .details
        .as_ref()
        .and_then(|details| details.get(SAFE_TOOL_FAILURE_DETAIL_KEY))
    else {
        return false;
    };
    marker.get("source").and_then(serde_json::Value::as_str) == Some("codebase_memory")
        && marker
            .get("category")
            .and_then(serde_json::Value::as_str)
            .and_then(ToolFailureCategory::from_stable_str)
            .is_some()
}

pub(super) fn anchor_output(name: &str, output: &ToolOutput) -> Option<AnchorOutput> {
    if output.is_error || !name.starts_with(CODEBASE_MEMORY_TOOL_PREFIX) {
        return None;
    }
    let details = output.details.as_ref()?;
    let correlation: GraphCorrelationV1 =
        serde_json::from_value(details.get(SAFE_GRAPH_CORRELATION_DETAIL_KEY)?.clone()).ok()?;
    if !correlation.is_valid() || correlation.tool.public_name() != name {
        return None;
    }
    let lineage: DecisionAnchorLineageV1 = serde_json::from_value(
        details
            .get(SAFE_DECISION_ANCHOR_LINEAGE_DETAIL_KEY)?
            .clone(),
    )
    .ok()?;
    lineage.is_valid_for(&correlation).then_some(AnchorOutput {
        lineage,
        tool: correlation.tool,
    })
}
