// SPDX-License-Identifier: MPL-2.0

use toml::Value;

use super::super::model::{AssertionResultEvidence, CiHeadEvidence, RunEvidenceArtifact};
use super::support::{ResultBuilder, required_assertion, same_normalized};

const SUPPORTED_FIELDS: &[&str] = &[
    "id",
    "required",
    "initial_head",
    "repaired_head",
    "heads_differ",
    "published_proofs",
    "stale_failure_absent_from_repaired",
    "completed_before_poll_cadence",
];

pub(super) fn evaluate(
    expect: &toml::Table,
    artifact: &RunEvidenceArtifact,
    results: &mut Vec<AssertionResultEvidence>,
) {
    let Some(value) = expect.get("ci_repair") else {
        return;
    };
    let Some(table) = value.as_table() else {
        results.push(
            ResultBuilder::new(
                "expect.ci_repair".to_string(),
                "Exact-head CI repair evidence is well-formed.".to_string(),
                None,
            )
            .failed("expect.ci_repair must be a table")
            .build(),
        );
        return;
    };
    results.push(evaluate_one(table, artifact));
}

fn evaluate_one(table: &toml::Table, artifact: &RunEvidenceArtifact) -> AssertionResultEvidence {
    let id = table
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("expect.ci_repair")
        .to_string();
    let mut builder = ResultBuilder::new(
        id,
        "One PR advances from a retained verified-failure head to a clean repaired head before broad polling."
            .to_string(),
        Some("pull_request:implementation".to_string()),
    );
    match required_assertion(table) {
        Ok(required) => builder = builder.required(required),
        Err(message) => return builder.failed(message).build(),
    }
    for field in table.keys() {
        if !SUPPORTED_FIELDS.contains(&field.as_str()) {
            builder = builder.failed(format!("unsupported CI repair expectation field `{field}`"));
        }
    }

    let Some(initial_phase) = table.get("initial_head").and_then(Value::as_str) else {
        return builder
            .failed("ci_repair.initial_head must be a string")
            .build();
    };
    let Some(repaired_phase) = table.get("repaired_head").and_then(Value::as_str) else {
        return builder
            .failed("ci_repair.repaired_head must be a string")
            .build();
    };
    let initial = match select_head(artifact, initial_phase) {
        Ok(head) => head,
        Err(message) => return builder.missing_fact(message).build(),
    };
    let repaired = match select_head(artifact, repaired_phase) {
        Ok(head) => head,
        Err(message) => return builder.missing_fact(message).build(),
    };
    builder = builder.passed(format!(
        "retained `{initial_phase}` head `{}` and `{repaired_phase}` head `{}`",
        initial.head_sha, repaired.head_sha
    ));

    if initial.observed_after_ms <= repaired.observed_after_ms {
        builder = builder.passed(format!(
            "initial head was observed at {}ms before repaired head at {}ms",
            initial.observed_after_ms, repaired.observed_after_ms
        ));
    } else {
        builder = builder.failed("repaired head was observed before the initial failed head");
    }

    if let Some(value) = table.get("heads_differ") {
        match value.as_bool() {
            Some(expected) if (initial.head_sha != repaired.head_sha) == expected => {
                builder = builder.passed(format!("head difference matched {expected}"));
            }
            Some(expected) => {
                builder = builder.failed(format!(
                    "expected heads_differ {expected}, initial and repaired heads were `{}` and `{}`",
                    initial.head_sha, repaired.head_sha
                ));
            }
            None => builder = builder.failed("ci_repair.heads_differ must be a boolean"),
        }
    }

    if let Some(value) = table.get("published_proofs") {
        let Some(expected) = value
            .as_integer()
            .and_then(|value| usize::try_from(value).ok())
        else {
            return builder
                .failed("ci_repair.published_proofs must be a non-negative integer")
                .build();
        };
        let Some(service) = artifact.final_state.ci.failure_evidence.as_ref() else {
            return builder
                .missing_fact("run evidence has no protected-workflow publication record")
                .build();
        };
        if service.published_proofs == expected {
            builder = builder.passed(format!(
                "protected workflow published exactly {expected} proof(s)"
            ));
        } else {
            builder = builder.failed(format!(
                "expected {expected} published proof(s), observed {}",
                service.published_proofs
            ));
        }
    }

    if table.contains_key("stale_failure_absent_from_repaired") {
        match table
            .get("stale_failure_absent_from_repaired")
            .and_then(Value::as_bool)
        {
            Some(expected) => {
                let clean = !repaired.jobs.is_empty()
                    && repaired.jobs.iter().all(|job| {
                        job.commit_sha.as_deref() == Some(repaired.head_sha.as_str())
                            && job.commit_sha.as_deref() != Some(initial.head_sha.as_str())
                            && job.verified_failure.is_none()
                    });
                if clean == expected {
                    builder = builder.passed(format!(
                        "stale failure absence from repaired head matched {expected}"
                    ));
                } else {
                    builder = builder.failed(format!(
                        "expected stale_failure_absent_from_repaired {expected}, repaired jobs were {:?}",
                        repaired.jobs
                    ));
                }
            }
            None => {
                builder =
                    builder.failed("ci_repair.stale_failure_absent_from_repaired must be a boolean")
            }
        }
    }

    if table.contains_key("completed_before_poll_cadence") {
        match table
            .get("completed_before_poll_cadence")
            .and_then(Value::as_bool)
        {
            Some(expected) => {
                let Some(configuration) = artifact.effective_configuration.as_ref() else {
                    return builder
                        .missing_fact("run evidence has no effective broad poll cadence")
                        .build();
                };
                let Some(convergence) = artifact.convergence.as_ref() else {
                    return builder
                        .missing_fact("run evidence has no convergence timing")
                        .build();
                };
                let Some(total_ms) = convergence.total_elapsed_ms else {
                    return builder
                        .missing_fact("run evidence has no total elapsed timing")
                        .build();
                };
                let broad_ms = configuration.poll_cadence_secs.saturating_mul(1_000);
                let before = total_ms < broad_ms && repaired.observed_after_ms < broad_ms;
                if before == expected {
                    builder = builder.passed(format!(
                        "completion-before-broad-poll matched {expected}: total={total_ms}ms broad={broad_ms}ms"
                    ));
                } else {
                    builder = builder.failed(format!(
                        "expected completed_before_poll_cadence {expected}, total={total_ms}ms broad={broad_ms}ms"
                    ));
                }
            }
            None => {
                builder =
                    builder.failed("ci_repair.completed_before_poll_cadence must be a boolean")
            }
        }
    }

    builder.build()
}

fn select_head<'a>(
    artifact: &'a RunEvidenceArtifact,
    phase: &str,
) -> Result<&'a CiHeadEvidence, String> {
    let matching = artifact
        .final_state
        .ci
        .heads
        .iter()
        .filter(|head| same_normalized(&head.phase, phase))
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [head] => Ok(*head),
        _ => Err(format!(
            "run evidence requires exactly one `{phase}` CI head, found {}",
            matching.len()
        )),
    }
}
