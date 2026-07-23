// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{Forge, Issue, UserId};

const MARKER_PREFIX: &str = "temper:comment-key=plan-validation:";
const EXPECTED_ROLE: &str = "tester";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationAuditEvidence {
    pub comment_id: String,
    pub author_id: String,
    pub outcome: String,
    pub summary: String,
    pub workflow_role: String,
    pub forge_actor: String,
    pub job_id: String,
    pub routed_transition: String,
    pub coordination_key: String,
}

pub(super) struct ValidationAuditExpectation<'a> {
    pub outcome: &'a str,
    pub summary: &'a str,
    pub transition: &'a str,
}

pub(super) async fn validation_audit_evidence(
    forge: &(impl Forge + ?Sized),
    plan: &Issue,
    expected: &[ValidationAuditExpectation<'_>],
) -> Result<Vec<ValidationAuditEvidence>, String> {
    // This is intentionally the portable ordinary-comments operation used by
    // the Forgejo backend, not a provider database or timeline lookup.
    let comments = forge
        .list_issue_comments(&plan.id)
        .await
        .map_err(|error| format!("list coordinating plan comments: {error}"))?;
    let audits = comments
        .iter()
        .filter(|comment| comment.body.contains(MARKER_PREFIX))
        .collect::<Vec<_>>();
    if audits.len() != expected.len() {
        let ids = audits
            .iter()
            .map(|comment| comment.id.as_str())
            .collect::<Vec<_>>();
        return Err(format!(
            "coordinating plan #{} must have exactly {} ordinary validation-audit comments; found {} with ids {ids:?}",
            plan.number,
            expected.len(),
            audits.len()
        ));
    }

    audits
        .into_iter()
        .zip(expected)
        .map(|(comment, expected)| parse_audit(comment, expected))
        .collect()
}

fn parse_audit(
    comment: &temper_forge_model::Comment,
    expected: &ValidationAuditExpectation<'_>,
) -> Result<ValidationAuditEvidence, String> {
    let body = &comment.body;
    require_contains(
        body,
        "validation outcome",
        &format!("**Outcome:** `{}`", expected.outcome),
    )?;
    require_contains(
        body,
        "safe validation summary",
        &format!("**Summary:** {}", expected.summary),
    )?;
    require_contains(body, "workflow role", "- Workflow role: `tester`")?;

    let expected_author = UserId::new(EXPECTED_ROLE);
    if comment.author_id != expected_author {
        return Err(format!(
            "validation-audit comment {} author mismatch: expected {}, got {}",
            comment.id, expected_author, comment.author_id
        ));
    }
    require_contains(
        body,
        "Forge actor identity",
        &format!(
            "- Forge actor: `{EXPECTED_ROLE}` (`{}`)",
            comment.author_id.as_str()
        ),
    )?;

    let job_id = backtick_field(body, "- Job ID: ")?;
    let routed_transition = backtick_field(body, "- Routed transition: ")?;
    if routed_transition != expected.transition {
        return Err(format!(
            "validation-audit routed transition mismatch: expected {:?}, got {routed_transition:?}",
            expected.transition
        ));
    }
    let coordination_key = backtick_field(body, "- Workspace coordination key: ")?;
    if coordination_key == "unavailable" {
        return Err("validation-audit workspace coordination key was unavailable".to_string());
    }

    require_valid_marker(comment)?;
    Ok(ValidationAuditEvidence {
        comment_id: comment.id.as_str().to_string(),
        author_id: comment.author_id.as_str().to_string(),
        outcome: expected.outcome.to_string(),
        summary: expected.summary.to_string(),
        workflow_role: EXPECTED_ROLE.to_string(),
        forge_actor: EXPECTED_ROLE.to_string(),
        job_id,
        routed_transition,
        coordination_key,
    })
}

fn require_valid_marker(comment: &temper_forge_model::Comment) -> Result<(), String> {
    let marker_prefix = format!("<!-- {MARKER_PREFIX}");
    let marker = comment
        .body
        .lines()
        .find(|line| line.starts_with(&marker_prefix))
        .ok_or_else(|| {
            format!(
                "validation-audit comment {} is missing its assignment-bound marker",
                comment.id
            )
        })?;
    let marker_key = marker
        .strip_prefix(&marker_prefix)
        .and_then(|value| value.strip_suffix(" -->"))
        .unwrap_or_default();
    let digest = marker_key
        .strip_prefix("assignment-sha256:")
        .unwrap_or_default();
    if comment.body.matches(MARKER_PREFIX).count() != 1
        || digest.len() != 64
        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "validation-audit comment {} must contain exactly one valid assignment-bound marker; got {marker:?}",
            comment.id
        ));
    }
    Ok(())
}

fn require_contains(body: &str, field: &str, expected: &str) -> Result<(), String> {
    if body.contains(expected) {
        Ok(())
    } else {
        Err(format!(
            "validation-audit comment is missing {field}: expected {expected:?}"
        ))
    }
}

fn backtick_field(body: &str, prefix: &str) -> Result<String, String> {
    let value = body
        .lines()
        .find_map(|line| line.strip_prefix(prefix))
        .and_then(|rest| rest.strip_prefix('`'))
        .and_then(|rest| rest.split_once('`').map(|(value, _)| value))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("validation-audit comment is missing non-empty field {prefix:?}"))?;
    Ok(value.to_string())
}
