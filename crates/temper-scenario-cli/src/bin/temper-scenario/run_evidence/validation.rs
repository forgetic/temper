// SPDX-License-Identifier: MPL-2.0

use crate::run_context::ScenarioTier;

use super::model::{RUN_EVIDENCE_SCHEMA, RUN_EVIDENCE_VERSION, RunEvidenceArtifact};

impl RunEvidenceArtifact {
    pub(crate) fn validate(&self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if self.schema != RUN_EVIDENCE_SCHEMA {
            diagnostics.push(format!(
                "run evidence schema mismatch: expected `{RUN_EVIDENCE_SCHEMA}`, got `{}`",
                self.schema
            ));
        }
        if self.version != RUN_EVIDENCE_VERSION {
            diagnostics.push(format!(
                "run evidence version mismatch: expected {RUN_EVIDENCE_VERSION}, got {}",
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
        if self.final_state.issues.is_empty()
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
        if let Some(assertions) = &self.assertions {
            if assertions.total != assertions.results.len() {
                diagnostics.push(format!(
                    "run evidence assertions.total is {}, but {} result(s) are present",
                    assertions.total,
                    assertions.results.len()
                ));
            }
            for status in [&assertions.status]
                .into_iter()
                .chain(assertions.results.iter().map(|result| &result.status))
            {
                if !matches!(
                    status.as_str(),
                    super::model::ASSERTION_STATUS_PASSED
                        | super::model::ASSERTION_STATUS_FAILED
                        | super::model::ASSERTION_STATUS_UNSUPPORTED
                ) {
                    diagnostics.push(format!(
                        "run evidence assertion status must be `passed`, `failed`, or `unsupported`, got `{status}`"
                    ));
                }
            }
        }
        diagnostics
    }
}
