// SPDX-License-Identifier: MPL-2.0

//! Privacy-safe ordinary-tool failure classification.

use std::collections::BTreeMap;

use temper_protocol_activity::{
    AgentActivityEventV1, AgentRunEventV1, ToolFailureCategoryV1, ToolFailureReasonV1, ToolStatusV1,
};

use super::CallKey;
use crate::{
    DiagnosticSeverityV1, MetricCoverageV1, OrdinaryToolMetricsV1, TraceDiagnosticCodeV1,
    TraceDiagnosticV1,
};

#[derive(Default)]
struct OrdinaryCall {
    name: String,
    status: Option<ToolStatusV1>,
    failure: Option<(ToolFailureCategoryV1, ToolFailureReasonV1)>,
    finish_seq: Option<u64>,
}

pub(super) fn ordinary_tool_metrics(
    events: &[AgentRunEventV1],
) -> (OrdinaryToolMetricsV1, Vec<TraceDiagnosticV1>) {
    let mut calls = BTreeMap::<CallKey, OrdinaryCall>::new();
    for event in events {
        match &event.event {
            AgentActivityEventV1::ToolStarted(tool) if !is_graph_wrapper(&tool.name) => {
                calls
                    .entry(CallKey::new(event, &tool.call_id))
                    .or_insert_with(|| OrdinaryCall {
                        name: tool.name.clone(),
                        ..OrdinaryCall::default()
                    });
            }
            AgentActivityEventV1::ToolFinished(tool) if !is_graph_wrapper(&tool.name) => {
                let call = calls.entry(CallKey::new(event, &tool.call_id)).or_default();
                call.name.clone_from(&tool.name);
                call.status = Some(tool.status);
                call.failure = tool
                    .failure
                    .as_ref()
                    .map(|failure| (failure.category, failure.reason));
                call.finish_seq = Some(event.seq);
            }
            _ => {}
        }
    }

    classify(calls)
}

fn classify(
    calls: BTreeMap<CallKey, OrdinaryCall>,
) -> (OrdinaryToolMetricsV1, Vec<TraceDiagnosticV1>) {
    let mut succeeded = 0_u64;
    let mut failed = 0_u64;
    let mut cancelled = 0_u64;
    let mut failures_by_category = BTreeMap::new();
    let mut failures_by_reason = BTreeMap::new();
    let mut category_observed = 0_u64;
    let mut reason_observed = 0_u64;
    let mut repeated_redirects = 0_u64;
    let mut diagnostics = Vec::new();

    for call in calls.values() {
        let Some(status) = call.status else {
            continue;
        };
        increment_status(status, &mut succeeded, &mut failed, &mut cancelled);
        if status == ToolStatusV1::Succeeded {
            continue;
        }
        let Some((category, reason)) = call.failure else {
            diagnostics.push(TraceDiagnosticV1 {
                code: TraceDiagnosticCodeV1::OrdinaryToolEvidenceUnavailable,
                severity: DiagnosticSeverityV1::Warning,
                message: "ordinary tool completion lacks a closed failure diagnostic; failure classification and repeated-redirect evidence are incomplete".to_string(),
                seq: call.finish_seq,
            });
            continue;
        };
        *failures_by_category.entry(category).or_default() += 1;
        *failures_by_reason.entry(reason).or_default() += 1;
        category_observed = category_observed.saturating_add(1);
        reason_observed = reason_observed.saturating_add(1);
        if category == ToolFailureCategoryV1::CircuitRedirect
            && matches!(
                reason,
                ToolFailureReasonV1::RepeatedNonRetryable
                    | ToolFailureReasonV1::RetryBudgetExhausted
            )
        {
            repeated_redirects = repeated_redirects.saturating_add(1);
        }
    }

    let call_count = calls.len() as u64;
    let status_observed = succeeded.saturating_add(failed).saturating_add(cancelled);
    let non_success = failed.saturating_add(cancelled);
    let complete_reasons = status_observed == call_count && reason_observed == non_success;
    (
        OrdinaryToolMetricsV1 {
            calls: call_count,
            succeeded,
            failed,
            cancelled,
            status_coverage: MetricCoverageV1 {
                observed: status_observed,
                expected: Some(call_count),
            },
            failures_by_category,
            failure_category_coverage: MetricCoverageV1 {
                observed: category_observed,
                expected: Some(non_success),
            },
            failures_by_reason,
            failure_reason_coverage: MetricCoverageV1 {
                observed: reason_observed,
                expected: Some(non_success),
            },
            repeated_failure_redirects: complete_reasons.then_some(repeated_redirects),
        },
        diagnostics,
    )
}

fn increment_status(
    status: ToolStatusV1,
    succeeded: &mut u64,
    failed: &mut u64,
    cancelled: &mut u64,
) {
    match status {
        ToolStatusV1::Succeeded => *succeeded = succeeded.saturating_add(1),
        ToolStatusV1::Failed => *failed = failed.saturating_add(1),
        ToolStatusV1::Cancelled => *cancelled = cancelled.saturating_add(1),
    }
}

fn is_graph_wrapper(name: &str) -> bool {
    name.starts_with("codebase_memory_")
}
