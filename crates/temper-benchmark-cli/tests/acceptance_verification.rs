// SPDX-License-Identifier: MPL-2.0

#[path = "support/acceptance.rs"]
mod support;

use std::fs;
use std::process::Command;

use support::{EvidenceFixture, MatrixConfig};
use temper_benchmark_cli::{AcceptanceGateV1, write_benchmark_acceptance};

fn gate(result: &temper_benchmark_cli::BenchmarkAcceptanceV1, gate: AcceptanceGateV1) -> bool {
    result
        .gates
        .iter()
        .find(|result| result.gate == gate)
        .unwrap()
        .passed
}

#[test]
fn aggregate_relevance_fails_at_49_percent_and_passes_at_50_percent() {
    let failing = EvidenceFixture::new(MatrixConfig {
        enabled_relevance: [10, 10, 10, 10, 9],
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&failing, AcceptanceGateV1::MatrixAggregateRelevance));
    let observation = failing
        .gates
        .iter()
        .find(|gate| gate.gate == AcceptanceGateV1::MatrixAggregateRelevance)
        .unwrap()
        .observation
        .as_ref()
        .unwrap();
    assert_eq!(
        (observation.numerator, observation.denominator),
        (Some(49), Some(100))
    );

    let passing = EvidenceFixture::new(MatrixConfig::default()).verify();
    assert!(gate(&passing, AcceptanceGateV1::MatrixAggregateRelevance));
}

#[test]
fn incomplete_relevance_and_incorrect_enabled_runs_fail_closed() {
    let incomplete = EvidenceFixture::new(MatrixConfig {
        incomplete_relevance: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(
        &incomplete,
        AcceptanceGateV1::EnabledDecisionEvidence
    ));
    assert!(!gate(
        &incomplete,
        AcceptanceGateV1::MatrixAggregateRelevance
    ));

    let incorrect = EvidenceFixture::new(MatrixConfig {
        incorrect_enabled: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&incorrect, AcceptanceGateV1::EnabledTaskCorrectness));
}

#[test]
fn duplicate_trials_and_execution_identity_drift_are_rejected() {
    let duplicate = EvidenceFixture::new(MatrixConfig {
        duplicate_trial: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&duplicate, AcceptanceGateV1::UniqueTrials));

    let drift = EvidenceFixture::new(MatrixConfig {
        identity_drift: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&drift, AcceptanceGateV1::ExecutionIdentity));
}

#[test]
fn unavailable_retries_and_incomplete_control_classification_fail() {
    let retry = EvidenceFixture::new(MatrixConfig {
        unavailable_retry: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&retry, AcceptanceGateV1::UnavailableRetry));

    let incomplete = EvidenceFixture::new(MatrixConfig {
        incomplete_classification: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&incomplete, AcceptanceGateV1::ControlClassification));
}

#[test]
fn missing_improvement_samples_are_not_dropped_or_converted_to_zero() {
    let result = EvidenceFixture::new(MatrixConfig {
        missing_improvement_sample: true,
        ..MatrixConfig::default()
    })
    .verify();
    let improvement = result
        .gates
        .iter()
        .find(|gate| gate.gate == AcceptanceGateV1::Improvement)
        .unwrap();
    assert!(!improvement.passed);
    assert_eq!(improvement.observation, None);
}

#[test]
fn exact_patch_and_host_validation_are_independent_gates() {
    let exact = EvidenceFixture::new(MatrixConfig {
        fail_exact_patch: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(!gate(&exact, AcceptanceGateV1::EnabledExactPatch));
    assert!(gate(&exact, AcceptanceGateV1::EnabledHostValidation));

    let host = EvidenceFixture::new(MatrixConfig {
        fail_host_validation: true,
        ..MatrixConfig::default()
    })
    .verify();
    assert!(gate(&host, AcceptanceGateV1::EnabledExactPatch));
    assert!(!gate(&host, AcceptanceGateV1::EnabledHostValidation));
}

#[test]
fn privacy_scan_fails_without_copying_the_matched_content() {
    let fixture = EvidenceFixture::new(MatrixConfig::default());
    fs::write(
        fixture.options.enabled.join("repetitions/001/run.md"),
        "MCP-FIXTURE-SECRET",
    )
    .unwrap();
    let result = fixture.verify();
    assert!(!gate(&result, AcceptanceGateV1::DurablePrivacy));
    write_benchmark_acceptance(&result, &fixture.options.output_dir).unwrap();
    let artifact = fs::read_to_string(fixture.options.output_dir.join("acceptance.json")).unwrap();
    assert!(!artifact.contains("MCP-FIXTURE-SECRET"));
}

#[test]
fn verify_cli_writes_failed_artifact_and_returns_nonzero() {
    let fixture = EvidenceFixture::new(MatrixConfig {
        enabled_relevance: [10, 10, 10, 10, 9],
        ..MatrixConfig::default()
    });
    let output = Command::new(env!("CARGO_BIN_EXE_temper-benchmark"))
        .arg("verify")
        .arg("--benchmark")
        .arg(&fixture.options.benchmark)
        .arg("--candidate-commit")
        .arg(&fixture.options.candidate_commit)
        .arg("--smoke")
        .arg(&fixture.options.smoke)
        .arg("--enabled")
        .arg(&fixture.options.enabled)
        .arg("--disabled")
        .arg(&fixture.options.disabled)
        .arg("--unavailable")
        .arg(&fixture.options.unavailable)
        .arg("--output-dir")
        .arg(&fixture.options.output_dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("acceptance failed"));
    let artifact: temper_benchmark_cli::BenchmarkAcceptanceV1 = serde_json::from_slice(
        &fs::read(fixture.options.output_dir.join("acceptance.json")).unwrap(),
    )
    .unwrap();
    assert!(!artifact.passed);
}

#[test]
fn passing_five_by_three_input_writes_privacy_safe_artifact() {
    let fixture = EvidenceFixture::new(MatrixConfig::default());
    let result = fixture.verify();
    assert!(result.passed, "{:#?}", result.gates);
    write_benchmark_acceptance(&result, &fixture.options.output_dir).unwrap();
    let bytes = fs::read(fixture.options.output_dir.join("acceptance.json")).unwrap();
    let artifact: temper_benchmark_cli::BenchmarkAcceptanceV1 =
        serde_json::from_slice(&bytes).unwrap();
    assert!(artifact.passed);
    let mut inconsistent = serde_json::to_value(&artifact).unwrap();
    inconsistent["gates"][0]["passed"] = serde_json::json!(false);
    assert!(
        serde_json::from_value::<temper_benchmark_cli::BenchmarkAcceptanceV1>(inconsistent)
            .is_err()
    );
    let text = String::from_utf8(bytes).unwrap();
    for forbidden in [
        "repo/src",
        "cargo test",
        "provider output",
        "trace.export",
        "Authorization: Bearer",
    ] {
        assert!(!text.contains(forbidden), "artifact copied {forbidden:?}");
    }
}
