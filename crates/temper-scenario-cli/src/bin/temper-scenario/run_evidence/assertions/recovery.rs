// SPDX-License-Identifier: MPL-2.0

//! Declarative assertions for bounded model recovery and publication safety.

use std::collections::BTreeSet;

use toml::Value;

use super::super::model::{AssertionResultEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, nonnegative_integer, required_assertion};

pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    evaluate_provider_requests(expect, artifact, results);
    evaluate_recovery(expect, artifact, results);
    evaluate_stimuli(expect, artifact, results);
    evaluate_workspace(expect, artifact, results);
    evaluate_publication(expect, artifact, results);
}

fn tables<'a>(expect: &'a toml::Table, field: &str) -> Result<Vec<&'a toml::Table>, String> {
    let Some(value) = expect.get(field) else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| format!("expect.{field} must be an array of tables"))?;
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_table()
                .ok_or_else(|| format!("expect.{field}[{index}] must be a table"))
        })
        .collect()
}

fn builder(field: &str, index: usize, table: &toml::Table) -> Result<ResultBuilder, String> {
    let id = table
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.{field}[{index}]"));
    let required = required_assertion(table)?;
    Ok(ResultBuilder::new(
        id.clone(),
        format!("Structured recovery assertion `{id}`."),
        None,
    )
    .required(required))
}

fn reject_unknown_fields(
    mut result: ResultBuilder,
    table: &toml::Table,
    allowed: &[&str],
) -> ResultBuilder {
    for field in table
        .keys()
        .filter(|field| !allowed.contains(&field.as_str()))
    {
        result = result.unsupported(format!("unsupported assertion field `{field}`"));
    }
    result
}

fn push_shape_error(field: &str, error: String, results: &mut Vec<AssertionResultEvidence>) {
    results.push(
        ResultBuilder::new(
            format!("expect.{field}"),
            format!("Manifest `{field}` assertions are well-formed."),
            None,
        )
        .failed(error)
        .build(),
    );
}

fn count(table: &toml::Table, field: &str) -> Result<Option<usize>, String> {
    table
        .get(field)
        .map(|value| {
            nonnegative_integer(value).and_then(|value| {
                usize::try_from(value).map_err(|_| format!("{field} is too large"))
            })
        })
        .transpose()
}

fn evaluate_provider_requests(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let assertions = match tables(expect, "provider_requests") {
        Ok(value) => value,
        Err(error) => {
            push_shape_error("provider_requests", error, results);
            return;
        }
    };
    for (index, table) in assertions.into_iter().enumerate() {
        let mut result = match builder("provider_requests", index, table) {
            Ok(value) => reject_unknown_fields(
                value,
                table,
                &["id", "required", "role", "exactly", "min", "max", "unique"],
            ),
            Err(error) => {
                push_shape_error("provider_requests", error, results);
                continue;
            }
        };
        let role = table.get("role").and_then(Value::as_str);
        let Some(provider) = artifact.provider.as_ref() else {
            results.push(
                result
                    .missing_fact("run evidence has no provider request facts")
                    .build(),
            );
            continue;
        };
        let actual = role.map_or(provider.request_count, |role| {
            provider.request_counts_by_role.get(role).copied()
        });
        let Some(actual) = actual else {
            results.push(
                result
                    .missing_fact(format!(
                        "provider request count for role `{}` is absent",
                        role.unwrap_or("all")
                    ))
                    .build(),
            );
            continue;
        };
        for (field, relation) in [("exactly", 0u8), ("min", 1u8), ("max", 2u8)] {
            match count(table, field) {
                Ok(Some(expected)) => {
                    let passed = match relation {
                        0 => actual == expected,
                        1 => actual >= expected,
                        _ => actual <= expected,
                    };
                    if passed {
                        result = result.passed(format!(
                            "provider requests {field}={expected}, observed {actual}"
                        ));
                    } else {
                        result = result.failed(format!(
                            "provider requests {field}={expected}, observed {actual}"
                        ));
                    }
                }
                Ok(None) => {}
                Err(error) => result = result.failed(error),
            }
        }
        if table.get("unique").and_then(Value::as_bool) == Some(true) {
            if provider.request_ids.len() != provider.request_count.unwrap_or_default() {
                result = result.missing_fact("provider request identities are incomplete");
            } else {
                let unique = provider.request_ids.iter().collect::<BTreeSet<_>>().len();
                if unique == provider.request_ids.len() {
                    result = result.passed(format!(
                        "all {unique} provider request identities are unique"
                    ));
                } else {
                    result = result.failed(format!(
                        "observed {} provider requests but only {unique} identities",
                        provider.request_ids.len()
                    ));
                }
            }
        }
        if !table
            .keys()
            .any(|key| matches!(key.as_str(), "exactly" | "min" | "max" | "unique"))
        {
            result = result.failed("provider_requests assertion requires exactly/min/max/unique");
        }
        results.push(result.build());
    }
}

fn evaluate_recovery(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let assertions = match tables(expect, "recovery") {
        Ok(value) => value,
        Err(error) => {
            push_shape_error("recovery", error, results);
            return;
        }
    };
    for (index, table) in assertions.into_iter().enumerate() {
        let mut result = match builder("recovery", index, table) {
            Ok(value) => reject_unknown_fields(
                value,
                table,
                &[
                    "id",
                    "required",
                    "event",
                    "action",
                    "attempt",
                    "next_attempt",
                    "disposition",
                    "final_disposition",
                    "boundary",
                    "event_kind",
                    "provider_request_id",
                    "status_present",
                    "code_present",
                    "session_number",
                    "session_failure_count",
                    "cumulative_failure_count",
                    "elapsed_ms",
                    "deferral_count",
                    "generation",
                    "workstream_id",
                ],
            ),
            Err(error) => {
                push_shape_error("recovery", error, results);
                continue;
            }
        };
        let Some(events) = artifact.observability.as_ref().map(|value| &value.events) else {
            results.push(
                result
                    .missing_fact("run evidence has no recovery events")
                    .build(),
            );
            continue;
        };
        let expected_event = table.get("event").and_then(Value::as_str);
        let predicates = [
            ("action", "action"),
            ("attempt", "attempt"),
            ("next_attempt", "next_attempt"),
            ("disposition", "disposition"),
            ("final_disposition", "final_disposition"),
            ("boundary", "boundary"),
            ("event_kind", "event_kind"),
            ("provider_request_id", "provider_request_id"),
            ("status_present", "status_present"),
            ("code_present", "code_present"),
            ("session_number", "session_number"),
            ("session_failure_count", "session_failure_count"),
            ("cumulative_failure_count", "cumulative_failure_count"),
            ("elapsed_ms", "elapsed_ms"),
            ("deferral_count", "deferral_count"),
            ("generation", "generation"),
            ("workstream_id", "workstream_id"),
        ];
        if expected_event.is_none()
            && !predicates
                .iter()
                .any(|(manifest, _)| table.contains_key(*manifest))
        {
            results.push(
                result
                    .failed("recovery assertion requires `event` or a safe field predicate")
                    .build(),
            );
            continue;
        }
        let matched = events.iter().find(|event| {
            expected_event.is_none_or(|expected| event.event == expected)
                && predicates.iter().all(|(manifest, fact)| {
                    table.get(*manifest).is_none_or(|expected| {
                        scalar(expected).is_some_and(|expected| {
                            event
                                .fields
                                .get(*fact)
                                .is_some_and(|actual| actual == &expected)
                        })
                    })
                })
        });
        if let Some(event) = matched {
            result = result.passed(format!(
                "matched recovery event `{}` at sequence {}",
                event.event, event.sequence
            ));
        } else if events.is_empty() {
            result = result.missing_fact("structured event capture contains no events");
        } else {
            result = result.failed(format!(
                "no recovery event matched event={:?} and declared safe fields",
                expected_event
            ));
        }
        results.push(result.build());
    }
}

fn scalar(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_integer().map(|value| value.to_string()))
        .or_else(|| value.as_bool().map(|value| value.to_string()))
}

fn evaluate_stimuli(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let assertions = match tables(expect, "stimuli") {
        Ok(value) => value,
        Err(error) => {
            push_shape_error("stimuli", error, results);
            return;
        }
    };
    for (index, table) in assertions.into_iter().enumerate() {
        let mut result = match builder("stimuli", index, table) {
            Ok(value) => reject_unknown_fields(
                value,
                table,
                &[
                    "id",
                    "required",
                    "stimulus",
                    "action",
                    "status",
                    "attempts",
                    "details_contain",
                ],
            ),
            Err(error) => {
                push_shape_error("stimuli", error, results);
                continue;
            }
        };
        let Some(stimulus_id) = table.get("stimulus").and_then(Value::as_str) else {
            results.push(
                result
                    .failed("stimuli assertion requires `stimulus`")
                    .build(),
            );
            continue;
        };
        let Some(stimulus) = artifact
            .stimuli
            .iter()
            .find(|value| value.id == stimulus_id)
        else {
            results.push(
                result
                    .missing_fact(format!("stimulus `{stimulus_id}` is absent"))
                    .build(),
            );
            continue;
        };
        for (field, actual) in [
            ("action", stimulus.action.clone()),
            ("status", stimulus.status.clone()),
            ("attempts", stimulus.attempts.to_string()),
        ] {
            if let Some(expected) = table.get(field).and_then(scalar) {
                if actual == expected {
                    result = result.passed(format!("stimulus {field}={actual}"));
                } else {
                    result = result.failed(format!(
                        "stimulus {field}: expected {expected}, observed {actual}"
                    ));
                }
            }
        }
        if let Some(expected) = table.get("details_contain").and_then(Value::as_str) {
            if stimulus
                .details
                .iter()
                .any(|detail| detail.contains(expected))
            {
                result = result.passed(format!("stimulus details contain `{expected}`"));
            } else {
                result = result.failed(format!("stimulus details do not contain `{expected}`"));
            }
        }
        results.push(result.build());
    }
}

fn evaluate_workspace(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let assertions = match tables(expect, "workspace") {
        Ok(value) => value,
        Err(error) => {
            push_shape_error("workspace", error, results);
            return;
        }
    };
    for (index, table) in assertions.into_iter().enumerate() {
        let mut result = match builder("workspace", index, table) {
            Ok(value) => reject_unknown_fields(
                value,
                table,
                &[
                    "id",
                    "required",
                    "retained",
                    "path_contains",
                    "tool",
                    "tool_effects",
                    "max_tool_effects",
                ],
            ),
            Err(error) => {
                push_shape_error("workspace", error, results);
                continue;
            }
        };
        if table.get("retained").and_then(Value::as_bool) == Some(true) {
            if artifact.artifacts.artifact_paths.is_empty() {
                result =
                    result.missing_fact("run evidence has no retained workspace/artifact path");
            } else {
                result = result.passed(format!(
                    "retained {} workspace/artifact path(s)",
                    artifact.artifacts.artifact_paths.len()
                ));
            }
        }
        if let Some(expected) = table.get("path_contains").and_then(Value::as_str) {
            if artifact
                .artifacts
                .artifact_paths
                .iter()
                .any(|path| path.contains(expected))
            {
                result = result.passed(format!("retained path contains `{expected}`"));
            } else if artifact.artifacts.artifact_paths.is_empty() {
                result =
                    result.missing_fact("run evidence has no retained workspace/artifact path");
            } else {
                result = result.failed(format!("no retained path contains `{expected}`"));
            }
        }
        if let Some(tool) = table.get("tool").and_then(Value::as_str) {
            let Some(events) = artifact.observability.as_ref().map(|value| &value.events) else {
                results.push(
                    result
                        .missing_fact("run evidence has no tool-effect events")
                        .build(),
                );
                continue;
            };
            let actual = events
                .iter()
                .filter(|event| {
                    matches!(event.event.as_str(), "tool.start" | "agent.tool.effect")
                        && event
                            .fields
                            .get("tool")
                            .or_else(|| event.fields.get("tool.name"))
                            .is_some_and(|name| name == tool)
                })
                .count();
            for (field, exact) in [("tool_effects", true), ("max_tool_effects", false)] {
                match count(table, field) {
                    Ok(Some(expected))
                        if (exact && actual == expected) || (!exact && actual <= expected) =>
                    {
                        result = result.passed(format!(
                            "tool `{tool}` effects {field}={expected}, observed {actual}"
                        ));
                    }
                    Ok(Some(expected)) => {
                        result = result.failed(format!(
                            "tool `{tool}` effects {field}={expected}, observed {actual}"
                        ));
                    }
                    Ok(None) => {}
                    Err(error) => result = result.failed(error),
                }
            }
        }
        results.push(result.build());
    }
}

fn evaluate_publication(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let assertions = match tables(expect, "publication") {
        Ok(value) => value,
        Err(error) => {
            push_shape_error("publication", error, results);
            return;
        }
    };
    for (index, table) in assertions.into_iter().enumerate() {
        let mut result = match builder("publication", index, table) {
            Ok(value) => reject_unknown_fields(
                value,
                table,
                &[
                    "id",
                    "required",
                    "pull_requests",
                    "branches",
                    "blocked_while_deferred",
                ],
            ),
            Err(error) => {
                push_shape_error("publication", error, results);
                continue;
            }
        };
        let pull_requests = artifact.final_state.pull_requests.len();
        let branches = artifact
            .final_state
            .repositories
            .iter()
            .map(|repo| repo.branches.len())
            .sum::<usize>();
        for (field, actual) in [("pull_requests", pull_requests), ("branches", branches)] {
            match count(table, field) {
                Ok(Some(expected)) if actual == expected => {
                    result =
                        result.passed(format!("{field}: expected {expected}, observed {actual}"));
                }
                Ok(Some(expected)) => {
                    result =
                        result.failed(format!("{field}: expected {expected}, observed {actual}"));
                }
                Ok(None) => {}
                Err(error) => result = result.failed(error),
            }
        }
        if table.get("blocked_while_deferred").and_then(Value::as_bool) == Some(true) {
            let Some(events) = artifact.observability.as_ref().map(|value| &value.events) else {
                results.push(
                    result
                        .missing_fact("publication gate check requires structured events")
                        .build(),
                );
                continue;
            };
            let deferred = events
                .iter()
                .position(|event| event.event == "model.provider.deferred");
            let resumed = deferred.and_then(|start| {
                events[start + 1..]
                    .iter()
                    .position(|event| {
                        matches!(
                            event.event.as_str(),
                            "model.provider.wake" | "model.recovery.cleared"
                        )
                    })
                    .map(|offset| start + 1 + offset)
            });
            match (deferred, resumed) {
                (Some(start), Some(end)) => {
                    let premature = events[start + 1..end].iter().any(|event| {
                        matches!(event.event.as_str(), "pr.opened" | "pr.merged" | "validation.outcome" | "item.resolved")
                    });
                    if premature {
                        result = result.failed("publication/landing event occurred while provider recovery was deferred");
                    } else {
                        result = result.passed("no publication, validation, resolution, or landing event occurred while deferred");
                    }
                }
                _ => result = result.missing_fact("defer and subsequent wake/clear events are required to prove publication fencing"),
            }
        }
        results.push(result.build());
    }
}

#[cfg(test)]
#[path = "recovery/tests.rs"]
mod tests;
