// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{
    CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION, MAX_CI_FAILURE_PROOF_IDENTITY_BYTES,
};
use toml::Value;

use super::super::model::{
    AssertionResultEvidence, CiJobEvidence, RunEvidenceArtifact, VerifiedFailureProofEvidence,
};
use super::support::{
    ResultBuilder, SelectionProblem, required_assertion, same_normalized, select_pull_request,
};

const SUPPORTED_FIELDS: &[&str] = &[
    "id",
    "required",
    "pull_request",
    "head",
    "job_name",
    "exactly",
    "category",
    "repository_id",
    "pull_request_id",
    "commit_sha",
    "run_id",
    "job_id",
    "attempt",
    "task_id",
    "producer_id",
    "issuer_id",
    "verification",
];
const EXPECTED_PROOF_FIELDS: &[&str] = &[
    "category",
    "repository_id",
    "pull_request_id",
    "commit_sha",
    "run_id",
    "job_id",
    "attempt",
    "task_id",
    "producer_id",
    "issuer_id",
    "verification",
];

pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("verified_failure_proof") else {
        return;
    };
    let Some(expectations) = value.as_array() else {
        results.push(
            ResultBuilder::new(
                "expect.verified_failure_proof".to_string(),
                "Verified ordinary-failure proof expectations are well-formed.".to_string(),
                None,
            )
            .failed("expect.verified_failure_proof must be an array of tables")
            .build(),
        );
        return;
    };
    for (index, value) in expectations.iter().enumerate() {
        let Some(expectation) = value.as_table() else {
            results.push(
                ResultBuilder::new(
                    format!("expect.verified_failure_proof[{index}]"),
                    "Verified ordinary-failure proof expectation is well-formed.".to_string(),
                    None,
                )
                .failed("expect.verified_failure_proof entries must be tables")
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
        .unwrap_or_else(|| format!("expect.verified_failure_proof[{index}]"));
    let pull_id = expectation.get("pull_request").and_then(Value::as_str);
    let mut builder = ResultBuilder::new(
        id,
        "A verified ordinary CI-failure proof has complete exact-head execution provenance."
            .to_string(),
        pull_id.map(|id| format!("pull_request:{id}")),
    );
    match required_assertion(expectation) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    for field in expectation.keys() {
        if !SUPPORTED_FIELDS.contains(&field.as_str()) {
            builder = builder.failed(format!(
                "unsupported verified failure proof expectation field `{field}`"
            ));
        }
    }

    let Some(job_name) = expectation
        .get("job_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return builder
            .failed("verified failure proof assertion requires a non-empty job_name")
            .build();
    };
    let Some(exactly) = expectation
        .get("exactly")
        .and_then(Value::as_integer)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
    else {
        return builder
            .failed("verified failure proof assertion requires a positive integer exactly")
            .build();
    };
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
    let selected_head = expectation.get("head").and_then(Value::as_str);
    let (jobs, proof_head) = if let Some(phase) = selected_head {
        let matching = artifact
            .final_state
            .ci
            .heads
            .iter()
            .filter(|head| same_normalized(&head.phase, phase))
            .collect::<Vec<_>>();
        let [head] = matching.as_slice() else {
            return builder
                .missing_fact(format!(
                    "run evidence requires exactly one `{phase}` CI head, found {}",
                    matching.len()
                ))
                .build();
        };
        (
            head.jobs
                .iter()
                .filter(|job| same_normalized(&job.name, job_name))
                .collect::<Vec<_>>(),
            Some(head.head_sha.as_str()),
        )
    } else {
        (
            jobs_for_pull(artifact, pull.number)
                .into_iter()
                .filter(|job| same_normalized(&job.name, job_name))
                .collect::<Vec<_>>(),
            pull.head_sha.as_deref(),
        )
    };
    if jobs.is_empty() {
        return builder
            .missing_fact(format!(
                "run evidence has no `{job_name}` CI job for pull request #{}",
                pull.number
            ))
            .build();
    }
    if jobs.len() == exactly {
        builder = builder.passed(format!("selected exactly {exactly} `{job_name}` CI job(s)"));
    } else {
        builder = builder.failed(format!(
            "expected exactly {exactly} `{job_name}` CI job(s), observed {}",
            jobs.len()
        ));
    }

    for (job_index, job) in jobs.into_iter().enumerate() {
        let Some(proof) = job.verified_failure.as_ref() else {
            builder = builder.missing_fact(format!(
                "`{job_name}` CI job {} has no verified failure proof",
                job_index + 1
            ));
            continue;
        };
        builder = evaluate_complete_proof(builder, proof, job, proof_head);
        builder = evaluate_expected_fields(builder, expectation, proof);
    }
    builder.build()
}

fn jobs_for_pull(artifact: &RunEvidenceArtifact, pull_number: u64) -> Vec<&CiJobEvidence> {
    let matching = artifact
        .final_state
        .ci
        .jobs
        .iter()
        .filter(|job| job.pull_request_number == Some(pull_number))
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

fn evaluate_complete_proof(
    mut builder: ResultBuilder,
    proof: &VerifiedFailureProofEvidence,
    job: &CiJobEvidence,
    pull_head: Option<&str>,
) -> ResultBuilder {
    let required = [
        ("category", proof.category.as_str()),
        ("repository_id", proof.repository_id.as_str()),
        ("commit_sha", proof.commit_sha.as_str()),
        ("run_id", proof.run_id.as_str()),
        ("job_id", proof.job_id.as_str()),
        ("attempt", proof.attempt.as_str()),
        ("producer_id", proof.producer_id.as_str()),
        ("issuer_id", proof.issuer_id.as_str()),
        ("verification", proof.verification.as_str()),
        ("created_at", proof.created_at.as_str()),
        ("expires_at", proof.expires_at.as_str()),
    ];
    let missing = required
        .into_iter()
        .filter_map(|(field, value)| value.trim().is_empty().then_some(field))
        .chain(
            proof
                .pull_request_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .is_none()
                .then_some("pull_request_id"),
        )
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return builder.missing_fact(format!(
            "verified failure proof is missing required provenance coordinates {:?}",
            missing
        ));
    }
    if proof.schema_version != CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION {
        builder = builder.failed(format!(
            "verified failure proof schema version {} is unsupported",
            proof.schema_version
        ));
    }
    if !matches!(proof.category.as_str(), "source" | "build" | "test") {
        builder = builder.failed(format!(
            "verified failure proof category `{}` is not ordinary",
            proof.category
        ));
    }
    for (field, value) in [
        ("repository_id", proof.repository_id.as_str()),
        (
            "pull_request_id",
            proof.pull_request_id.as_deref().unwrap_or_default(),
        ),
        ("run_id", proof.run_id.as_str()),
        ("job_id", proof.job_id.as_str()),
        ("attempt", proof.attempt.as_str()),
        ("producer_id", proof.producer_id.as_str()),
        ("issuer_id", proof.issuer_id.as_str()),
    ] {
        if value.len() > MAX_CI_FAILURE_PROOF_IDENTITY_BYTES {
            builder = builder.failed(format!(
                "verified failure proof {field} exceeds the {MAX_CI_FAILURE_PROOF_IDENTITY_BYTES}-byte identity bound"
            ));
        }
    }
    if proof.verification != "protected_producer" {
        builder = builder.failed(format!(
            "verified failure proof has unsupported verification `{}`",
            proof.verification
        ));
    }
    let Some(pull_head) = pull_head.filter(|value| !value.trim().is_empty()) else {
        return builder.missing_fact("pull request head_sha is missing for proof correlation");
    };
    if proof.commit_sha != pull_head || job.commit_sha.as_deref() != Some(proof.commit_sha.as_str())
    {
        builder = builder
            .failed("verified failure proof commit does not match the selected PR and CI job head");
    } else {
        builder = builder.passed("verified failure proof commit matches the exact PR/job head");
    }
    if job.provider_run_id.as_deref() != Some(proof.run_id.as_str())
        || job.provider_attempt.as_deref() != Some(proof.attempt.as_str())
    {
        builder = builder
            .failed("verified failure proof run/attempt does not match the enclosing CI job");
    } else {
        builder =
            builder.passed("verified failure proof run and attempt match the enclosing CI job");
    }
    builder
}

fn evaluate_expected_fields(
    mut builder: ResultBuilder,
    expectation: &toml::Table,
    proof: &VerifiedFailureProofEvidence,
) -> ResultBuilder {
    for field in EXPECTED_PROOF_FIELDS {
        let Some(value) = expectation.get(*field) else {
            continue;
        };
        let Some(expected) = value.as_str() else {
            builder = builder.failed(format!("{field} must be a string"));
            continue;
        };
        let actual = match *field {
            "category" => Some(proof.category.as_str()),
            "repository_id" => Some(proof.repository_id.as_str()),
            "pull_request_id" => proof.pull_request_id.as_deref(),
            "commit_sha" => Some(proof.commit_sha.as_str()),
            "run_id" => Some(proof.run_id.as_str()),
            "job_id" => Some(proof.job_id.as_str()),
            "attempt" => Some(proof.attempt.as_str()),
            "task_id" => proof.task_id.as_deref(),
            "producer_id" => Some(proof.producer_id.as_str()),
            "issuer_id" => Some(proof.issuer_id.as_str()),
            "verification" => Some(proof.verification.as_str()),
            _ => unreachable!("closed proof expectation fields"),
        };
        let Some(actual) = actual else {
            builder = builder.missing_fact(format!(
                "verified failure proof has no `{field}` coordinate"
            ));
            continue;
        };
        let matches = if matches!(*field, "category" | "verification") {
            same_normalized(actual, expected)
        } else {
            actual == expected
        };
        if matches {
            builder = builder.passed(format!("verified failure proof {field} matched"));
        } else {
            builder = builder.failed(format!(
                "expected verified failure proof {field} `{expected}`, observed `{actual}`"
            ));
        }
    }
    builder
}
