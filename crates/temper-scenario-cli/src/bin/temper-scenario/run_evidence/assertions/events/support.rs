// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use toml::Value;

use super::super::super::model::{RunEvidenceArtifact, StructuredEventEvidence};
use super::super::support::{ResultBuilder, nonnegative_integer};

pub(super) struct EventMatcher {
    pub(super) event: String,
    fields: BTreeMap<String, String>,
}

impl EventMatcher {
    pub(super) fn from_table(
        table: &toml::Table,
        control_fields: &[&str],
        artifact: &RunEvidenceArtifact,
    ) -> Result<Self, String> {
        let event = table
            .get("event")
            .and_then(Value::as_str)
            .ok_or_else(|| "event expectation must include string field `event`".to_string())?
            .to_string();
        let mut fields = BTreeMap::new();
        for (key, value) in table {
            if control_fields.contains(&key.as_str()) || key == "event" {
                continue;
            }
            let Some(field_key) = field_alias(key) else {
                return Err(format!(
                    "field `{key}` is not supported in structured event expectations"
                ));
            };
            fields.insert(field_key.to_string(), expected_value(value, artifact)?);
        }
        if let Some(extra) = table.get("fields") {
            let Some(extra) = extra.as_table() else {
                return Err("event expectation `fields` must be a table".to_string());
            };
            for (key, value) in extra {
                fields.insert(key.clone(), expected_value(value, artifact)?);
            }
        }
        Ok(Self { event, fields })
    }

    pub(super) fn matches(&self, event: &StructuredEventEvidence) -> bool {
        event.event == self.event
            && self.fields.iter().all(|(key, expected)| {
                event
                    .fields
                    .get(key)
                    .is_some_and(|actual| actual == expected)
            })
    }

    pub(super) fn field_summary(&self) -> String {
        if self.fields.is_empty() {
            return "no additional field predicates".to_string();
        }
        self.fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(super) struct CountConstraints {
    min: Option<usize>,
    max: Option<usize>,
    exactly: Option<usize>,
}

impl CountConstraints {
    pub(super) fn from_table(table: &toml::Table) -> Result<Self, String> {
        let min = optional_count(table, "min")?;
        let max = optional_count(table, "max")?;
        let exactly = optional_count(table, "exactly")?.or(optional_count(table, "count")?);
        if min.is_none() && max.is_none() && exactly.is_none() {
            return Err(
                "count expectation must include one of `min`, `max`, `exactly`, or `count`"
                    .to_string(),
            );
        }
        Ok(Self { min, max, exactly })
    }

    pub(super) fn evaluate(
        &self,
        mut builder: ResultBuilder,
        actual: usize,
        label: String,
    ) -> ResultBuilder {
        if let Some(exactly) = self.exactly {
            if actual == exactly {
                builder = builder.passed(format!(
                    "{label}: expected exactly {exactly}, observed {actual}"
                ));
            } else {
                builder = builder.failed(format!(
                    "{label}: expected exactly {exactly}, observed {actual}"
                ));
            }
        }
        if let Some(min) = self.min {
            if actual >= min {
                builder = builder.passed(format!(
                    "{label}: expected at least {min}, observed {actual}"
                ));
            } else {
                builder = builder.failed(format!(
                    "{label}: expected at least {min}, observed {actual}"
                ));
            }
        }
        if let Some(max) = self.max {
            if actual <= max {
                builder = builder.passed(format!(
                    "{label}: expected at most {max}, observed {actual}"
                ));
            } else {
                builder = builder.failed(format!(
                    "{label}: expected at most {max}, observed {actual}"
                ));
            }
        }
        builder
    }
}

fn optional_count(table: &toml::Table, key: &str) -> Result<Option<usize>, String> {
    table
        .get(key)
        .map(|value| {
            nonnegative_integer(value).and_then(|count| {
                usize::try_from(count).map_err(|_| format!("{key} is too large for this platform"))
            })
        })
        .transpose()
}

fn expected_value(value: &Value, artifact: &RunEvidenceArtifact) -> Result<String, String> {
    if let Some(raw) = value.as_str() {
        return Ok(resolve_placeholder(raw, artifact));
    }
    if let Some(value) = value.as_integer() {
        return Ok(value.to_string());
    }
    if let Some(value) = value.as_bool() {
        return Ok(value.to_string());
    }
    Err("event field predicates must be strings, integers, or booleans".to_string())
}

fn resolve_placeholder(raw: &str, artifact: &RunEvidenceArtifact) -> String {
    if let Some(id) = raw.strip_prefix("$issue:") {
        return issue_ref_by_id(artifact, id).unwrap_or_else(|| raw.to_string());
    }
    if let Some(id) = raw.strip_prefix("$artifact:issue:") {
        return issue_ref_by_id(artifact, id).unwrap_or_else(|| raw.to_string());
    }
    if let Some(id) = raw.strip_prefix("$pr:") {
        return pull_request_ref_by_id(artifact, id).unwrap_or_else(|| raw.to_string());
    }
    if let Some(id) = raw.strip_prefix("$pull_request:") {
        return pull_request_ref_by_id(artifact, id).unwrap_or_else(|| raw.to_string());
    }
    if let Some(id) = raw.strip_prefix("$correlation:") {
        return issue_number_by_id(artifact, id)
            .map(|number| format!("pr-for-code-{number}"))
            .unwrap_or_else(|| raw.to_string());
    }
    match raw {
        "$source_artifact" | "$issue:intake" | "$issue:source" | "$artifact:issue:intake" => {
            source_issue_ref(artifact).unwrap_or_else(|| raw.to_string())
        }
        "$implementation_pr" | "$pull_request:implementation" | "$pr:implementation" => {
            implementation_pr_ref(artifact).unwrap_or_else(|| raw.to_string())
        }
        "$provider.issue_number" => artifact
            .provider
            .as_ref()
            .and_then(|provider| provider.issue_number)
            .map(|number| number.to_string())
            .unwrap_or_else(|| raw.to_string()),
        "$provider.pr_number" => artifact
            .provider
            .as_ref()
            .and_then(|provider| provider.pr_number)
            .map(|number| number.to_string())
            .unwrap_or_else(|| raw.to_string()),
        _ => raw.to_string(),
    }
}

fn issue_ref_by_id(artifact: &RunEvidenceArtifact, id: &str) -> Option<String> {
    let repo = repo_slug(artifact)?;
    let issue = issue_number_by_id(artifact, id)?;
    Some(format!("{repo}#{issue}"))
}

fn issue_number_by_id(artifact: &RunEvidenceArtifact, id: &str) -> Option<u64> {
    artifact
        .final_state
        .issues
        .iter()
        .find(|issue| issue.id.as_deref() == Some(id))
        .map(|issue| issue.number)
}

fn pull_request_ref_by_id(artifact: &RunEvidenceArtifact, id: &str) -> Option<String> {
    let repo = repo_slug(artifact)?;
    let pr = artifact
        .final_state
        .pull_requests
        .iter()
        .find(|pull_request| pull_request.id.as_deref() == Some(id))
        .map(|pull_request| pull_request.number)?;
    Some(format!("{repo} PR#{pr}"))
}

fn source_issue_ref(artifact: &RunEvidenceArtifact) -> Option<String> {
    let repo = repo_slug(artifact)?;
    let issue = artifact
        .provider
        .as_ref()
        .and_then(|provider| provider.issue_number)
        .or_else(|| {
            artifact
                .final_state
                .issues
                .iter()
                .find(|issue| {
                    issue
                        .id
                        .as_deref()
                        .is_some_and(|id| matches!(id, "intake" | "source"))
                })
                .map(|issue| issue.number)
        })
        .or_else(|| {
            artifact
                .final_state
                .issues
                .first()
                .map(|issue| issue.number)
        })?;
    Some(format!("{repo}#{issue}"))
}

fn implementation_pr_ref(artifact: &RunEvidenceArtifact) -> Option<String> {
    let repo = repo_slug(artifact)?;
    let pr = artifact
        .provider
        .as_ref()
        .and_then(|provider| provider.pr_number)
        .or_else(|| {
            artifact
                .final_state
                .pull_requests
                .iter()
                .find(|pull_request| pull_request.id.as_deref() == Some("implementation"))
                .map(|pull_request| pull_request.number)
        })
        .or_else(|| {
            artifact
                .final_state
                .pull_requests
                .first()
                .map(|pull_request| pull_request.number)
        })?;
    Some(format!("{repo} PR#{pr}"))
}

fn repo_slug(artifact: &RunEvidenceArtifact) -> Option<String> {
    artifact
        .provider
        .as_ref()
        .and_then(|provider| provider.repo_slug.clone())
        .or_else(|| {
            artifact
                .final_state
                .repositories
                .iter()
                .find_map(|repo| repo.slug.clone())
        })
}

pub(super) fn field_alias(key: &str) -> Option<&'static str> {
    match key {
        "event" => Some("event"),
        "service" => Some("service"),
        "artifact_ref" | "artifact.ref" => Some("artifact.ref"),
        "artifact_kind" | "artifact.kind" => Some("artifact.kind"),
        "pr_ref" | "pr.ref" => Some("pr.ref"),
        "source_artifact" => Some("source_artifact"),
        "transition" => Some("transition"),
        "conclusion" | "ci_conclusion" => Some("conclusion"),
        "kind" => Some("kind"),
        "queue_to" | "queue.to" => Some("queue.to"),
        "role" => Some("role"),
        "for_issue" | "for_issue_number" => Some("for_issue"),
        "scenario_run_id" | "scenario.run_id" => Some("scenario.run_id"),
        "action" => Some("action"),
        "pr_title" | "pr.title" => Some("pr.title"),
        "title_source" | "title.source" => Some("title.source"),
        "body_source" | "body.source" => Some("body.source"),
        "metadata_kind" | "metadata.kind" => Some("metadata.kind"),
        "metadata_parent_ref" | "metadata.parent.ref" => Some("metadata.parent.ref"),
        "correlation_key" | "correlation.key" => Some("correlation.key"),
        "tool_name" | "tool.name" => Some("tool.name"),
        "tool_model_visible" | "tool.model_visible" => Some("tool.model_visible"),
        "mcp_tool" | "mcp.tool" => Some("mcp.tool"),
        "mcp_project" | "mcp.project" => Some("mcp.project"),
        "repo_root" | "repo.root" => Some("repo.root"),
        "file_path" | "file.path" => Some("file.path"),
        "result_preview" | "result.preview" => Some("result.preview"),
        _ => None,
    }
}
