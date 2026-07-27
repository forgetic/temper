// SPDX-License-Identifier: MPL-2.0

//! Structured exact-head validator evidence carried across the worker/daemon boundary.

use sha2::{Digest, Sha256};
use temper_protocol_worker::{JobContext, JobResult};
use temper_scenario_core::{ValidationVerdict, ValidatorResult};
use temper_workflow::ExactHeadValidationAuthority;

use crate::InFlightJob;

pub(super) const VALIDATOR_RESULT_FIELD: &str = "validator_result";
const BINDING_ID_FIELD: &str = "validation_binding_id";
const FEATURE_FIELD: &str = "validation_feature";
const PLAN_FIELD: &str = "validation_plan";
const SOURCE_BRANCH_FIELD: &str = "validation_source_branch";

pub(crate) fn requires_exact_head_validation(context: &JobContext) -> bool {
    value(context, BINDING_ID_FIELD).is_some()
}

pub(crate) fn parsed_validator_result(
    context: &JobContext,
    result: &JobResult,
) -> Result<Option<ValidatorResult>, String> {
    if !requires_exact_head_validation(context) {
        return Ok(None);
    }
    if result.verdict.as_deref() != Some("validated") {
        return Ok(None);
    }
    let payload = result
        .details
        .as_ref()
        .and_then(|details| details.get(VALIDATOR_RESULT_FIELD))
        .ok_or_else(|| {
            "workflow-native validated outcome is missing structured validator evidence".to_string()
        })?;
    let evidence: ValidatorResult = serde_json::from_value(payload.clone())
        .map_err(|error| format!("malformed structured validator evidence: {error}"))?;
    let diagnostics = evidence.validate_contract();
    if !diagnostics.is_empty() {
        return Err(format!(
            "structured validator evidence is incomplete: {}",
            diagnostics.join("; ")
        ));
    }
    if evidence.verdict != ValidationVerdict::Passed {
        return Err("only passed structured validator evidence can authorize landing".to_string());
    }
    validate_assignment_identity(context, result, &evidence)?;
    Ok(Some(evidence))
}

pub(super) fn authority_for_result(
    job: &InFlightJob,
    context: &JobContext,
    result: &JobResult,
) -> Result<Option<ExactHeadValidationAuthority>, String> {
    let Some(evidence) = parsed_validator_result(context, result)? else {
        return Ok(None);
    };
    let attempt_id = job
        .attempt_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "exact-head validation requires a durable attempt id".to_string())?;
    let canonical = serde_json::to_vec(&evidence)
        .map_err(|error| format!("serialize validated evidence: {error}"))?;
    let evidence_sha256 = format!("sha256:{:x}", Sha256::digest(canonical));
    Ok(Some(ExactHeadValidationAuthority {
        binding_id: required_value(context, BINDING_ID_FIELD)?.to_string(),
        attempt_id: attempt_id.to_string(),
        feature: evidence.feature.clone().expect("contract checked"),
        plan: evidence.plan.clone().expect("contract checked"),
        mapping_id: evidence.mapping_id.clone().expect("contract checked"),
        scenario_name: evidence.scenario_name.clone().expect("contract checked"),
        scenario_path: evidence.scenario_path.clone().expect("contract checked"),
        source_branch: evidence.source_branch.clone().expect("contract checked"),
        exact_head_sha: evidence.exact_head_sha.clone().expect("contract checked"),
        resolved_content_digest: evidence
            .resolved_content_digest
            .clone()
            .expect("contract checked"),
        binary_sha256: evidence
            .standalone_binary
            .as_ref()
            .expect("contract checked")
            .sha256
            .clone(),
        evidence_sha256,
        invalidated: false,
        retained_paths: evidence.retained_paths.clone(),
    }))
}

fn validate_assignment_identity(
    context: &JobContext,
    result: &JobResult,
    evidence: &ValidatorResult,
) -> Result<(), String> {
    let expected_feature = required_value(context, FEATURE_FIELD)?;
    let expected_plan = required_value(context, PLAN_FIELD)?;
    let expected_branch = required_value(context, SOURCE_BRANCH_FIELD)?;
    for (field, actual, expected) in [
        ("feature", evidence.feature.as_deref(), expected_feature),
        ("plan", evidence.plan.as_deref(), expected_plan),
        (
            "source_branch",
            evidence.source_branch.as_deref(),
            expected_branch,
        ),
    ] {
        if actual != Some(expected) {
            return Err(format!(
                "structured validator evidence `{field}` does not match assignment: expected `{expected}`, got `{}`",
                actual.unwrap_or("(missing)")
            ));
        }
    }
    if evidence.target.repo != context.repo {
        return Err(format!(
            "structured validator evidence repository `{}` does not match assignment `{}`",
            evidence.target.repo, context.repo
        ));
    }
    let expected_number = context.artifact.as_ref().map(|artifact| artifact.number);
    if evidence.target.reference.issue_number != expected_number {
        return Err(
            "structured validator evidence target does not match the assigned plan".to_string(),
        );
    }
    let scenario_name = evidence.scenario_name.as_deref().expect("contract checked");
    let expected_mapping = format!("{expected_feature}:{scenario_name}");
    if evidence.mapping_id.as_deref() != Some(expected_mapping.as_str()) {
        return Err(
            "structured validator evidence mapping id does not match feature and scenario"
                .to_string(),
        );
    }
    let scenario_path = evidence.scenario_path.as_deref().expect("contract checked");
    if !scenario_path
        .trim_end_matches('/')
        .ends_with(&format!("/{scenario_name}"))
    {
        return Err(
            "structured validator evidence scenario path does not match its name".to_string(),
        );
    }
    if result.attempt_id.as_deref().is_none_or(str::is_empty) {
        return Err(
            "structured validator result is not fenced to an assignment attempt".to_string(),
        );
    }
    Ok(())
}

fn required_value<'a>(context: &'a JobContext, key: &str) -> Result<&'a str, String> {
    value(context, key)
        .ok_or_else(|| format!("exact-head validation assignment is missing `{key}`"))
}

fn value<'a>(context: &'a JobContext, key: &str) -> Option<&'a str> {
    context
        .source_metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
