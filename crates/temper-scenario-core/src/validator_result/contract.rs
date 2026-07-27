// SPDX-License-Identifier: MPL-2.0

//! Strict exact-head evidence contract.

use super::*;

impl ValidatorResult {
    /// Validate the strict exact-head evidence contract independently of a
    /// workflow gate. Legacy v1 payloads remain readable but cannot authorize a
    /// passing exact-head validation.
    pub fn validate_contract(&self) -> Vec<String> {
        let mut diagnostics = Vec::new();
        if !matches!(
            self.schema.as_str(),
            VALIDATOR_RESULT_SCHEMA | LEGACY_VALIDATOR_RESULT_SCHEMA
        ) {
            diagnostics.push(format!(
                "unsupported validator result schema `{}`",
                self.schema
            ));
        }
        if self.verdict != ValidationVerdict::Passed {
            return diagnostics;
        }
        if self.schema != VALIDATOR_RESULT_SCHEMA {
            diagnostics
                .push("passing exact-head validation requires validator result v2".to_string());
        }
        for (field, value) in [
            ("feature", self.feature.as_deref()),
            ("plan", self.plan.as_deref()),
            ("mapping_id", self.mapping_id.as_deref()),
            ("scenario_name", self.scenario_name.as_deref()),
            ("scenario_path", self.scenario_path.as_deref()),
            ("source_branch", self.source_branch.as_deref()),
            ("exact_head_sha", self.exact_head_sha.as_deref()),
            (
                "resolved_content_digest",
                self.resolved_content_digest.as_deref(),
            ),
        ] {
            if value.is_none_or(|value| value.trim().is_empty()) {
                diagnostics.push(format!("passing validator result is missing `{field}`"));
            }
        }
        if self.exact_head_sha.as_deref().is_some_and(|sha| {
            !matches!(sha.len(), 40 | 64)
                || !sha
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            diagnostics.push(
                "passing validator result has an invalid lowercase Git `exact_head_sha`"
                    .to_string(),
            );
        }
        if self
            .resolved_content_digest
            .as_deref()
            .is_some_and(|digest| !valid_sha256_digest(digest))
        {
            diagnostics.push(
                "passing validator result has an invalid `resolved_content_digest`".to_string(),
            );
        }
        if let (Some(name), Some(path)) =
            (self.scenario_name.as_deref(), self.scenario_path.as_deref())
        {
            if path != format!("scenarios/{name}") {
                diagnostics.push(
                    "passing validator result scenario path does not exactly match its name"
                        .to_string(),
                );
            }
        }
        match &self.standalone_binary {
            Some(binary)
                if binary.path.trim().is_empty()
                    || !valid_sha256(&binary.sha256)
                    || binary.size_bytes == 0 =>
            {
                diagnostics.push(
                    "passing validator result has an incomplete `standalone_binary`".to_string(),
                );
            }
            None => diagnostics
                .push("passing validator result is missing `standalone_binary`".to_string()),
            Some(_) => {}
        }
        if self.duration_ms.is_none_or(|duration| duration == 0) {
            diagnostics.push("passing validator result has no positive `duration_ms`".to_string());
        }
        if self.retained_paths.is_empty()
            || self
                .retained_paths
                .iter()
                .any(|path| path.trim().is_empty())
        {
            diagnostics.push("passing validator result has no complete retained paths".to_string());
        }
        let unique_retained = self
            .retained_paths
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if unique_retained.len() != self.retained_paths.len() {
            diagnostics.push("passing validator result has duplicate retained paths".to_string());
        }
        let required_assertions = self
            .validated_claims
            .iter()
            .chain(self.acceptance_criteria.iter())
            .filter(|assertion| assertion.required)
            .collect::<Vec<_>>();
        if required_assertions.is_empty() {
            diagnostics.push("passing validator result has no required assertions".to_string());
        }
        if self.evidence.is_empty() {
            diagnostics.push("passing validator result has no structured evidence".to_string());
        }
        if !self
            .evidence
            .iter()
            .any(|entry| entry.kind == EvidenceKind::ScenarioRun)
        {
            diagnostics
                .push("passing validator result has no live scenario-run evidence".to_string());
        }
        let mut evidence_ids = std::collections::BTreeSet::new();
        for evidence in &self.evidence {
            if evidence.id.trim().is_empty() || evidence.summary.trim().is_empty() {
                diagnostics.push("structured evidence has an empty id or summary".to_string());
            } else if !evidence_ids.insert(evidence.id.as_str()) {
                diagnostics.push(format!(
                    "duplicate structured evidence id `{}`",
                    evidence.id
                ));
            }
        }
        for assertion in required_assertions {
            if assertion.description.trim().is_empty() {
                diagnostics.push("required assertion has an empty description".to_string());
            }
            if !matches!(
                assertion.status,
                ValidationStatus::Satisfied | ValidationStatus::Observed
            ) {
                diagnostics.push(format!(
                    "required assertion did not pass: {} ({})",
                    assertion.description, assertion.status
                ));
            }
            if assertion.evidence_refs.is_empty() {
                diagnostics.push(format!(
                    "required assertion has no evidence reference: {}",
                    assertion.description
                ));
            }
            for reference in &assertion.evidence_refs {
                if !evidence_ids.contains(reference.as_str()) {
                    diagnostics.push(format!(
                        "required assertion references missing evidence `{reference}`: {}",
                        assertion.description
                    ));
                }
            }
        }
        diagnostics
    }
}

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_sha256)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
