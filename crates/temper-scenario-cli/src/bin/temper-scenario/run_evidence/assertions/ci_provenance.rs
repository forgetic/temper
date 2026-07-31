// SPDX-License-Identifier: MPL-2.0

use std::collections::BTreeSet;

use toml::Value;

use super::super::model::{AssertionResultEvidence, CiJobEvidence, RunEvidenceArtifact};
#[path = "ci_provenance/requests.rs"]
mod requests;

use super::support::{
    ResultBuilder, SelectionProblem, required_assertion, same_normalized, select_pull_request,
};

const SUPPORTED_FIELDS: &[&str] = &[
    "id",
    "required",
    "pull_request",
    "matching_provider_run",
    "materialized_jobs",
    "job_count",
    "provider_run_count",
    "stable_identities",
    "exact_head",
    "job_outcomes",
    "required_requests",
    "forbidden_requests",
];
pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("ci_provenance") else {
        return;
    };
    let Some(expectations) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.ci_provenance".to_string(),
                "CI provenance expectations are well-formed.".to_string(),
                None,
            )
            .failed("expect.ci_provenance must be an array of tables")
            .build(),
        );
        return;
    };

    for (index, value) in expectations.iter().enumerate() {
        let Some(expectation) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.ci_provenance[{index}]"),
                    "CI provenance expectation is well-formed.".to_string(),
                    None,
                )
                .failed("expect.ci_provenance entries must be tables")
                .build(),
            );
            continue;
        };
        results.push(evaluate_one(index, expectation, artifact));
    }
}

fn evaluate_one(
    index: usize,
    expectation: &toml::Table,
    artifact: &RunEvidenceArtifact,
) -> AssertionResultEvidence {
    let id = expectation
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("expect.ci_provenance[{index}]"));
    let pull_id = expectation.get("pull_request").and_then(Value::as_str);
    let artifact_name = pull_id.map(|id| format!("pull_request:{id}"));
    let mut builder = ResultBuilder::new(
        id,
        "Structured CI identity and provider request provenance match the manifest.".to_string(),
        artifact_name,
    );
    match required_assertion(expectation) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    for field in expectation.keys() {
        if !SUPPORTED_FIELDS.contains(&field.as_str()) {
            builder = builder.failed(format!(
                "unsupported CI provenance expectation field `{field}`"
            ));
        }
    }

    let selected = match select_pull_request(&artifact.final_state.pull_requests, pull_id) {
        Ok(selected) => selected,
        Err(SelectionProblem::Failed(message)) => return builder.failed(message).build(),
        Err(SelectionProblem::MissingFact(message)) => {
            return builder.missing_fact(message).build();
        }
    };
    if let Some(note) = selected.note {
        builder = builder.passed(note);
    }
    let pull = selected.pull_request;
    let jobs = jobs_for_pull_request(artifact, pull.number);
    let mut evaluated = 0usize;

    if let Some(value) = expectation.get("matching_provider_run") {
        evaluated += 1;
        builder = match value.as_bool() {
            Some(expected) => evaluate_matching_run(builder, expected, artifact),
            None => builder.failed("matching_provider_run must be a boolean"),
        };
    }
    if let Some(value) = expectation.get("materialized_jobs") {
        evaluated += 1;
        builder = match value.as_bool() {
            Some(expected) => evaluate_materialized_jobs(builder, expected, artifact, &jobs),
            None => builder.failed("materialized_jobs must be a boolean"),
        };
    }
    if let Some(value) = expectation.get("job_count") {
        evaluated += 1;
        builder = evaluate_count(builder, "job_count", value, jobs.len(), artifact);
    }
    if let Some(value) = expectation.get("provider_run_count") {
        evaluated += 1;
        builder = evaluate_provider_run_count(builder, value, &jobs);
    }
    if let Some(value) = expectation.get("stable_identities") {
        evaluated += 1;
        builder = match value.as_bool() {
            Some(expected) => evaluate_stable_identities(builder, expected, artifact, pull.number),
            None => builder.failed("stable_identities must be a boolean"),
        };
    }
    if let Some(value) = expectation.get("exact_head") {
        evaluated += 1;
        builder = match value.as_bool() {
            Some(expected) => {
                evaluate_exact_head(builder, expected, pull.head_sha.as_deref(), &jobs)
            }
            None => builder.failed("exact_head must be a boolean"),
        };
    }
    if let Some(value) = expectation.get("job_outcomes") {
        evaluated += 1;
        builder = evaluate_job_outcomes(builder, value, &jobs);
    }

    let provider_runs = provider_run_ids(&jobs);
    if let Some(value) = expectation.get("required_requests") {
        evaluated += 1;
        builder = requests::evaluate_requests(builder, value, false, artifact, &provider_runs);
    }
    if let Some(value) = expectation.get("forbidden_requests") {
        evaluated += 1;
        builder = requests::evaluate_requests(builder, value, true, artifact, &provider_runs);
    }

    if evaluated == 0 {
        builder = builder.unsupported("CI provenance expectation contains no assertion fields");
    }
    builder.build()
}

fn jobs_for_pull_request(
    artifact: &RunEvidenceArtifact,
    pull_request_number: u64,
) -> Vec<&CiJobEvidence> {
    let matching = artifact
        .final_state
        .ci
        .jobs
        .iter()
        .filter(|job| job.pull_request_number == Some(pull_request_number))
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        return matching;
    }
    if artifact.final_state.pull_requests.len() == 1
        && artifact
            .final_state
            .ci
            .jobs
            .iter()
            .all(|job| job.pull_request_number.is_none())
    {
        return artifact.final_state.ci.jobs.iter().collect();
    }
    Vec::new()
}

fn observation_jobs_for_pull_request<'a>(
    artifact: &RunEvidenceArtifact,
    observation: &'a super::super::model::CiObservationEvidence,
    pull_request_number: u64,
) -> Vec<&'a CiJobEvidence> {
    let matching = observation
        .jobs
        .iter()
        .filter(|job| job.pull_request_number == Some(pull_request_number))
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        return matching;
    }
    if artifact.final_state.pull_requests.len() == 1
        && observation
            .jobs
            .iter()
            .all(|job| job.pull_request_number.is_none())
    {
        return observation.jobs.iter().collect();
    }
    Vec::new()
}

fn evaluate_matching_run(
    mut builder: ResultBuilder,
    expected: bool,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let observations = &artifact.final_state.ci.observations;
    if observations.is_empty() {
        return builder.missing_fact("run evidence has no CI provider-run observations");
    }
    if observations
        .iter()
        .any(|observation| observation.matching_provider_run.is_none())
    {
        return builder.missing_fact("CI observations do not state whether a provider run matched");
    }
    let actual = observations
        .iter()
        .all(|observation| observation.matching_provider_run == Some(true));
    if actual == expected {
        builder = builder.passed(format!(
            "matching provider run presence was {actual} across {} observation(s)",
            observations.len()
        ));
    } else {
        builder = builder.failed(format!(
            "expected matching provider run presence {expected}, observed {actual}"
        ));
    }
    builder
}

fn evaluate_materialized_jobs(
    builder: ResultBuilder,
    expected: bool,
    artifact: &RunEvidenceArtifact,
    jobs: &[&CiJobEvidence],
) -> ResultBuilder {
    if artifact.final_state.ci.completed_jobs.is_none() && jobs.is_empty() {
        return builder.missing_fact("run evidence has no materialized CI job fact");
    }
    let actual = !jobs.is_empty();
    if actual == expected {
        builder.passed(format!("materialized CI jobs presence was {actual}"))
    } else {
        builder.failed(format!(
            "expected materialized CI jobs presence {expected}, observed {actual}"
        ))
    }
}

fn evaluate_count(
    builder: ResultBuilder,
    field: &str,
    value: &Value,
    actual: usize,
    artifact: &RunEvidenceArtifact,
) -> ResultBuilder {
    let Some(expected) = value.as_integer().filter(|count| *count >= 0) else {
        return builder.failed(format!("{field} must be a non-negative integer"));
    };
    if artifact.final_state.ci.completed_jobs.is_none() && artifact.final_state.ci.jobs.is_empty() {
        return builder.missing_fact("run evidence has no CI job count fact");
    }
    if actual == expected as usize {
        builder.passed(format!("{field} matched {expected}"))
    } else {
        builder.failed(format!("expected {field} {expected}, observed {actual}"))
    }
}

fn provider_run_ids(jobs: &[&CiJobEvidence]) -> BTreeSet<String> {
    jobs.iter()
        .filter_map(|job| nonempty(job.provider_run_id.as_deref()))
        .map(str::to_string)
        .collect()
}

fn evaluate_provider_run_count(
    builder: ResultBuilder,
    value: &Value,
    jobs: &[&CiJobEvidence],
) -> ResultBuilder {
    let Some(expected) = value.as_integer().filter(|count| *count >= 0) else {
        return builder.failed("provider_run_count must be a non-negative integer");
    };
    if jobs.is_empty() {
        return builder.missing_fact("run evidence has no CI jobs for provider-run identity");
    }
    if jobs
        .iter()
        .any(|job| nonempty(job.provider_run_id.as_deref()).is_none())
    {
        return builder.missing_fact("one or more CI jobs lack provider_run_id");
    }
    let actual = provider_run_ids(jobs).len();
    if actual == expected as usize {
        builder.passed(format!("provider_run_count matched {expected}"))
    } else {
        builder.failed(format!(
            "expected provider_run_count {expected}, observed {actual}"
        ))
    }
}

fn evaluate_stable_identities(
    builder: ResultBuilder,
    expected: bool,
    artifact: &RunEvidenceArtifact,
    pull_request_number: u64,
) -> ResultBuilder {
    let observations = &artifact.final_state.ci.observations;
    if observations.len() < 2 {
        return builder.missing_fact(format!(
            "stable CI identity requires at least 2 observations, found {}",
            observations.len()
        ));
    }
    let mut sets = Vec::new();
    for (index, observation) in observations.iter().enumerate() {
        let jobs = observation_jobs_for_pull_request(artifact, observation, pull_request_number);
        if jobs.is_empty() {
            return builder.missing_fact(format!(
                "CI observation {} has no jobs for pull request #{pull_request_number}",
                index + 1
            ));
        }
        let Some(identity) = complete_identity_set(&jobs) else {
            return builder.missing_fact(format!(
                "CI observation {} has an empty job, run, attempt, or commit identity",
                index + 1
            ));
        };
        if identity.len() != jobs.len() {
            return builder.failed(format!(
                "CI observation {} contains duplicate job identities",
                index + 1
            ));
        }
        sets.push(identity);
    }
    let stable = sets.windows(2).all(|pair| pair[0] == pair[1]);
    if stable == expected {
        builder.passed(format!(
            "CI identity stability was {stable} across {} observations",
            sets.len()
        ))
    } else {
        builder.failed(format!(
            "expected stable_identities {expected}, observed {stable}"
        ))
    }
}

fn complete_identity_set(
    jobs: &[&CiJobEvidence],
) -> Option<BTreeSet<(String, String, String, String)>> {
    jobs.iter()
        .map(|job| {
            Some((
                nonempty(job.job_id.as_deref())?.to_string(),
                nonempty(job.provider_run_id.as_deref())?.to_string(),
                nonempty(job.provider_attempt.as_deref())?.to_string(),
                nonempty(job.commit_sha.as_deref())?.to_string(),
            ))
        })
        .collect()
}

fn evaluate_exact_head(
    builder: ResultBuilder,
    expected: bool,
    head_sha: Option<&str>,
    jobs: &[&CiJobEvidence],
) -> ResultBuilder {
    let Some(head_sha) = nonempty(head_sha) else {
        return builder.missing_fact("pull request head_sha is missing");
    };
    if jobs.is_empty() {
        return builder.missing_fact("run evidence has no CI jobs to compare with the exact head");
    }
    if jobs
        .iter()
        .any(|job| nonempty(job.commit_sha.as_deref()).is_none())
    {
        return builder.missing_fact("one or more CI jobs lack commit_sha ownership");
    }
    let actual = jobs
        .iter()
        .all(|job| job.commit_sha.as_deref() == Some(head_sha));
    if actual == expected {
        builder.passed(format!(
            "exact-head CI ownership was {actual} for `{head_sha}`"
        ))
    } else {
        builder.failed(format!(
            "expected exact_head {expected} for `{head_sha}`, observed job commits {:?}",
            jobs.iter()
                .filter_map(|job| job.commit_sha.as_deref())
                .collect::<Vec<_>>()
        ))
    }
}

fn evaluate_job_outcomes(
    mut builder: ResultBuilder,
    value: &Value,
    jobs: &[&CiJobEvidence],
) -> ResultBuilder {
    let Some(rules) = value.as_array() else {
        return builder.failed("job_outcomes must be an array of tables");
    };
    for (index, value) in rules.iter().enumerate() {
        let Some(rule) = value.as_table() else {
            builder = builder.failed(format!("job_outcomes[{index}] must be a table"));
            continue;
        };
        for field in rule.keys() {
            if !matches!(
                field.as_str(),
                "name" | "status" | "conclusion" | "provider_conclusion" | "exactly"
            ) {
                builder =
                    builder.failed(format!("unsupported job_outcomes[{index}] field `{field}`"));
            }
        }
        let Some(expected) = rule
            .get("exactly")
            .and_then(Value::as_integer)
            .filter(|count| *count >= 0)
        else {
            builder = builder.failed(format!(
                "job_outcomes[{index}].exactly must be a non-negative integer"
            ));
            continue;
        };
        let actual = jobs.iter().filter(|job| job_matches(job, rule)).count();
        if actual == expected as usize {
            builder = builder.passed(format!(
                "job_outcomes[{index}] matched exactly {expected} job(s)"
            ));
        } else {
            builder = builder.failed(format!(
                "job_outcomes[{index}] expected exactly {expected} job(s), observed {actual}"
            ));
        }
    }
    builder
}

fn job_matches(job: &CiJobEvidence, rule: &toml::Table) -> bool {
    for (field, actual) in [
        ("name", Some(job.name.as_str())),
        ("status", Some(job.status.as_str())),
        ("conclusion", job.conclusion.as_deref()),
        ("provider_conclusion", job.provider_conclusion.as_deref()),
    ] {
        if let Some(expected) = rule.get(field) {
            let Some(expected) = expected.as_str() else {
                return false;
            };
            if actual.is_none_or(|actual| !same_normalized(actual, expected)) {
                return false;
            }
        }
    }
    true
}

pub(super) fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
