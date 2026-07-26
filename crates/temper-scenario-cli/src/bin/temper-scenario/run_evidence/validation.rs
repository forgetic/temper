// SPDX-License-Identifier: MPL-2.0

use crate::run_context::ScenarioTier;

use super::model::{
    LEGACY_RUN_EVIDENCE_VERSION, RUN_EVIDENCE_SCHEMA, RUN_EVIDENCE_VERSION, RunEvidenceArtifact,
    RunEvidenceVerdict,
};

impl RunEvidenceArtifact {
    pub(crate) fn validate(&self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if self.schema != RUN_EVIDENCE_SCHEMA {
            diagnostics.push(format!(
                "run evidence schema mismatch: expected `{RUN_EVIDENCE_SCHEMA}`, got `{}`",
                self.schema
            ));
        }
        if !matches!(
            self.version,
            LEGACY_RUN_EVIDENCE_VERSION | RUN_EVIDENCE_VERSION
        ) {
            diagnostics.push(format!(
                "run evidence version mismatch: supported versions are {LEGACY_RUN_EVIDENCE_VERSION} and {RUN_EVIDENCE_VERSION}, got {}",
                self.version
            ));
        }
        if self.scenario.name.trim().is_empty() {
            diagnostics.push("run evidence scenario.name is missing".to_string());
        }
        if self.scenario.manifest_path.trim().is_empty() {
            diagnostics.push("run evidence scenario.manifest_path is missing".to_string());
        }
        if !matches!(self.scenario.source.as_str(), "checked_in" | "ephemeral") {
            diagnostics.push(format!(
                "run evidence scenario.source must be `checked_in` or `ephemeral`, got `{}`",
                self.scenario.source
            ));
        }
        if ScenarioTier::parse(&self.scenario.tier).is_none() {
            diagnostics.push(format!(
                "run evidence scenario.tier must be `hermetic` or `live`, got `{}`",
                self.scenario.tier
            ));
        }
        if self.scenario.runner_id.trim().is_empty() {
            diagnostics.push("run evidence scenario.runner_id is missing".to_string());
        }
        if self.version >= RUN_EVIDENCE_VERSION {
            for (field, value) in [
                (
                    "scenario.mapped_scenario",
                    self.scenario.mapped_scenario.as_deref(),
                ),
                (
                    "scenario.resolved_content_digest",
                    self.scenario.resolved_content_digest.as_deref(),
                ),
            ] {
                if value.is_none_or(str::is_empty) {
                    diagnostics.push(format!("run evidence {field} is missing for version 2"));
                }
            }
            if self.scenario.feature.is_some()
                && self
                    .scenario
                    .mapping_id
                    .as_deref()
                    .is_none_or(str::is_empty)
            {
                diagnostics
                    .push("feature-mapped run evidence is missing scenario.mapping_id".to_string());
            }
            if self.verdict == RunEvidenceVerdict::Passed && self.scenario.source == "checked_in" {
                for (field, value) in [
                    (
                        "scenario.source_branch",
                        self.scenario.source_branch.as_deref(),
                    ),
                    (
                        "scenario.checkout_head_sha",
                        self.scenario.checkout_head_sha.as_deref(),
                    ),
                ] {
                    if value.is_none_or(str::is_empty) {
                        diagnostics.push(format!(
                            "passing checked-in run evidence is missing {field}"
                        ));
                    }
                }
            }
            if self.verdict == RunEvidenceVerdict::Passed && self.execution.is_none() {
                diagnostics.push(
                    "passing run evidence is missing execution duration/status facts".to_string(),
                );
            }
            if self.verdict == RunEvidenceVerdict::Passed
                && self.scenario.tier == "live"
                && self.binary.is_none()
            {
                diagnostics.push(
                    "passing live run evidence is missing standalone binary identity".to_string(),
                );
            }
            if self.verdict == RunEvidenceVerdict::Passed && self.scenario.tier == "live" {
                for (field, value) in [
                    ("topology.forge", self.scenario.topology.forge.as_deref()),
                    ("topology.runner", self.scenario.topology.runner.as_deref()),
                    ("topology.temper", self.scenario.topology.temper.as_deref()),
                    (
                        "topology.agent_model",
                        self.scenario.topology.agent_model.as_deref(),
                    ),
                ] {
                    if value.is_none_or(str::is_empty) {
                        diagnostics.push(format!("passing live run evidence is missing `{field}`"));
                    }
                }
                match self.provider.as_ref() {
                    Some(provider)
                        if !provider.jig_script_paths.is_empty()
                            && provider
                                .request_log_path
                                .as_deref()
                                .is_some_and(|p| !p.is_empty())
                            && provider.request_count.is_some()
                            && !provider.request_counts_by_role.is_empty() =>
                    {
                        let total = provider.request_counts_by_role.values().sum::<usize>();
                        if provider.request_count != Some(total) {
                            diagnostics.push(
                                "run evidence Jig request total does not match per-role counts"
                                    .to_string(),
                            );
                        }
                    }
                    _ => diagnostics.push(
                        "passing live run evidence is missing Jig script/request facts".to_string(),
                    ),
                }
                match self.observability.as_ref() {
                    Some(observability)
                        if !observability.event_log_paths.is_empty()
                            && !observability.events.is_empty() => {}
                    _ => diagnostics.push(
                        "passing live run evidence is missing structured Temper event facts"
                            .to_string(),
                    ),
                }
                if self.artifacts.log_paths.is_empty() || self.artifacts.artifact_paths.is_empty() {
                    diagnostics.push(
                        "passing live run evidence is missing retained log/artifact paths"
                            .to_string(),
                    );
                }
            }
        }
        if self.verdict == RunEvidenceVerdict::Passed
            && self.final_state.issues.is_empty()
            && self.final_state.pull_requests.is_empty()
            && self.final_state.repositories.is_empty()
            && self.final_state.ci.completed_jobs.is_none()
            && self.final_state.ci.jobs.is_empty()
        {
            diagnostics.push(
                "run evidence final_state has no issue, pull request, repository, or CI data"
                    .to_string(),
            );
        }
        if let Some(binary) = &self.binary {
            if binary.path.trim().is_empty()
                || binary.sha256.len() != 64
                || !binary.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
                || binary.size_bytes == 0
            {
                diagnostics
                    .push("run evidence standalone binary identity is incomplete".to_string());
            }
        }
        if let Some(execution) = &self.execution {
            if execution.status.trim().is_empty()
                || (self.verdict == RunEvidenceVerdict::Passed
                    && !matches!(execution.status.as_str(), "completed" | "passed"))
            {
                diagnostics.push(format!(
                    "run evidence execution status `{}` is inconsistent with verdict `{}`",
                    execution.status,
                    self.verdict.as_str()
                ));
            }
        }
        for stimulus in &self.stimuli {
            if stimulus.id.trim().is_empty()
                || stimulus.action.trim().is_empty()
                || stimulus.attempts == 0
                || stimulus.timeout_ms == 0
            {
                diagnostics.push("run evidence contains an incomplete stimulus result".to_string());
            }
            if !matches!(stimulus.status.as_str(), "passed" | "failed" | "timed_out") {
                diagnostics.push(format!(
                    "run evidence stimulus `{}` has unknown status `{}`",
                    stimulus.id, stimulus.status
                ));
            }
            if self.verdict == RunEvidenceVerdict::Passed && stimulus.status != "passed" {
                diagnostics.push(format!(
                    "passing run evidence contains non-passing stimulus `{}` ({})",
                    stimulus.id, stimulus.status
                ));
            }
        }
        if let Some(assertions) = &self.assertions {
            if assertions.total != assertions.results.len() {
                diagnostics.push(format!(
                    "run evidence assertions.total is {}, but {} result(s) are present",
                    assertions.total,
                    assertions.results.len()
                ));
            }
            let mut assertion_ids = std::collections::BTreeSet::new();
            for result in &assertions.results {
                if result.id.trim().is_empty() {
                    diagnostics
                        .push("run evidence contains an assertion without an id".to_string());
                } else if !assertion_ids.insert(result.id.as_str()) {
                    diagnostics.push(format!(
                        "run evidence contains duplicate assertion id `{}`",
                        result.id
                    ));
                }
            }
            for status in [&assertions.status]
                .into_iter()
                .chain(assertions.results.iter().map(|result| &result.status))
            {
                if !matches!(
                    status.as_str(),
                    super::model::ASSERTION_STATUS_PASSED
                        | super::model::ASSERTION_STATUS_FAILED
                        | super::model::ASSERTION_STATUS_INCONCLUSIVE
                        | super::model::ASSERTION_STATUS_MISSING_FACT
                        | super::model::ASSERTION_STATUS_TIMED_OUT
                        | super::model::ASSERTION_STATUS_UNSUPPORTED
                ) {
                    diagnostics.push(format!(
                        "run evidence assertion status must be `passed`, `failed`, `inconclusive`, `missing_fact`, `timed_out`, or `unsupported`, got `{status}`"
                    ));
                }
            }
            if self.version >= RUN_EVIDENCE_VERSION {
                let recomputed =
                    super::model::AssertionEvidence::from_results(assertions.results.clone());
                if &recomputed != assertions {
                    diagnostics.push(
                        "run evidence assertion summary counters/status do not match results"
                            .to_string(),
                    );
                }
                if self.verdict == RunEvidenceVerdict::Passed && assertions.required == 0 {
                    diagnostics
                        .push("passing run evidence contains no required assertions".to_string());
                }
                if self.verdict == RunEvidenceVerdict::Passed && assertions.has_failures() {
                    diagnostics.push(
                        "passing run evidence contains a blocking required assertion".to_string(),
                    );
                }
            }
        } else if self.version >= RUN_EVIDENCE_VERSION && self.verdict == RunEvidenceVerdict::Passed
        {
            diagnostics
                .push("passing run evidence is missing required assertion results".to_string());
        }
        diagnostics
    }
}
