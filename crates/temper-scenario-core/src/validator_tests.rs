// SPDX-License-Identifier: MPL-2.0

use crate::{
    AcceptanceCriterion, EvidenceEntry, EvidenceKind, FollowUpIssueIntent, ValidatedClaim,
    ValidationReport, ValidationStatus, ValidationVerdict, ValidatorBinaryIdentity,
    ValidatorContext, ValidatorResult, ValidatorResultTarget,
};

#[test]
fn validator_schema_preserves_report_vocabulary() {
    for (verdict, expected) in [
        (ValidationVerdict::Passed, "passed"),
        (ValidationVerdict::Failed, "failed"),
        (ValidationVerdict::Inconclusive, "inconclusive"),
    ] {
        assert_eq!(
            serde_json::to_value(verdict).expect("verdict serializes"),
            serde_json::json!(expected)
        );
    }

    for (status, expected) in [
        (ValidationStatus::Satisfied, "satisfied"),
        (ValidationStatus::Observed, "observed"),
        (ValidationStatus::Failed, "failed"),
        (ValidationStatus::Unproven, "unproven"),
        (ValidationStatus::NotApplicable, "not applicable"),
    ] {
        assert_eq!(
            serde_json::to_value(status).expect("status serializes"),
            serde_json::json!(expected)
        );
    }

    for (kind, expected) in [
        (EvidenceKind::ScenarioCheck, "scenario_check"),
        (EvidenceKind::ScenarioRun, "scenario_run"),
        (EvidenceKind::Command, "command"),
        (EvidenceKind::Artifact, "artifact"),
        (EvidenceKind::Observation, "observation"),
    ] {
        assert_eq!(
            serde_json::to_value(kind).expect("evidence kind serializes"),
            serde_json::json!(expected)
        );
    }
}

#[test]
fn validator_context_json_fixtures_round_trip_and_summarize() {
    let per_pr: ValidatorContext =
        serde_json::from_str(VALIDATOR_CONTEXT_PER_PR_JSON).expect("per-pr context fixture parses");
    assert_eq!(per_pr.schema, crate::VALIDATOR_CONTEXT_SCHEMA);
    assert_eq!(per_pr.target.kind, "implementation_pr");
    assert_eq!(per_pr.pull_requests.len(), 1);
    round_trip_context(&per_pr);
    assert_eq!(
        per_pr.summary(),
        "schema: temper.validator.context.v1\
         \ntarget: implementation_pr PR #70, merged `abc123`, observed `abc123` in ai/temper\
         \nbinding: validate_each_merged_implementation_pr (validator / validate_merged_pr)\
         \npull_requests: #70@abc123\
         \nissues: #35 (issue)\
         \naggregate: none\
         \nworkflow: validator / validate_merged_pr / validation\n"
    );

    let aggregate: ValidatorContext = serde_json::from_str(VALIDATOR_CONTEXT_AGGREGATE_JSON)
        .expect("aggregate context fixture parses");
    assert_eq!(aggregate.target.kind, "epic");
    assert_eq!(aggregate.pull_requests.len(), 2);
    assert!(aggregate.aggregate.is_some());
    round_trip_context(&aggregate);
    assert_eq!(
        aggregate.summary(),
        "schema: temper.validator.context.v1\
         \ntarget: epic issue #35 in ai/temper\
         \nbinding: validate_epic_when_ready (validator / validate_epic)\
         \npull_requests: #65@abc123, #67@def456\
         \nissues: #35 (epic), #60 (issue), #62 (issue)\
         \naggregate: 2 child issues complete, 2 produced PRs merged via validation-ready label or all children complete\
         \nworkflow: validator / validate_epic / validation\n"
    );
}

#[test]
fn validator_result_json_fixtures_round_trip_and_render() {
    let per_pr: ValidatorResult =
        serde_json::from_str(VALIDATOR_RESULT_PER_PR_JSON).expect("per-pr result fixture parses");
    assert_eq!(per_pr.schema, crate::VALIDATOR_RESULT_SCHEMA);
    assert_eq!(per_pr.target.kind, "implementation_pr");
    assert_eq!(per_pr.verdict, ValidationVerdict::Inconclusive);
    round_trip_result(&per_pr);

    let aggregate: ValidatorResult = serde_json::from_str(VALIDATOR_RESULT_AGGREGATE_JSON)
        .expect("aggregate result fixture parses");
    assert_eq!(aggregate.target.kind, "epic");
    assert_eq!(aggregate.verdict, ValidationVerdict::Passed);
    assert_eq!(
        serde_json::to_value(aggregate.evidence[0].kind).expect("kind serializes"),
        serde_json::json!("scenario_check")
    );
    assert_eq!(
        serde_json::to_value(aggregate.acceptance_criteria[1].status).expect("status serializes"),
        serde_json::json!("not applicable")
    );
    round_trip_result(&aggregate);

    assert_eq!(
        aggregate.render_markdown(),
        concat!(
            "# Validation report\n",
            "\n",
            "- Target: epic issue #35 in ai/temper\n",
            "- State fingerprint: `aggregate:abc123-def456`\n",
            "- Trigger: validation-ready after all child issues completed\n",
            "- Verdict: passed\n",
            "\n",
            "## Related PRs\n",
            "\n",
            "- #65 (source issue #60), merged `abc123`\n",
            "- #67 (source issue #62), merged `def456`\n",
            "\n",
            "## Validated claims\n",
            "\n",
            "- [satisfied] The aggregate target includes all completed child issues and merged PRs.\n",
            "  - evidence: `aggregate-rollup`\n",
            "- [observed] Scenario metadata remains available to validators.\n",
            "  - evidence: `scenario-check`\n",
            "\n",
            "## Acceptance criteria\n",
            "\n",
            "- [satisfied] Aggregate targets preserve PR-level merge and source issue details.\n",
            "  - evidence: `aggregate-rollup`\n",
            "- [not applicable] No follow-up issue is required for a passed validation.\n",
            "  - evidence: `aggregate-rollup`\n",
            "\n",
            "## Evidence\n",
            "\n",
            "1. **scenario check** `scenario-check` — basic-delivery scenario metadata was present and valid.\n",
            "   - uri: artifact://scenario/basic-delivery.json\n",
            "2. **observation** `aggregate-rollup` — 2 child issues complete and 2 produced PRs merged.\n",
            "   - PR #65 -> abc123\n",
            "     PR #67 -> def456\n",
            "\n",
            "## Limitations\n",
            "\n",
            "- None recorded.\n",
            "\n",
            "## Follow-up intent\n",
            "\n",
            "- None recorded.\n",
            "\n",
            "## Scenario promotion intent\n",
            "\n",
            "- Scenario: aggregate-validation-rollup\n",
            "- Intent: Exercise aggregate validator handoff bundles with multiple related PRs.\n",
            "- Proposed effects: issue=true, pr=false\n",
            "- Source evidence:\n",
            "  - aggregate-rollup\n",
            "- Fixture notes:\n",
            "> Use a two-child epic fixture with both produced PRs merged.\n"
        )
    );
}

#[test]
fn strict_validator_contract_rejects_missing_or_unproven_required_evidence() {
    let mut result = ValidatorResult::new(
        ValidatorResultTarget::new("feature", "ai/temper", crate::ArtifactReference::issue(778)),
        ValidationVerdict::Passed,
    );
    result.feature = Some("ai/temper#778".to_string());
    result.plan = Some("ai/temper#779".to_string());
    result.scenario_name = Some("exact-head-feature-validation".to_string());
    result.scenario_path = Some("scenarios/exact-head-feature-validation".to_string());
    result.source_branch = Some("feature/778-exact-head-validation".to_string());
    result.exact_head_sha = Some("deadbeef".to_string());
    result.resolved_content_digest = Some("sha256:cafebabe".to_string());
    result.standalone_binary = Some(ValidatorBinaryIdentity {
        path: "target/debug/temper".to_string(),
        sha256: "012345".to_string(),
        size_bytes: 42,
    });
    result.duration_ms = Some(1200);
    result.retained_paths = vec!["artifacts/run-evidence.json".to_string()];
    result.evidence.push(crate::StructuredEvidenceEntry::new(
        "run-evidence",
        EvidenceKind::ScenarioRun,
        "Exact-head live scenario evidence was retained.",
    ));
    result
        .acceptance_criteria
        .push(crate::ValidationAssertion::new(
            "Exact-head assertion passed.",
            ValidationStatus::Satisfied,
        ));
    assert!(result.validate_contract().is_empty());

    result
        .acceptance_criteria
        .push(crate::ValidationAssertion::new(
            "Required structured fact was absent.",
            ValidationStatus::Unproven,
        ));
    let diagnostics = result.validate_contract();
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("required assertion did not pass")),
        "{diagnostics:?}"
    );

    result
        .acceptance_criteria
        .last_mut()
        .expect("optional assertion")
        .required = false;
    assert!(result.validate_contract().is_empty());
}

#[test]
fn legacy_validator_result_remains_readable_but_cannot_authorize_passing() {
    let legacy_json = VALIDATOR_RESULT_AGGREGATE_JSON.replace(
        crate::VALIDATOR_RESULT_SCHEMA,
        crate::validator_result::LEGACY_VALIDATOR_RESULT_SCHEMA,
    );
    let legacy: ValidatorResult = serde_json::from_str(&legacy_json).expect("v1 result parses");
    assert!(legacy.acceptance_criteria.iter().all(|item| item.required));
    assert!(
        legacy
            .validate_contract()
            .iter()
            .any(|diagnostic| diagnostic.contains("requires validator result v2"))
    );
}

#[test]
fn structured_result_converts_temporary_validation_report_fields() {
    let mut report = ValidationReport::new(123, "deadbeef", ValidationVerdict::Failed);
    report.validated_claims.push(
        ValidatedClaim::new("Claim from Markdown bridge.", ValidationStatus::Failed)
            .with_evidence("evidence-1"),
    );
    report.acceptance_criteria.push(
        AcceptanceCriterion::new(
            "Criterion from Markdown bridge.",
            ValidationStatus::Unproven,
        )
        .with_evidence("evidence-1"),
    );
    report.evidence.push(
        EvidenceEntry::new(EvidenceKind::Command, "Command failed.")
            .with_detail("exit status: 1")
            .with_detail("stderr: boom"),
    );
    report
        .limitations
        .push("No live Forgejo lookup.".to_string());
    report.follow_up = Some(
        FollowUpIssueIntent::new(
            "Repair validation failure",
            "Investigate the failed command.",
        )
        .with_label("validation")
        .with_relation_hint("relates to PR #123"),
    );

    let result = ValidatorResult::from_validation_report(report, "ai/temper");

    assert_eq!(result.target.kind, "implementation_pr");
    assert_eq!(result.target.reference.pr_number, Some(123));
    assert_eq!(
        result.related_prs[0].merged_main_sha.as_deref(),
        Some("deadbeef")
    );
    assert_eq!(
        result.validated_claims[0].evidence_refs,
        vec!["evidence-1".to_string()]
    );
    assert_eq!(
        result.evidence[0].details.as_deref(),
        Some("exit status: 1\nstderr: boom")
    );
    assert_eq!(
        result
            .follow_up_issue
            .as_ref()
            .expect("follow-up preserved")
            .relation_hints,
        vec!["relates to PR #123".to_string()]
    );
}

const VALIDATOR_CONTEXT_PER_PR_JSON: &str =
    include_str!("../tests/fixtures/validator-context-per-pr.json");
const VALIDATOR_CONTEXT_AGGREGATE_JSON: &str =
    include_str!("../tests/fixtures/validator-context-aggregate.json");
const VALIDATOR_RESULT_PER_PR_JSON: &str =
    include_str!("../tests/fixtures/validator-result-per-pr.json");
const VALIDATOR_RESULT_AGGREGATE_JSON: &str =
    include_str!("../tests/fixtures/validator-result-aggregate.json");

fn round_trip_context(context: &ValidatorContext) {
    let json = serde_json::to_string_pretty(context).expect("context serializes");
    let back: ValidatorContext = serde_json::from_str(&json).expect("context deserializes");
    assert_eq!(&back, context);
}

fn round_trip_result(result: &ValidatorResult) {
    let json = serde_json::to_string_pretty(result).expect("result serializes");
    let back: ValidatorResult = serde_json::from_str(&json).expect("result deserializes");
    assert_eq!(&back, result);
}
