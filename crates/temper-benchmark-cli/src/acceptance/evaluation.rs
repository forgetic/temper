// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use super::input::TrialSet;
use super::{
    AcceptanceGateResultV1, AcceptanceGateV1, AcceptanceImprovementMeasureV1,
    AcceptanceObservationV1, BenchmarkAcceptancePolicyV1,
};
use crate::{
    BenchmarkConditionV1, GraphConsumptionModeV1, GraphEvidenceToolV1, ResolvedBenchmarkManifest,
    RunSummaryV1, RunTerminalStatusV1, ValidationArtifactV1,
};

pub(super) fn identity_matches(
    set: &TrialSet,
    manifest: &ResolvedBenchmarkManifest,
    policy: &BenchmarkAcceptancePolicyV1,
    candidate_commit: &str,
    condition: BenchmarkConditionV1,
    repetitions: u32,
) -> bool {
    if set.aggregate.benchmark.as_deref() != Some(manifest.manifest().name.as_str())
        || set.aggregate.mode != Some(policy.mode)
        || set.aggregate.condition != Some(condition)
        || set.aggregate.runs.len() != repetitions as usize
        || set.aggregate.outcomes.total != u64::from(repetitions)
    {
        return false;
    }
    let expected_repetitions = (1..=repetitions).collect::<BTreeSet<_>>();
    let actual_repetitions = set
        .aggregate
        .runs
        .iter()
        .map(|run| run.repetition)
        .collect::<BTreeSet<_>>();
    if actual_repetitions != expected_repetitions {
        return false;
    }
    set.aggregate.runs.iter().all(|run| {
        let summary = &run.summary;
        let benchmark = summary.benchmark.as_ref();
        let host = summary.host.as_ref();
        benchmark.is_some_and(|identity| {
            identity.name == manifest.manifest().name
                && identity.mode == policy.mode
                && identity.condition == Some(condition)
                && identity.repetition == run.repetition
        }) && summary.capture == Some(manifest.manifest().capture)
            && host.is_some_and(|host| {
                host.temper
                    .commit
                    .as_deref()
                    .is_some_and(|commit| commit.eq_ignore_ascii_case(candidate_commit))
                    && host.observed_models.len() == 1
                    && host.observed_models[0].provider == policy.provider
                    && host.observed_models[0].model == policy.model
                    && host.cache_warmth == manifest.manifest().annotations.cache_warmth
                    && host.provider_region == manifest.manifest().annotations.provider_region
            })
    })
}

pub(super) fn unique_trials(sets: &[&TrialSet]) -> bool {
    let mut runs = BTreeSet::new();
    let mut sessions = BTreeSet::new();
    sets.iter().flat_map(|set| set.summaries()).all(|summary| {
        !summary.identity.run_id.trim().is_empty()
            && runs.insert(summary.identity.run_id.clone())
            && summary
                .identity
                .agent_session_id
                .as_ref()
                .is_some_and(|session| {
                    !session.trim().is_empty() && sessions.insert(session.clone())
                })
    })
}

pub(super) fn task_correct(summary: &RunSummaryV1) -> bool {
    summary.terminal.as_ref().map(|terminal| terminal.status)
        == Some(RunTerminalStatusV1::Succeeded)
        && summary.workspace_result.is_some()
}

pub(super) fn host_validation_passed(
    summary: &RunSummaryV1,
    validation: &ValidationArtifactV1,
) -> bool {
    if validation.post_run_commands.is_empty()
        || !validation.post_run_commands.iter().all(|command| {
            command.status == "passed" && command.exit_code == Some(0) && !command.timed_out
        })
    {
        return false;
    }
    if validation.accepted_submit.as_ref().is_some_and(|submit| {
        !submit.response.accepted
            || !submit.fingerprint_current_after_session
            || submit.response.gates.iter().any(|gate| {
                gate.timed_out
                    || gate.exit_code != Some(0)
                    || !gate.exit_status.eq_ignore_ascii_case("passed")
            })
    }) {
        return false;
    }
    validation_summary_matches(summary, validation)
}

pub(super) fn exact_patch_passed(validation: &ValidationArtifactV1) -> bool {
    validation
        .exact_patch
        .as_ref()
        .is_some_and(|patch| patch.status == "passed" && patch.untracked_files == 0)
}

pub(super) fn validation_summary_matches(
    summary: &RunSummaryV1,
    validation: &ValidationArtifactV1,
) -> bool {
    let gates = validation
        .accepted_submit
        .as_ref()
        .map_or(&[][..], |submit| submit.response.gates.as_slice());
    let gate_succeeded = gates
        .iter()
        .filter(|gate| {
            !gate.timed_out
                && gate.exit_code == Some(0)
                && gate.exit_status.eq_ignore_ascii_case("passed")
        })
        .count() as u64;
    let command_succeeded = validation
        .post_run_commands
        .iter()
        .filter(|command| command.status == "passed")
        .count() as u64;
    let exact_count = u64::from(validation.exact_patch.is_some());
    let exact_succeeded = u64::from(
        validation
            .exact_patch
            .as_ref()
            .is_some_and(|patch| patch.status == "passed"),
    );
    let count = gates.len() as u64 + validation.post_run_commands.len() as u64 + exact_count;
    let succeeded = gate_succeeded + command_succeeded + exact_succeeded;
    summary.validation.as_ref().is_some_and(|summary| {
        summary.command_count == count
            && summary.succeeded == succeeded
            && summary.failed == count.saturating_sub(succeeded)
    })
}

pub(super) fn decision_evidence_complete(
    summary: &RunSummaryV1,
    policy: &BenchmarkAcceptancePolicyV1,
) -> bool {
    let Some(graph) = &summary.metrics.graph else {
        return false;
    };
    let status_total = graph
        .succeeded
        .checked_add(graph.failed)
        .and_then(|total| total.checked_add(graph.cancelled));
    let relevance_total = graph
        .relevant_results
        .zip(graph.irrelevant_successes)
        .and_then(|(relevant, irrelevant)| relevant.checked_add(irrelevant));
    if status_total != Some(graph.calls)
        || graph.status_coverage.observed != graph.status_coverage.expected.unwrap_or(u64::MAX)
        || graph.relevance_coverage.observed
            != graph.relevance_coverage.expected.unwrap_or(u64::MAX)
        || graph.relevance_coverage.expected != Some(graph.succeeded)
        || graph
            .typed_correlation_coverage
            .as_ref()
            .is_none_or(|coverage| {
                coverage.expected != Some(graph.succeeded) || coverage.observed != graph.succeeded
            })
        || graph
            .typed_lineage_coverage
            .as_ref()
            .is_none_or(|coverage| {
                coverage.expected != Some(graph.succeeded) || coverage.observed != graph.succeeded
            })
        || graph.relevant_results.is_none()
        || graph.irrelevant_successes.is_none()
        || relevance_total != Some(graph.succeeded)
        || graph.succeeded == 0
    {
        return false;
    }
    let evidence_valid = graph.decision_evidence.iter().all(|evidence| {
        !evidence.graph_call_id.trim().is_empty()
            && !evidence.consumer_call_id.trim().is_empty()
            && evidence.graph_finish_seq < evidence.consumer_start_seq
            && matches!(
                evidence.graph_tool,
                GraphEvidenceToolV1::SearchGraph
                    | GraphEvidenceToolV1::SearchCode
                    | GraphEvidenceToolV1::TracePath
                    | GraphEvidenceToolV1::GetCodeSnippet
            )
            && match evidence.consumption_mode {
                GraphConsumptionModeV1::Source => {
                    evidence.consumer_tool == GraphEvidenceToolV1::GetCodeSnippet
                }
                GraphConsumptionModeV1::Selection => matches!(
                    evidence.consumer_tool,
                    GraphEvidenceToolV1::Read
                        | GraphEvidenceToolV1::Edit
                        | GraphEvidenceToolV1::Write
                ),
                GraphConsumptionModeV1::Mutation => {
                    evidence.consumer_tool == GraphEvidenceToolV1::ApplyPatch
                }
                GraphConsumptionModeV1::Graph => matches!(
                    evidence.consumer_tool,
                    GraphEvidenceToolV1::SearchGraph
                        | GraphEvidenceToolV1::SearchCode
                        | GraphEvidenceToolV1::TracePath
                ),
            }
    });
    evidence_valid
        && policy.required_decision_kinds.iter().all(|kind| {
            graph
                .decision_evidence
                .iter()
                .any(|evidence| evidence.kind == *kind)
        })
        && policy.required_consumption_modes.iter().all(|mode| {
            graph
                .decision_evidence
                .iter()
                .any(|evidence| evidence.consumption_mode == *mode)
        })
        && graph.decision_evidence.iter().any(|evidence| {
            evidence.target == policy.exact_source_selection_target
                && evidence.consumption_mode == GraphConsumptionModeV1::Selection
                && evidence.consumer_tool == GraphEvidenceToolV1::Read
        })
}

pub(super) fn summed_relevance(set: &TrialSet) -> Option<(u64, u64)> {
    set.summaries().try_fold((0_u64, 0_u64), |totals, summary| {
        let graph = summary.metrics.graph.as_ref()?;
        if graph.relevance_coverage.expected != Some(graph.succeeded)
            || graph.relevance_coverage.observed != graph.succeeded
        {
            return None;
        }
        Some((
            totals.0.checked_add(graph.relevant_results?)?,
            totals.1.checked_add(graph.succeeded)?,
        ))
    })
}

pub(super) fn relevance_passes(
    relevance: Option<(u64, u64)>,
    policy: &BenchmarkAcceptancePolicyV1,
) -> bool {
    relevance.is_some_and(|(relevant, succeeded)| {
        succeeded > 0
            && u128::from(relevant) * 100
                >= u128::from(succeeded) * u128::from(policy.minimum_relevance_percent)
    })
}

pub(super) fn unavailable_retry_passed(summary: &RunSummaryV1) -> bool {
    summary.metrics.graph.as_ref().is_some_and(|graph| {
        let unavailable = graph.failed.checked_add(graph.cancelled);
        graph.calls > 0
            && unavailable.is_some_and(|unavailable| unavailable > 0)
            && graph.status_coverage.expected == Some(graph.calls)
            && graph.status_coverage.observed == graph.calls
            && graph.immediate_repeat_coverage.expected == unavailable
            && Some(graph.immediate_repeat_coverage.observed) == unavailable
            && graph.immediate_repeated_attempts_after_systemic_failure == Some(0)
    })
}

pub(super) fn classification_complete(summary: &RunSummaryV1) -> bool {
    summary
        .metrics
        .graph
        .as_ref()
        .and_then(|graph| graph.conventional_discovery_before_selection.as_ref())
        .is_some_and(|discovery| {
            discovery.total_calls.is_some()
                && discovery.shell_command_classification_coverage.expected
                    == Some(discovery.shell_command_classification_coverage.observed)
        })
}

pub(super) fn improvement_observation(
    enabled: &TrialSet,
    disabled: &TrialSet,
    policy: &BenchmarkAcceptancePolicyV1,
) -> Option<AcceptanceObservationV1> {
    let enabled = enabled
        .summaries()
        .map(|summary| improvement_value(summary, policy.improvement_measure))
        .collect::<Option<Vec<_>>>()?;
    let disabled = disabled
        .summaries()
        .map(|summary| improvement_value(summary, policy.improvement_measure))
        .collect::<Option<Vec<_>>>()?;
    if enabled.len() != policy.matrix_repetitions as usize
        || disabled.len() != policy.matrix_repetitions as usize
    {
        return None;
    }
    Some(AcceptanceObservationV1 {
        numerator: None,
        denominator: None,
        required_percent: Some(policy.minimum_improvement_percent),
        enabled_median: nearest_rank_median(enabled),
        disabled_median: nearest_rank_median(disabled),
    })
}

fn improvement_value(
    summary: &RunSummaryV1,
    measure: AcceptanceImprovementMeasureV1,
) -> Option<u64> {
    match measure {
        AcceptanceImprovementMeasureV1::ConventionalDiscoveryCalls => {
            summary
                .metrics
                .graph
                .as_ref()?
                .conventional_discovery_before_selection
                .as_ref()?
                .total_calls
        }
        AcceptanceImprovementMeasureV1::WallTimeMs => summary.wall_time_ms,
        AcceptanceImprovementMeasureV1::InputTokens => {
            let tokens = summary.metrics.tokens.as_ref()?;
            (tokens.coverage.expected == Some(tokens.coverage.observed))
                .then_some(tokens.input_tokens)
        }
        AcceptanceImprovementMeasureV1::OutputTokens => {
            let tokens = summary.metrics.tokens.as_ref()?;
            (tokens.coverage.expected == Some(tokens.coverage.observed))
                .then_some(tokens.output_tokens)
        }
    }
}

fn nearest_rank_median(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len().div_ceil(2) - 1])
}

pub(super) fn gate(gate: AcceptanceGateV1, passed: bool) -> AcceptanceGateResultV1 {
    AcceptanceGateResultV1 {
        gate,
        passed,
        observation: None,
    }
}

pub(super) fn relevance_gate(
    gate: AcceptanceGateV1,
    passed: bool,
    relevance: Option<(u64, u64)>,
    required_percent: u8,
) -> AcceptanceGateResultV1 {
    AcceptanceGateResultV1 {
        gate,
        passed,
        observation: relevance.map(|(numerator, denominator)| AcceptanceObservationV1 {
            numerator: Some(numerator),
            denominator: Some(denominator),
            required_percent: Some(required_percent),
            enabled_median: None,
            disabled_median: None,
        }),
    }
}
