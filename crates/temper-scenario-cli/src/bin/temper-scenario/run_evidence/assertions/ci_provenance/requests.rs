// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use toml::Value;

use super::super::super::model::{CiRequestEvidence, RunEvidenceArtifact};
use super::super::support::{ResultBuilder, same_normalized};

const REQUEST_RULE_FIELDS: &[&str] = &[
    "method",
    "route",
    "route_contains",
    "authentication_scheme",
    "accepts_json",
    "query_keys",
    "at_least",
];

pub(super) fn evaluate_requests(
    mut builder: ResultBuilder,
    value: &Value,
    forbidden: bool,
    artifact: &RunEvidenceArtifact,
    provider_runs: &BTreeSet<String>,
) -> ResultBuilder {
    let label = if forbidden {
        "forbidden_requests"
    } else {
        "required_requests"
    };
    let Some(rules) = value.as_array() else {
        return builder.failed(format!("{label} must be an array of tables"));
    };
    match artifact.final_state.ci.request_capture_dropped {
        None => {
            builder = builder.missing_fact(
                "run evidence does not report whether CI request provenance was dropped",
            )
        }
        Some(dropped) if dropped > 0 => {
            builder = builder.failed(format!(
                "CI request provenance is incomplete: {dropped} request(s) were dropped"
            ))
        }
        Some(_) => {}
    }
    if artifact.final_state.ci.requests.is_empty() {
        return builder.missing_fact("run evidence contains no structured CI request provenance");
    }

    for (index, value) in rules.iter().enumerate() {
        let Some(rule) = value.as_table() else {
            builder = builder.failed(format!("{label}[{index}] must be a table"));
            continue;
        };
        for field in rule.keys() {
            if !REQUEST_RULE_FIELDS.contains(&field.as_str()) {
                builder = builder.failed(format!("unsupported {label}[{index}] field `{field}`"));
            }
        }
        let expanded = match expand_rule(rule, artifact, provider_runs) {
            Ok(expanded) => expanded,
            Err(message) => {
                builder = builder.missing_fact(format!("{label}[{index}]: {message}"));
                continue;
            }
        };
        let minimum = match rule.get("at_least") {
            Some(value) => match value.as_integer().filter(|count| *count >= 0) {
                Some(value) => value as usize,
                None => {
                    builder = builder.failed(format!(
                        "{label}[{index}].at_least must be a non-negative integer"
                    ));
                    continue;
                }
            },
            None => 1,
        };
        for expanded_rule in expanded {
            let matches = artifact
                .final_state
                .ci
                .requests
                .iter()
                .filter(|request| request_matches(request, &expanded_rule))
                .count();
            if forbidden {
                if matches == 0 {
                    builder =
                        builder.passed(format!("{label}[{index}] matched no retained requests"));
                } else {
                    builder = builder.failed(format!(
                        "{label}[{index}] matched {matches} retained request(s)"
                    ));
                }
            } else if matches >= minimum {
                builder = builder.passed(format!(
                    "{label}[{index}] matched {matches} request(s), required at least {minimum}"
                ));
            } else {
                builder = builder.failed(format!(
                    "{label}[{index}] matched {matches} request(s), required at least {minimum}"
                ));
            }
        }
    }
    builder
}

fn expand_rule(
    rule: &toml::Table,
    artifact: &RunEvidenceArtifact,
    provider_runs: &BTreeSet<String>,
) -> Result<Vec<toml::Table>, String> {
    let needs_repo = ["route", "route_contains"].iter().any(|field| {
        rule.get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("{repo}"))
    });
    let repo = if needs_repo {
        Some(
            artifact
                .provider
                .as_ref()
                .and_then(|provider| provider.repo_slug.as_deref())
                .and_then(|repo| super::nonempty(Some(repo)))
                .ok_or_else(|| {
                    "request template requires missing provider.repo_slug".to_string()
                })?,
        )
    } else {
        None
    };
    let needs_run = ["route", "route_contains"].iter().any(|field| {
        rule.get(*field)
            .and_then(Value::as_str)
            .is_some_and(|value| value.contains("{provider_run_id}"))
    });
    if needs_run && provider_runs.is_empty() {
        return Err("request template requires missing provider run identities".to_string());
    }
    let runs = if needs_run {
        provider_runs.iter().map(String::as_str).collect::<Vec<_>>()
    } else {
        vec![""]
    };
    Ok(runs
        .into_iter()
        .map(|run| {
            let mut expanded = rule.clone();
            for field in ["route", "route_contains"] {
                if let Some(value) = expanded.get_mut(field) {
                    if let Some(template) = value.as_str() {
                        *value = Value::String(
                            template
                                .replace("{repo}", repo.unwrap_or_default())
                                .replace("{provider_run_id}", run),
                        );
                    }
                }
            }
            expanded
        })
        .collect())
}

fn request_matches(request: &CiRequestEvidence, rule: &toml::Table) -> bool {
    if let Some(expected) = rule.get("method") {
        let Some(expected) = expected.as_str() else {
            return false;
        };
        if !same_normalized(&request.method, expected) {
            return false;
        }
    }
    if let Some(expected) = rule.get("route") {
        if expected.as_str() != Some(request.path.as_str()) {
            return false;
        }
    }
    if let Some(expected) = rule.get("route_contains") {
        let Some(expected) = expected.as_str() else {
            return false;
        };
        if !request.path.contains(expected) {
            return false;
        }
    }
    if let Some(expected) = rule.get("authentication_scheme") {
        let Some(expected) = expected.as_str() else {
            return false;
        };
        if !request.authentication_present
            || request
                .authentication_scheme
                .as_deref()
                .is_none_or(|actual| !same_normalized(actual, expected))
        {
            return false;
        }
    }
    if let Some(expected) = rule.get("accepts_json") {
        if expected.as_bool() != Some(request.accepts_json) {
            return false;
        }
    }
    if let Some(expected) = rule.get("query_keys") {
        let Some(expected) = expected.as_array() else {
            return false;
        };
        if !expected.iter().all(|key| {
            key.as_str()
                .is_some_and(|key| request.query_keys.iter().any(|actual| actual == key))
        }) {
            return false;
        }
    }
    true
}
