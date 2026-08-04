// SPDX-License-Identifier: MPL-2.0
//! Offline contract for the configured generic ordinary-failure source.

mod support;

use chrono::{Duration, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use support::{MockHttpClient, block_on, repo_id};
use temper_forge_forgejo::{
    ForgejoConfig, ForgejoFailureEvidenceConfig, ForgejoForge, HttpError, HttpResponse,
};
use temper_forge_model::{CiJobConclusion, CiJobId, CiJobQuery};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";
const ENDPOINT: &str = "https://evidence.example/v1/forgejo-failures";
const BEARER: &str = "evidence-read-secret";
const HMAC_KEY: &str = "evidence-integrity-secret";

fn forge(client: MockHttpClient) -> ForgejoForge<MockHttpClient> {
    let evidence = ForgejoFailureEvidenceConfig::new(
        ENDPOINT,
        BEARER,
        HMAC_KEY,
        "runner-host",
        ["protected-ci"],
    )
    .unwrap();
    ForgejoForge::with_client(
        ForgejoConfig::new("https://forge.example.com", "forge-token")
            .with_failure_evidence(evidence),
        client,
    )
    .with_request_provenance(8)
}

fn runs(status: &str) -> String {
    json!({
        "workflow_runs": [{
            "id": 591,
            "status": status,
            "prettyref": "#7",
            "head_sha": HEAD,
            "created_at": "2026-08-03T11:00:00Z",
            "updated_at": "2026-08-03T11:05:00Z"
        }]
    })
    .to_string()
}

fn jobs(status: &str) -> String {
    json!([{
        "id": 42,
        "run_id": 591,
        "attempt": 2,
        "task_id": 9001,
        "name": "test",
        "status": status
    }])
    .to_string()
}

fn statement() -> Value {
    let now = Utc::now();
    json!({
        "schema_version": 1,
        "category": "test",
        "repository_id": "forgejo:acme/widgets",
        "pull_request_id": "forgejo:acme/widgets:pull:7",
        "commit_sha": HEAD,
        "run_id": "591",
        "job_id": "42",
        "attempt": "2",
        "task_id": "9001",
        "producer_id": "protected-ci",
        "issuer_id": "runner-host",
        "created_at": (now - Duration::seconds(5)).to_rfc3339_opts(SecondsFormat::Secs, true),
        "expires_at": (now + Duration::minutes(5)).to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

fn signature(statement: &str, key: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
    mac.update(statement.as_bytes());
    format!("sha256={:x}", mac.finalize().into_bytes())
}

fn evidence(values: Vec<Value>) -> String {
    let records = values
        .into_iter()
        .map(|value| {
            let statement = value.to_string();
            json!({
                "hmac_sha256": signature(&statement, HMAC_KEY),
                "statement": statement
            })
        })
        .collect::<Vec<_>>();
    json!({ "schema_version": 1, "records": records }).to_string()
}

fn read_with_evidence(body: impl Into<String>) -> (temper_forge_model::CiJob, MockHttpClient) {
    let client = MockHttpClient::new();
    client.push_response(200, runs("failure"));
    client.push_response(200, jobs("failure"));
    client.push_response(200, body);
    let listed = block_on(forge(client.clone()).list_ci_jobs(
        &repo_id(),
        CiJobQuery {
            commit_sha: Some(HEAD.to_string()),
            ..Default::default()
        },
    ))
    .unwrap();
    (listed.into_iter().next().unwrap(), client)
}

#[test]
fn one_authenticated_exact_fresh_proof_strengthens_one_attempt() {
    let (job, client) = read_with_evidence(evidence(vec![statement()]));
    assert_eq!(job.conclusion, Some(CiJobConclusion::Failure));
    let proof = job.verified_failure.expect("verified provenance retained");
    assert_eq!(proof.coordinates().run_id(), "591");
    assert_eq!(proof.coordinates().job_id(), "42");
    assert_eq!(proof.coordinates().attempt(), "2");
    assert_eq!(proof.coordinates().task_id(), Some("9001"));
    assert_eq!(proof.attestation().issuer_id(), "runner-host");
    assert_eq!(proof.attestation().producer_id(), "protected-ci");

    let requests = client.recorded();
    assert_eq!(requests[2].path, ENDPOINT);
    assert_eq!(
        requests[2].query,
        [
            ("repository_id".into(), "forgejo:acme/widgets".into()),
            ("run_id".into(), "591".into())
        ]
    );
    assert!(
        requests[2].headers.iter().any(|(name, value)| {
            name == "Authorization" && value == &format!("Bearer {BEARER}")
        })
    );
}

#[test]
fn every_subject_and_provider_coordinate_mismatch_remains_unknown() {
    let cases = [
        ("repository_id", json!("forgejo:other/repository")),
        ("pull_request_id", json!("forgejo:acme/widgets:pull:8")),
        (
            "commit_sha",
            json!("ffffffffffffffffffffffffffffffffffffffff"),
        ),
        ("run_id", json!("592")),
        ("job_id", json!("43")),
        ("attempt", json!("3")),
        ("task_id", json!("9002")),
    ];
    for (field, value) in cases {
        let mut mismatched = statement();
        mismatched[field] = value;
        let (job, _) = read_with_evidence(evidence(vec![mismatched]));
        assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown), "{field}");
        assert_eq!(job.verified_failure, None, "{field}");
    }
}

#[test]
fn stale_expired_and_not_yet_valid_proofs_remain_unknown() {
    let now = Utc::now();
    for (created, expires) in [
        (now - Duration::minutes(10), now - Duration::minutes(1)),
        (now - Duration::minutes(1), now),
        (now + Duration::minutes(1), now + Duration::minutes(2)),
    ] {
        let mut value = statement();
        value["created_at"] = json!(created.to_rfc3339_opts(SecondsFormat::Secs, true));
        value["expires_at"] = json!(expires.to_rfc3339_opts(SecondsFormat::Secs, true));
        let (job, _) = read_with_evidence(evidence(vec![value]));
        assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));
        assert_eq!(job.verified_failure, None);
    }
}

#[test]
fn malformed_unauthorized_tampered_duplicate_and_conflicting_records_fail_closed() {
    let valid_statement = statement().to_string();
    let malformed_bodies = [
        "{".to_string(),
        json!({"schema_version": 2, "records": []}).to_string(),
        json!({
            "schema_version": 1,
            "records": [{"statement": valid_statement, "hmac_sha256": "sha256=00"}]
        })
        .to_string(),
    ];
    for body in malformed_bodies {
        let (job, _) = read_with_evidence(body);
        assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));
        assert_eq!(job.verified_failure, None);
    }

    for (field, value) in [
        ("issuer_id", json!("other-host")),
        ("producer_id", json!("unprotected-workflow")),
    ] {
        let mut unauthorized = statement();
        unauthorized[field] = value;
        let (job, _) = read_with_evidence(evidence(vec![unauthorized]));
        assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown), "{field}");
    }

    let duplicate = statement();
    let (job, _) = read_with_evidence(evidence(vec![duplicate.clone(), duplicate]));
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));

    let first = statement();
    let mut conflicting = first.clone();
    conflicting["category"] = json!("build");
    let (job, _) = read_with_evidence(evidence(vec![first, conflicting]));
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));

    let valid = statement();
    let mut cross_run = statement();
    cross_run["run_id"] = json!("592");
    let (job, _) = read_with_evidence(evidence(vec![valid, cross_run]));
    assert_eq!(
        job.conclusion,
        Some(CiJobConclusion::Unknown),
        "an uncorrelated sibling invalidates the acquisition batch"
    );
}

#[test]
fn absence_and_acquisition_unavailability_preserve_bare_failure() {
    let (job, _) = read_with_evidence(evidence(Vec::new()));
    assert_eq!(job.conclusion, Some(CiJobConclusion::Unknown));

    for response in [
        Ok(HttpResponse::new(404, "not found")),
        Err(HttpError::Transport(format!(
            "connection failed near {BEARER} {HMAC_KEY}"
        ))),
    ] {
        let client = MockHttpClient::new();
        client.push_response(200, runs("failure"));
        client.push_response(200, jobs("failure"));
        client.push_result(response);
        let listed = block_on(forge(client).list_ci_jobs(
            &repo_id(),
            CiJobQuery {
                commit_sha: Some(HEAD.into()),
                ..Default::default()
            },
        ))
        .expect("evidence source failure does not make Forge unavailable");
        assert_eq!(listed[0].conclusion, Some(CiJobConclusion::Unknown));
        assert_eq!(listed[0].verified_failure, None);
    }
}

#[test]
fn proof_never_overrides_non_ordinary_or_non_failure_provider_evidence() {
    for (run_status, job_status, expected) in [
        ("success", "success", CiJobConclusion::Success),
        ("cancelled", "cancelled", CiJobConclusion::Cancelled),
        ("runner_lost", "runner_lost", CiJobConclusion::RunnerLost),
        ("timed_out", "timed_out", CiJobConclusion::TimedOut),
    ] {
        let client = MockHttpClient::new();
        client.push_response(200, runs(run_status));
        client.push_response(200, jobs(job_status));
        let listed = block_on(forge(client.clone()).list_ci_jobs(
            &repo_id(),
            CiJobQuery {
                commit_sha: Some(HEAD.into()),
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(listed[0].conclusion, Some(expected));
        assert_eq!(listed[0].verified_failure, None);
        assert_eq!(
            client.call_count(),
            2,
            "ineligible evidence is not acquired"
        );
    }
}

#[test]
fn list_and_exact_opaque_get_return_identical_conclusion_and_provenance() {
    let (listed, _) = read_with_evidence(evidence(vec![statement()]));

    let client = MockHttpClient::new();
    client.push_response(200, runs("failure"));
    client.push_response(200, jobs("failure"));
    client.push_response(200, evidence(vec![statement()]));
    let found = block_on(
        forge(client).get_ci_job(&CiJobId::new("forgejo:acme/widgets:actions:591:42:2:9001")),
    )
    .unwrap()
    .unwrap();

    assert_eq!(found.id, listed.id);
    assert_eq!(found.conclusion, listed.conclusion);
    assert_eq!(found.verified_failure, listed.verified_failure);
    assert_eq!(found.provider_conclusion, listed.provider_conclusion);
}
