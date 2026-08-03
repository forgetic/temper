use super::*;
use crate::ids::CiJobId;
use crate::model::CiJobConclusion;
use serde_json::json;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn proof(category: CiOrdinaryFailureCategory) -> CiVerifiedFailureProof {
    CiVerifiedFailureProof::new(
        category,
        CiFailureProofSubject::new(
            RepositoryId::new("forgejo:acme/widgets"),
            Some(PullRequestId::new("forgejo:acme/widgets:pull:7")),
            SHA,
        )
        .unwrap(),
        CiFailureProofCoordinates::new("591", "42", "2", Some("9001")).unwrap(),
        CiFailureProofAttestation::new(
            "temper-ci-proof@v1",
            "forgejo:acme",
            CiFailureProofVerification::ProtectedProducer,
        )
        .unwrap(),
        at("2026-07-29T12:00:00Z"),
        at("2026-07-29T12:10:00Z"),
    )
    .unwrap()
}

fn completed_job() -> CiJob {
    CiJob {
        id: CiJobId::new("forgejo:acme/widgets:actions:591:42:2:9001"),
        repo_id: RepositoryId::new("forgejo:acme/widgets"),
        pull_request_id: Some(PullRequestId::new("forgejo:acme/widgets:pull:7")),
        commit_sha: SHA.into(),
        name: "test".into(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Unknown),
        provider_conclusion: Some("failure".into()),
        provider_reason: None,
        run_id: Some("591".into()),
        attempt: Some("2".into()),
        verified_failure: None,
        url: None,
        created_at: at("2026-07-29T11:59:00Z"),
        started_at: Some(at("2026-07-29T11:59:10Z")),
        completed_at: Some(at("2026-07-29T12:00:00Z")),
        updated_at: at("2026-07-29T12:00:00Z"),
    }
}

#[test]
fn every_ordinary_category_round_trips_deterministically() {
    for category in [
        CiOrdinaryFailureCategory::Source,
        CiOrdinaryFailureCategory::Build,
        CiOrdinaryFailureCategory::Test,
    ] {
        let proof = proof(category);
        let encoded = serde_json::to_string(&proof).unwrap();
        let decoded: CiVerifiedFailureProof = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, proof);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
        assert_eq!(
            decoded.schema_version(),
            CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION
        );
    }
}

#[test]
fn optional_proof_identities_are_omitted_and_round_trip() {
    let proof = CiVerifiedFailureProof::new(
        CiOrdinaryFailureCategory::Source,
        CiFailureProofSubject::new(RepositoryId::new("repo"), None, SHA).unwrap(),
        CiFailureProofCoordinates::new("run", "job", "1", None::<String>).unwrap(),
        CiFailureProofAttestation::new(
            "producer",
            "issuer",
            CiFailureProofVerification::ProtectedProducer,
        )
        .unwrap(),
        at("2026-07-29T12:00:00Z"),
        at("2026-07-29T12:01:00Z"),
    )
    .unwrap();
    let encoded = serde_json::to_string(&proof).unwrap();
    assert!(!encoded.contains("pull_request_id"));
    assert!(!encoded.contains("task_id"));
    assert_eq!(
        serde_json::from_str::<CiVerifiedFailureProof>(&encoded).unwrap(),
        proof
    );
}

#[test]
fn proof_identities_are_trimmed_bounded_and_control_free() {
    let coordinates = CiFailureProofCoordinates::new(" 591 ", "42", "2", Some("9001"))
        .expect("surrounding whitespace is sanitized");
    assert_eq!(coordinates.run_id(), "591");
    assert_eq!(coordinates.task_id(), Some("9001"));

    let subject = CiFailureProofSubject::new(
        RepositoryId::new(" forgejo:acme/widgets "),
        Some(PullRequestId::new(" forgejo:acme/widgets:pull:7 ")),
        SHA.to_ascii_uppercase(),
    )
    .expect("subject identities and SHA are canonicalized");
    assert_eq!(subject.repo_id().as_str(), "forgejo:acme/widgets");
    assert_eq!(
        subject.pull_request_id().map(PullRequestId::as_str),
        Some("forgejo:acme/widgets:pull:7")
    );
    assert_eq!(subject.commit_sha(), SHA);

    let sha256 = "A".repeat(64);
    assert_eq!(
        CiFailureProofSubject::new(RepositoryId::new("repo"), None, &sha256)
            .unwrap()
            .commit_sha(),
        sha256.to_ascii_lowercase()
    );

    let boundary = "x".repeat(MAX_CI_FAILURE_PROOF_IDENTITY_BYTES);
    let attestation = CiFailureProofAttestation::new(
        &boundary,
        &boundary,
        CiFailureProofVerification::ProtectedProducer,
    )
    .expect("the documented identity bound is inclusive");
    assert_eq!(attestation.producer_id(), boundary);
    assert_eq!(attestation.issuer_id(), boundary);

    for oversized_field in 0..4 {
        let oversized = "x".repeat(MAX_CI_FAILURE_PROOF_IDENTITY_BYTES + 1);
        let values = ["run", "job", "attempt", "task"];
        let result = CiFailureProofCoordinates::new(
            if oversized_field == 0 {
                &oversized
            } else {
                values[0]
            },
            if oversized_field == 1 {
                &oversized
            } else {
                values[1]
            },
            if oversized_field == 2 {
                &oversized
            } else {
                values[2]
            },
            Some(if oversized_field == 3 {
                oversized.as_str()
            } else {
                values[3]
            }),
        );
        assert!(matches!(
            result,
            Err(CiVerifiedFailureProofError::IdentityTooLong { .. })
        ));
    }

    let oversized = "x".repeat(MAX_CI_FAILURE_PROOF_IDENTITY_BYTES + 1);
    for result in [
        CiFailureProofSubject::new(RepositoryId::new(&oversized), None, SHA),
        CiFailureProofSubject::new(
            RepositoryId::new("repo"),
            Some(PullRequestId::new(&oversized)),
            SHA,
        ),
    ] {
        assert!(matches!(
            result,
            Err(CiVerifiedFailureProofError::IdentityTooLong { .. })
        ));
    }
    for result in [
        CiFailureProofAttestation::new(
            &oversized,
            "issuer",
            CiFailureProofVerification::ProtectedProducer,
        ),
        CiFailureProofAttestation::new(
            "producer",
            &oversized,
            CiFailureProofVerification::ProtectedProducer,
        ),
    ] {
        assert!(matches!(
            result,
            Err(CiVerifiedFailureProofError::IdentityTooLong { .. })
        ));
    }

    assert!(matches!(
        CiFailureProofAttestation::new(
            "producer\nidentity",
            "issuer",
            CiFailureProofVerification::ProtectedProducer
        ),
        Err(CiVerifiedFailureProofError::InvalidIdentity("producer id"))
    ));
    assert!(matches!(
        CiFailureProofSubject::new(RepositoryId::new("repo"), None, "abbreviated"),
        Err(CiVerifiedFailureProofError::InvalidCommitSha)
    ));
}

#[test]
fn proof_requires_every_identity_and_exact_job_coordinate() {
    for empty_field in 0..4 {
        let values = ["run", "job", "attempt", "task"];
        let result = CiFailureProofCoordinates::new(
            if empty_field == 0 { " " } else { values[0] },
            if empty_field == 1 { "" } else { values[1] },
            if empty_field == 2 { "\t" } else { values[2] },
            Some(if empty_field == 3 { "\n" } else { values[3] }),
        );
        assert!(matches!(
            result,
            Err(CiVerifiedFailureProofError::EmptyIdentity(_))
        ));
    }
    assert!(matches!(
        CiFailureProofSubject::new(RepositoryId::new(" "), None, SHA),
        Err(CiVerifiedFailureProofError::EmptyIdentity("repository id"))
    ));
    assert!(matches!(
        CiFailureProofSubject::new(RepositoryId::new("repo"), Some(PullRequestId::new("")), SHA,),
        Err(CiVerifiedFailureProofError::EmptyIdentity(
            "pull request id"
        ))
    ));
    for (producer, issuer) in [("", "issuer"), ("producer", " ")] {
        assert!(matches!(
            CiFailureProofAttestation::new(
                producer,
                issuer,
                CiFailureProofVerification::ProtectedProducer
            ),
            Err(CiVerifiedFailureProofError::EmptyIdentity(_))
        ));
    }

    let proof = proof(CiOrdinaryFailureCategory::Test);
    let job = completed_job();
    let now = at("2026-07-29T12:05:00Z");
    assert!(proof.matches_job_at(&job, "42", Some("9001"), now));
    assert!(!proof.matches_job_at(&job, "43", Some("9001"), now));
    assert!(!proof.matches_job_at(&job, "42", Some("9002"), now));

    let mut mismatch = job.clone();
    mismatch.repo_id = RepositoryId::new("forgejo:other/widgets");
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
    mismatch = job.clone();
    mismatch.pull_request_id = None;
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
    mismatch = job.clone();
    mismatch.commit_sha = "ffffffffffffffffffffffffffffffffffffffff".into();
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
    mismatch = job.clone();
    mismatch.run_id = Some("592".into());
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
    mismatch = job.clone();
    mismatch.attempt = Some("3".into());
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
    mismatch = job;
    mismatch.status = CiJobStatus::Running;
    assert!(!proof.matches_job_at(&mismatch, "42", Some("9001"), now));
}

#[test]
fn proof_validity_is_short_bounded_and_expiry_is_exclusive() {
    let proof = proof(CiOrdinaryFailureCategory::Build);
    assert!(!proof.is_fresh_at(at("2026-07-29T11:59:59Z")));
    assert!(proof.is_fresh_at(proof.created_at()));
    assert!(proof.is_fresh_at(at("2026-07-29T12:09:59Z")));
    assert!(!proof.is_fresh_at(proof.expires_at()));

    let make = |expires_at| {
        CiVerifiedFailureProof::new(
            CiOrdinaryFailureCategory::Build,
            proof.subject().clone(),
            proof.coordinates().clone(),
            proof.attestation().clone(),
            proof.created_at(),
            expires_at,
        )
    };
    assert!(matches!(
        make(proof.created_at()),
        Err(CiVerifiedFailureProofError::InvalidValidityWindow)
    ));
    assert!(
        make(proof.created_at() + chrono::Duration::seconds(MAX_CI_FAILURE_PROOF_VALIDITY_SECONDS))
            .is_ok()
    );
    assert!(matches!(
        make(
            proof.created_at()
                + chrono::Duration::seconds(MAX_CI_FAILURE_PROOF_VALIDITY_SECONDS + 1)
        ),
        Err(CiVerifiedFailureProofError::ValidityTooLong)
    ));
}

#[test]
fn malformed_versions_and_untyped_categories_are_rejected() {
    let mut value = serde_json::to_value(proof(CiOrdinaryFailureCategory::Source)).unwrap();
    value["schema_version"] = json!(0);
    assert!(serde_json::from_value::<CiVerifiedFailureProof>(value.clone()).is_err());
    value["schema_version"] = json!(CI_VERIFIED_FAILURE_PROOF_SCHEMA_VERSION + 1);
    assert!(serde_json::from_value::<CiVerifiedFailureProof>(value).is_err());

    let mut value = serde_json::to_value(proof(CiOrdinaryFailureCategory::Source)).unwrap();
    value["category"] = json!("infrastructure");
    assert!(serde_json::from_value::<CiVerifiedFailureProof>(value).is_err());
}

#[test]
fn proof_schema_and_errors_do_not_retain_forbidden_diagnostics() {
    let secret = "token=do-not-retain";
    let error = CiFailureProofAttestation::new(
        secret,
        "issuer",
        CiFailureProofVerification::ProtectedProducer,
    )
    .unwrap_err()
    .to_string();
    assert!(!error.contains(secret));

    let proof = proof(CiOrdinaryFailureCategory::Test);
    let encoded = serde_json::to_string(&proof).unwrap();
    for forbidden in [
        "signature",
        "credential",
        "secret",
        "log",
        "ui_text",
        "workflow_name",
        "repository_name",
        "scenario",
        "description",
    ] {
        assert!(!encoded.contains(forbidden));
    }

    let mut value = serde_json::to_value(proof).unwrap();
    value["signature"] = json!(secret);
    assert!(serde_json::from_value::<CiVerifiedFailureProof>(value).is_err());
}

#[test]
fn attached_proof_round_trips_without_changing_typed_conclusion() {
    let mut job = completed_job();
    job.verified_failure = Some(proof(CiOrdinaryFailureCategory::Test));
    let encoded = serde_json::to_string(&job).unwrap();
    let decoded: CiJob = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, job);
    assert_eq!(decoded.conclusion, Some(CiJobConclusion::Unknown));
}
