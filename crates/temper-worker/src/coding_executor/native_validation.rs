// SPDX-License-Identifier: MPL-2.0

//! Worker-owned preparation and normalization of workflow-native exact-head validation.

mod runner;

pub use runner::NativeValidatorCommand;
pub(super) use runner::run;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use temper_protocol_agent::{WorkspaceResult, WorkspaceResultChild};
use temper_protocol_worker::FailureClass;
use temper_scenario_core::{
    ForgeIssueKey, ResolveFeatureScenarioRequest, ValidationVerdict, ValidatorResult,
    resolve_feature_scenario,
};

use crate::executor::JobOutcome;
use crate::managed_effect::JoinedBlocking;

use super::{JobMode, PreparedRepo, failure, workspace_failure};

const BINDING_FIELD: &str = "validation_binding_id";
const IDEMPOTENCY_FIELD: &str = "validation_idempotency_key";
const FEATURE_FIELD: &str = "validation_feature";
const PLAN_FIELD: &str = "validation_plan";
const SOURCE_BRANCH_FIELD: &str = "validation_source_branch";
const MAPPING_ID_FIELD: &str = "validation_mapping_id";
const SCENARIO_NAME_FIELD: &str = "validation_scenario_name";
const SCENARIO_PATH_FIELD: &str = "validation_scenario_path";
const MANIFEST_PATH_FIELD: &str = "validation_manifest_path";
const CONTENT_DIGEST_FIELD: &str = "validation_content_digest";
const EXACT_HEAD_FIELD: &str = "validation_exact_head_sha";

pub(super) fn configured(source_metadata: &temper_verdict::SourceMetadata) -> bool {
    source_metadata.get(BINDING_FIELD).is_some()
}

/// Resolves the workflow binding inside the prepared checkout before the agent
/// starts. These worker-owned fields make the selected scenario, digest, and
/// exact attempt visible to the constrained action without trusting model prose.
pub(super) async fn bind_resolved_mapping(
    source_metadata: &mut temper_verdict::SourceMetadata,
    mode: JobMode,
    prepared: &PreparedRepo,
    landing_base: &str,
) -> Result<(), JobOutcome> {
    if source_metadata.get(BINDING_FIELD).is_none() {
        return Ok(());
    }
    if mode != JobMode::ReadOnly {
        return Err(failure(
            FailureClass::Protocol,
            "workflow-native validation requires a read_only checkout",
        ));
    }

    let feature = required_metadata(source_metadata, FEATURE_FIELD)?
        .parse::<ForgeIssueKey>()
        .map_err(|error| {
            failure(
                FailureClass::Protocol,
                format!("validation assignment has invalid feature identity: {error}"),
            )
        })?;
    let expected_plan = required_metadata(source_metadata, PLAN_FIELD)?.to_string();
    let expected_branch = required_metadata(source_metadata, SOURCE_BRANCH_FIELD)?.to_string();
    let checkout_root = prepared.workspace.path().to_path_buf();
    let request = ResolveFeatureScenarioRequest::new(
        checkout_root,
        PathBuf::from(temper_scenario_core::DEFAULT_SCENARIOS_DIR),
        feature,
        landing_base.to_string(),
    );
    let resolved = JoinedBlocking::spawn("temper-validator-mapping", move || {
        resolve_feature_scenario(&request)
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join exact-head scenario resolver: {error}"),
        )
    })?
    .map_err(|error| {
        failure(
            FailureClass::Permanent,
            format!("resolve exact-head feature scenario: {error}"),
        )
    })?;

    if resolved.plan.as_ref().map(ToString::to_string).as_deref() != Some(&expected_plan) {
        return Err(failure(
            FailureClass::Protocol,
            "mapped scenario plan identity does not match the validation assignment",
        ));
    }
    if resolved.source_branch != expected_branch {
        return Err(failure(
            FailureClass::Protocol,
            "mapped scenario source branch does not match the validation assignment",
        ));
    }
    if resolved.head_sha != prepared.start_head_sha {
        return Err(failure(
            FailureClass::Canceled,
            "mapped scenario resolution does not match the prepared checkout head",
        ));
    }

    source_metadata.insert(MAPPING_ID_FIELD.to_string(), resolved.mapping_id);
    source_metadata.insert(SCENARIO_NAME_FIELD.to_string(), resolved.scenario_name);
    source_metadata.insert(SCENARIO_PATH_FIELD.to_string(), resolved.scenario_path);
    source_metadata.insert(MANIFEST_PATH_FIELD.to_string(), resolved.manifest_path);
    source_metadata.insert(CONTENT_DIGEST_FIELD.to_string(), resolved.digest);
    source_metadata.insert(EXACT_HEAD_FIELD.to_string(), resolved.head_sha.clone());
    if let Some(template) = source_metadata.get(IDEMPOTENCY_FIELD).cloned() {
        let binding = required_metadata(source_metadata, BINDING_FIELD)?;
        let issue_number = expected_plan
            .rsplit_once('#')
            .map_or("unknown", |(_, number)| number);
        let key = template
            .replace("{binding_id}", binding)
            .replace("{issue_number}", issue_number)
            .replace("{exact_head_sha}", &resolved.head_sha);
        source_metadata.insert(IDEMPOTENCY_FIELD.to_string(), key);
    }
    Ok(())
}

pub(super) async fn normalize(
    mut result: WorkspaceResult,
    source_metadata: &temper_verdict::SourceMetadata,
    mode: JobMode,
    prepared: &PreparedRepo,
) -> Result<(WorkspaceResult, Option<Value>), JobOutcome> {
    if source_metadata.get(BINDING_FIELD).is_none() {
        return Ok((result, None));
    }
    if mode != JobMode::ReadOnly {
        return Err(failure(
            FailureClass::Protocol,
            "workflow-native validation requires a read_only checkout",
        ));
    }
    if prepared
        .workspace
        .has_changes()
        .await
        .map_err(|error| workspace_failure("inspect validator checkout", error))?
    {
        prepared
            .workspace
            .discard_changes()
            .await
            .map_err(|error| workspace_failure("discard validator checkout mutation", error))?;
        return Err(failure(
            FailureClass::Permanent,
            "workflow-native validator mutated its read-only checkout",
        ));
    }

    let raw = result.body.as_deref().ok_or_else(|| {
        failure(
            FailureClass::Protocol,
            "workflow-native validator returned no typed evidence payload",
        )
    })?;
    let evidence: ValidatorResult = serde_json::from_str(raw).map_err(|error| {
        failure(
            FailureClass::Protocol,
            format!("workflow-native validator returned malformed evidence: {error}"),
        )
    })?;
    let diagnostics = evidence.validate_contract();
    if !diagnostics.is_empty() {
        return Err(failure(
            FailureClass::Protocol,
            format!(
                "workflow-native validator evidence failed its contract: {}",
                diagnostics.join("; ")
            ),
        ));
    }
    validate_assignment_identity(&evidence, source_metadata, &prepared.start_head_sha)?;
    if evidence.verdict == ValidationVerdict::Passed {
        verify_binary_identity(&evidence, prepared.workspace.path()).await?;
        verify_retained_paths(&evidence, prepared.workspace.path()).await?;
    }

    result.verdict = Some(
        match evidence.verdict {
            ValidationVerdict::Passed => "validated",
            ValidationVerdict::Failed | ValidationVerdict::Inconclusive => "needs_followup",
        }
        .to_string(),
    );
    result.title = (evidence.verdict == ValidationVerdict::Passed).then(|| {
        format!(
            "Land validated feature head {}",
            short_sha(&prepared.start_head_sha)
        )
    });
    result.summary = Some(format!(
        "{} exact-head scenario {} at {}",
        evidence.verdict,
        evidence.scenario_name.as_deref().unwrap_or("(unmapped)"),
        short_sha(&prepared.start_head_sha)
    ));
    result.body = Some(evidence.render_markdown());
    if evidence.verdict != ValidationVerdict::Passed && result.children.is_empty() {
        let follow_up = evidence.follow_up_issue.as_ref().ok_or_else(|| {
            failure(
                FailureClass::Protocol,
                "failed or inconclusive validation requires structured follow-up intent",
            )
        })?;
        result.children.push(WorkspaceResultChild {
            slug: format!("validation-{}", short_sha(&prepared.start_head_sha)),
            title: follow_up.title.clone(),
            body: follow_up.body.clone(),
            kind: Some("code".to_string()),
            labels: follow_up.labels.clone(),
            depends_on: Vec::new(),
            target_repo: None,
        });
    }
    let payload = serde_json::to_value(&evidence).map_err(|error| {
        failure(
            FailureClass::Protocol,
            format!("serialize workflow-native validator evidence: {error}"),
        )
    })?;
    Ok((result, Some(json!({ "validator_result": payload }))))
}

fn validate_assignment_identity(
    evidence: &ValidatorResult,
    metadata: &temper_verdict::SourceMetadata,
    checkout_head: &str,
) -> Result<(), JobOutcome> {
    for (field, actual, metadata_key) in [
        ("feature", evidence.feature.as_deref(), FEATURE_FIELD),
        ("plan", evidence.plan.as_deref(), PLAN_FIELD),
        (
            "source_branch",
            evidence.source_branch.as_deref(),
            SOURCE_BRANCH_FIELD,
        ),
        (
            "mapping_id",
            evidence.mapping_id.as_deref(),
            MAPPING_ID_FIELD,
        ),
        (
            "scenario_name",
            evidence.scenario_name.as_deref(),
            SCENARIO_NAME_FIELD,
        ),
        (
            "scenario_path",
            evidence.scenario_path.as_deref(),
            SCENARIO_PATH_FIELD,
        ),
        (
            "resolved_content_digest",
            evidence.resolved_content_digest.as_deref(),
            CONTENT_DIGEST_FIELD,
        ),
        (
            "exact_head_sha",
            evidence.exact_head_sha.as_deref(),
            EXACT_HEAD_FIELD,
        ),
    ] {
        let expected = required_metadata(metadata, metadata_key)?;
        if actual != Some(expected) {
            return Err(failure(
                FailureClass::Protocol,
                format!("validator evidence `{field}` does not match its assignment"),
            ));
        }
    }
    if evidence.exact_head_sha.as_deref() != Some(checkout_head) {
        return Err(failure(
            FailureClass::Canceled,
            "validator evidence does not match the exact read-only checkout head",
        ));
    }
    if evidence.target.repo
        != required_metadata(metadata, FEATURE_FIELD)?
            .rsplit_once('#')
            .map_or("", |(repo, _)| repo)
    {
        return Err(failure(
            FailureClass::Protocol,
            "validator evidence target repository does not match its assignment",
        ));
    }
    let expected_plan_number = required_metadata(metadata, PLAN_FIELD)?
        .rsplit_once('#')
        .and_then(|(_, number)| number.parse::<u64>().ok());
    if evidence.target.reference.issue_number != expected_plan_number {
        return Err(failure(
            FailureClass::Protocol,
            "validator evidence target does not identify the assigned plan",
        ));
    }
    Ok(())
}

async fn verify_retained_paths(
    evidence: &ValidatorResult,
    checkout_root: &Path,
) -> Result<(), JobOutcome> {
    let retained = evidence.retained_paths.clone();
    let checkout_root = checkout_root.to_path_buf();
    JoinedBlocking::spawn("temper-validator-retained", move || {
        for declared in retained {
            let path = Path::new(&declared);
            let candidate = if path.is_absolute() {
                path.to_path_buf()
            } else {
                checkout_root.join(path)
            };
            if !candidate.exists() {
                return Err(format!(
                    "retained validation artifact does not exist: {}",
                    candidate.display()
                ));
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join retained validation artifact verifier: {error}"),
        )
    })?
    .map_err(|message| failure(FailureClass::Protocol, message))
}

async fn verify_binary_identity(
    evidence: &ValidatorResult,
    checkout_root: &Path,
) -> Result<(), JobOutcome> {
    let binary = evidence
        .standalone_binary
        .as_ref()
        .expect("passing contract requires binary identity")
        .clone();
    let checkout_root = checkout_root.to_path_buf();
    JoinedBlocking::spawn("temper-validator-binary", move || {
        let checkout = fs::canonicalize(&checkout_root)
            .map_err(|error| format!("canonicalize validator checkout: {error}"))?;
        let declared = Path::new(&binary.path);
        let candidate = if declared.is_absolute() {
            declared.to_path_buf()
        } else {
            checkout.join(declared)
        };
        let path = fs::canonicalize(&candidate).map_err(|error| {
            format!("resolve standalone binary {}: {error}", candidate.display())
        })?;
        if !path.starts_with(&checkout) || !path.is_file() {
            return Err(
                "standalone binary is not a regular file derived inside the exact checkout"
                    .to_string(),
            );
        }
        let bytes = fs::read(&path)
            .map_err(|error| format!("read standalone binary {}: {error}", path.display()))?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if size != binary.size_bytes || sha256 != binary.sha256 {
            return Err("standalone binary identity does not match the retained file".to_string());
        }
        Ok(())
    })
    .await
    .map_err(|error| {
        failure(
            FailureClass::Transient,
            format!("join standalone binary verifier: {error}"),
        )
    })?
    .map_err(|message| failure(FailureClass::Protocol, message))
}

fn required_metadata<'a>(
    metadata: &'a temper_verdict::SourceMetadata,
    key: &str,
) -> Result<&'a str, JobOutcome> {
    metadata
        .get(key)
        .map(String::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            failure(
                FailureClass::Protocol,
                format!("validation assignment is missing `{key}`"),
            )
        })
}

fn short_sha(sha: &str) -> &str {
    sha.get(..sha.len().min(12)).unwrap_or(sha)
}
