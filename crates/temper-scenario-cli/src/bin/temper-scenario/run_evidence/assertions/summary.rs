// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeMap;

use toml::Value;

use super::super::model::{AssertionResultEvidence, IssueStateEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, has_label, nonnegative_integer, state_is};

pub(super) fn evaluate_templates(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    for template in template_names(expect) {
        match template.as_str() {
            "single-pr-merged-source-closed" => {
                results.push(evaluate_single_pr_merged_source_closed(artifact));
            }
            "no-duplicate-prs" => results.push(evaluate_no_duplicate_prs(
                "template:no-duplicate-prs".to_string(),
                "No duplicate implementation PRs are present.".to_string(),
                artifact,
            )),
            other => results.push(ResultBuilder::new(
                format!("template:{other}"),
                format!("Assertion template `{other}` is declared."),
                None,
            )
            .unsupported(format!(
                "template `{other}` is known to manifest validation but is not backed by structured run-evidence assertions yet"
            ))
            .build()),
        }
    }
}

fn template_names(expect: &toml::Table) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = expect.get("template").and_then(Value::as_str) {
        names.push(name.to_string());
    }
    if let Some(array) = expect.get("templates").and_then(Value::as_array) {
        names.extend(array.iter().filter_map(Value::as_str).map(str::to_string));
    }
    names
}

fn evaluate_single_pr_merged_source_closed(
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let mut builder = ResultBuilder::new(
        "template:single-pr-merged-source-closed".to_string(),
        "One implementation PR merges and closes its source issue.".to_string(),
        None,
    );

    if artifact.final_state.pull_requests.is_empty() {
        builder = builder.missing_fact("run evidence has no final pull request facts");
    } else {
        let merged = artifact
            .final_state
            .pull_requests
            .iter()
            .filter(|pull_request| state_is(pull_request.state.as_deref(), "merged"))
            .count();
        if merged == 1 {
            builder = builder.passed("observed exactly 1 merged pull request");
        } else {
            builder = builder.failed(format!(
                "expected exactly 1 merged pull request, observed {merged}"
            ));
        }
    }

    match source_issue_candidates(artifact) {
        SourceIssueCandidates::Issues(issues) => {
            let closed = issues
                .iter()
                .filter(|issue| state_is(issue.state.as_deref(), "closed"))
                .count();
            if closed == 1 {
                builder = builder.passed("observed exactly 1 closed source/parent issue");
            } else {
                builder = builder.failed(format!(
                    "expected exactly 1 closed source/parent issue, observed {closed}"
                ));
            }
        }
        SourceIssueCandidates::MissingFact(message) => {
            builder = builder.missing_fact(message);
        }
    }

    builder.build()
}

pub(super) fn evaluate_counts(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    for field in [
        "merged_pull_requests",
        "closed_parent_issues",
        "created_pull_requests",
        "refreshed_pull_requests",
    ] {
        let Some(value) = expect.get(field) else {
            continue;
        };
        let expected = match nonnegative_integer(value) {
            Ok(expected) => expected,
            Err(message) => {
                results.push(
                    ResultBuilder::new(
                        format!("expect.{field}"),
                        format!("Count expectation `{field}` is well-formed."),
                        None,
                    )
                    .failed(message)
                    .build(),
                );
                continue;
            }
        };

        match field {
            "merged_pull_requests" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Merged pull request count matches the manifest expectation.".to_string(),
                expected,
                count_merged_pull_requests(artifact),
                "run evidence has no final pull request facts".to_string(),
            )),
            "closed_parent_issues" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Closed parent/source issue count matches the manifest expectation.".to_string(),
                expected,
                count_closed_parent_issues(artifact),
                "run evidence has no final issue facts".to_string(),
            )),
            "created_pull_requests" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Created pull request action count matches the manifest expectation.".to_string(),
                expected,
                count_pull_request_action_events(artifact, "pr.opened", "created"),
                "run evidence has no structured pr.opened action facts".to_string(),
            )),
            "refreshed_pull_requests" => results.push(evaluate_count(
                format!("expect.{field}"),
                "Refreshed pull request action count matches the manifest expectation.".to_string(),
                expected,
                count_pull_request_action_events(artifact, "pr.updated", "refreshed"),
                "run evidence has no structured pr.updated action facts".to_string(),
            )),
            _ => {}
        }
    }
}

fn evaluate_count(
    id: String,
    description: String,
    expected: u64,
    actual: Option<u64>,
    missing_fact: String,
) -> AssertionResultEvidence {
    let builder = ResultBuilder::new(id, description, None);
    let Some(actual) = actual else {
        return builder.missing_fact(missing_fact).build();
    };
    if actual == expected {
        builder
            .passed(format!("expected {expected}, observed {actual}"))
            .build()
    } else {
        builder
            .failed(format!("expected {expected}, observed {actual}"))
            .build()
    }
}

fn evaluate_no_duplicate_prs(
    id: String,
    description: String,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let mut builder = ResultBuilder::new(id, description, None);
    if artifact.final_state.pull_requests.is_empty() {
        return builder
            .missing_fact("run evidence has no final pull request facts")
            .build();
    }
    let implementation = artifact
        .final_state
        .pull_requests
        .iter()
        .filter(|pull_request| has_label(&pull_request.labels, "implementation"))
        .collect::<Vec<_>>();
    if implementation.is_empty() {
        return builder
            .missing_fact("run evidence has pull requests but no implementation label facts")
            .build();
    }
    if implementation
        .iter()
        .any(|pull_request| pull_request.head_branch.is_none())
    {
        return builder
            .missing_fact("implementation PR duplicate detection requires head_branch facts")
            .build();
    }

    let mut by_branch = BTreeMap::<&str, Vec<u64>>::new();
    for pull_request in implementation {
        let branch = pull_request
            .head_branch
            .as_deref()
            .expect("head_branch checked above");
        by_branch
            .entry(branch)
            .or_default()
            .push(pull_request.number);
    }
    let duplicates = by_branch
        .iter()
        .filter(|(_, numbers)| numbers.len() > 1)
        .map(|(branch, numbers)| format!("{branch}: {numbers:?}"))
        .collect::<Vec<_>>();
    if duplicates.is_empty() {
        builder = builder.passed("no implementation PR head branch has multiple PRs");
    } else {
        builder = builder.failed(format!(
            "duplicate implementation PRs found by head branch: {}",
            duplicates.join(", ")
        ));
    }
    builder.build()
}

fn count_merged_pull_requests(artifact: &RunEvidenceArtifact) -> Option<u64> {
    if artifact.final_state.pull_requests.is_empty() {
        return None;
    }
    Some(
        artifact
            .final_state
            .pull_requests
            .iter()
            .filter(|pull_request| state_is(pull_request.state.as_deref(), "merged"))
            .count() as u64,
    )
}

fn count_closed_parent_issues(artifact: &RunEvidenceArtifact) -> Option<u64> {
    if artifact.final_state.issues.is_empty() {
        return None;
    }
    Some(
        artifact
            .final_state
            .issues
            .iter()
            .filter(|issue| state_is(issue.state.as_deref(), "closed"))
            .count() as u64,
    )
}

fn count_pull_request_action_events(
    artifact: &RunEvidenceArtifact,
    event_name: &str,
    action: &str,
) -> Option<u64> {
    let observability = artifact.observability.as_ref()?;
    Some(
        observability
            .events
            .iter()
            .filter(|event| {
                event.event == event_name
                    && event
                        .fields
                        .get("action")
                        .is_some_and(|actual| actual == action)
            })
            .count() as u64,
    )
}

enum SourceIssueCandidates<'a> {
    Issues(Vec<&'a IssueStateEvidence>),
    MissingFact(String),
}

fn source_issue_candidates(artifact: &RunEvidenceArtifact) -> SourceIssueCandidates<'_> {
    if artifact.final_state.issues.is_empty() {
        return SourceIssueCandidates::MissingFact(
            "run evidence has no final issue facts".to_string(),
        );
    }
    if let Some(issue_number) = artifact
        .provider
        .as_ref()
        .and_then(|provider| provider.issue_number)
    {
        let matches = artifact
            .final_state
            .issues
            .iter()
            .filter(|issue| issue.number == issue_number)
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return SourceIssueCandidates::Issues(matches);
        }
    }
    let named = artifact
        .final_state
        .issues
        .iter()
        .filter(|issue| {
            issue.id.as_deref().is_some_and(|id| {
                matches!(id, "intake" | "source" | "parent" | "parent_issue")
                    || id.starts_with("source:")
            })
        })
        .collect::<Vec<_>>();
    if !named.is_empty() {
        return SourceIssueCandidates::Issues(named);
    }
    if artifact.final_state.issues.len() == 1 {
        return SourceIssueCandidates::Issues(artifact.final_state.issues.iter().collect());
    }
    SourceIssueCandidates::MissingFact(
        "run evidence has multiple issues but no source/parent issue id or provider issue number fact"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::super::model::{
        ArtifactCollections, CiStateEvidence, FinalStateEvidence, ObservabilityEvidence,
        RUN_EVIDENCE_SCHEMA, RUN_EVIDENCE_VERSION, ScenarioEvidence, StructuredEventEvidence,
        TopologyEvidence,
    };
    use super::*;

    #[test]
    fn counts_created_and_refreshed_pull_requests_from_structured_actions() {
        let artifact = artifact_with_events(vec![
            event(1, "pr.opened", "created"),
            event(2, "pr.updated", "refreshed"),
            event(3, "pr.opened", ""),
            event(4, "pr.updated", "created"),
        ]);

        assert_eq!(
            count_pull_request_action_events(&artifact, "pr.opened", "created"),
            Some(1)
        );
        assert_eq!(
            count_pull_request_action_events(&artifact, "pr.updated", "refreshed"),
            Some(1)
        );
    }

    #[test]
    fn action_counts_need_observability_facts() {
        let mut artifact = artifact_with_events(Vec::new());
        artifact.observability = None;

        assert_eq!(
            count_pull_request_action_events(&artifact, "pr.opened", "created"),
            None
        );
    }

    fn artifact_with_events(events: Vec<StructuredEventEvidence>) -> RunEvidenceArtifact {
        RunEvidenceArtifact {
            schema: RUN_EVIDENCE_SCHEMA.to_string(),
            version: RUN_EVIDENCE_VERSION,
            verdict: super::super::super::model::RunEvidenceVerdict::Passed,
            scenario: ScenarioEvidence {
                name: "implementation-pr-handoff".to_string(),
                source: "checked-in".to_string(),
                source_description: "checked-in scenario".to_string(),
                scenario_path: "scenarios/implementation-pr-handoff".to_string(),
                manifest_path: "scenarios/implementation-pr-handoff/scenario.toml".to_string(),
                feature: None,
                plan: None,
                mapping_id: None,
                mapped_scenario: Some("implementation-pr-handoff".to_string()),
                source_branch: None,
                checkout_head_sha: None,
                resolved_content_digest: Some("sha256:test".to_string()),
                runner_id: "manifest".to_string(),
                runner_selector: "runner.uses".to_string(),
                runner_selection: "runner: `manifest` selected by runner.uses".to_string(),
                tier: "live".to_string(),
                tier_description: "live".to_string(),
                topology: TopologyEvidence::default(),
            },
            binary: None,
            execution: None,
            fixtures: Vec::new(),
            final_state: FinalStateEvidence {
                issues: Vec::new(),
                pull_requests: Vec::new(),
                repositories: Vec::new(),
                ci: CiStateEvidence::default(),
            },
            convergence: None,
            provider: None,
            observability: Some(ObservabilityEvidence {
                scenario_run_id: "test-run".to_string(),
                log_format: "json".to_string(),
                rust_log: "temper=debug".to_string(),
                event_log_path: "standalone.log".to_string(),
                event_log_paths: vec!["standalone.log".to_string()],
                captured_events: events.len(),
                events,
            }),
            artifacts: ArtifactCollections::default(),
            evidence_lines: Vec::new(),
            stimuli: Vec::new(),
            limitations: Vec::new(),
            follow_up_intent: None,
            assertions: None,
        }
    }

    fn event(sequence: usize, event: &str, action: &str) -> StructuredEventEvidence {
        let mut fields = BTreeMap::new();
        fields.insert("action".to_string(), action.to_string());
        StructuredEventEvidence {
            sequence,
            event: event.to_string(),
            service: Some("engine".to_string()),
            target: Some("temper::engine".to_string()),
            fields,
        }
    }
}
