use std::path::PathBuf;
use std::time::Duration;

use super::process::tune_init_config;
use super::runtime::{action_name, walk_steps};
use super::{
    ConvergenceStrategy, ManifestAction, ManifestStep, RecoveryFixture, StimulusKind, StimulusSpec,
};

#[test]
fn walker_dispatches_every_manifest_action_in_declared_order() {
    let steps = vec![
        step("forge", ManifestAction::ProvisionForgejo),
        step("runner", ManifestAction::AwaitForgejoRunner),
        step(
            "repo",
            ManifestAction::SeedRepository {
                repo_id: "service".to_string(),
                seed_path: PathBuf::from("repo"),
                ci_source_path: PathBuf::from("ci.yml"),
            },
        ),
        step(
            "jig",
            ManifestAction::StartJig {
                script_path: PathBuf::from("jig.json"),
                roles: vec!["engineer".to_string()],
                late_stream_failure: None,
            },
        ),
        step(
            "temper",
            ManifestAction::LaunchTemper {
                workflow_path: PathBuf::from("workflow.json"),
            },
        ),
        step(
            "issue",
            ManifestAction::SeedIssue {
                issue_id: "source".to_string(),
                repo_id: "service".to_string(),
                binding: Some("source".to_string()),
                after_pr_binding: None,
            },
        ),
        step(
            "pr",
            ManifestAction::SeedPullRequest {
                repo_id: "service".to_string(),
                source_issue_id: "source".to_string(),
                title: "stale".to_string(),
                body: "stale".to_string(),
                metadata_kind: "implementation_pr".to_string(),
                correlation_key: "$correlation:source".to_string(),
            },
        ),
        step(
            "mcp",
            ManifestAction::StartCodebaseMemoryMcp {
                project: "demo".to_string(),
                safe_tools: vec!["search_code".to_string()],
                hidden_tools: vec!["index_repository".to_string()],
                readiness_delay_ms: 0,
                forced_systemic_failure: None,
            },
        ),
        step(
            "tools",
            ManifestAction::ConfigureAgentTools {
                role: "engineer".to_string(),
                tool: "codebase_memory".to_string(),
                mode: "required".to_string(),
                index: "blocking".to_string(),
                tool_timeout_secs: None,
                server_step: "$step:mcp".to_string(),
            },
        ),
        step(
            "stimulus",
            ManifestAction::Stimulus(StimulusSpec {
                id: "restart".to_string(),
                kind: StimulusKind::RestartRunner,
                timeout: Duration::from_secs(1),
                max_attempts: 1,
            }),
        ),
        step(
            "converge",
            ManifestAction::WaitForConvergence {
                strategy: ConvergenceStrategy::SinglePullRequest,
            },
        ),
    ];
    let mut dispatched = Vec::new();

    walk_steps(&steps, |step| {
        dispatched.push(action_name(&step.action));
        Ok(())
    })
    .expect("all actions dispatch");

    assert_eq!(dispatched.len(), steps.len());
    assert_eq!(dispatched.first(), Some(&"forgejo.provision"));
    assert_eq!(dispatched.last(), Some(&"workflow.wait_convergence"));
    assert!(dispatched.contains(&"mcp.fake_codebase_memory.start"));
    assert!(dispatched.contains(&"pr.seed_existing"));
    assert!(dispatched.contains(&"forgejo_runner.restart"));
}

#[test]
fn recovery_tuning_adds_deadlines_to_minimal_init_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[engine]\npoll_cadence_secs = 60\n[worker]\nworkspace = \"workspaces\"\n[agent]\nprovider = \"deepseek\"\n",
    )
    .expect("minimal init config");
    let recovery = RecoveryFixture {
        model_retry_max_attempts: 2,
        model_retry_base_delay_ms: 1,
        model_retry_max_delay_ms: 2,
        model_retry_jitter_percent: 0,
        session_failure_limit: 1,
        fresh_session_limit: 1,
        provider_deferral_limit: 3,
        provider_deferral_delay_secs: 300,
        model_recovery_slo_secs: 7_200,
    };

    let effective =
        tune_init_config(&path, 600, 17, 1, Some(&recovery)).expect("tune recovery config");

    let tuned = std::fs::read_to_string(path).expect("tuned config");
    let parsed = tuned.parse::<toml::Value>().expect("tuned TOML");
    let deadlines = parsed["agent"]["deadlines"]
        .as_table()
        .expect("created agent.deadlines table");
    assert_eq!(deadlines["model_retry_max_attempts"].as_integer(), Some(2));
    assert_eq!(effective.poll_cadence_secs, 600);
    assert_eq!(effective.ci_poll_cadence_secs, 17);
    assert_eq!(effective.mechanical_cadence_secs, 1);
    assert_eq!(
        parsed["engine"]["poll_cadence_secs"].as_integer(),
        Some(600)
    );
    assert_eq!(
        parsed["engine"]["ci_poll_cadence_secs"].as_integer(),
        Some(17)
    );
    assert_eq!(
        parsed["engine"]["mechanical_cadence_secs"].as_integer(),
        Some(1)
    );
    assert_eq!(
        deadlines["model_retry_jitter_percent"].as_integer(),
        Some(0)
    );
    assert_eq!(
        parsed["worker"]["fresh_session_limit"].as_integer(),
        Some(1)
    );
}

#[test]
fn verified_failure_is_projected_without_integrity_material() {
    use temper_forge_model::{
        CiFailureProofAttestation, CiFailureProofCoordinates, CiFailureProofSubject,
        CiFailureProofVerification, CiJob, CiJobConclusion, CiJobId, CiJobStatus,
        CiOrdinaryFailureCategory, CiVerifiedFailureProof, PullRequestId, RepositoryId,
    };

    let repository = RepositoryId::new("forgejo:acme/service");
    let pull_request = PullRequestId::new("forgejo:acme/service:pull:7");
    let created_at = "2026-07-26T12:00:00Z".parse().unwrap();
    let expires_at = "2026-07-26T12:05:00Z".parse().unwrap();
    let proof = CiVerifiedFailureProof::new(
        CiOrdinaryFailureCategory::Test,
        CiFailureProofSubject::new(
            repository.clone(),
            Some(pull_request.clone()),
            "0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap(),
        CiFailureProofCoordinates::new("591", "42", "2", Some("9001")).unwrap(),
        CiFailureProofAttestation::new(
            "forgejo-actions",
            "temper-proof-issuer",
            CiFailureProofVerification::ProtectedProducer,
        )
        .unwrap(),
        created_at,
        expires_at,
    )
    .unwrap();
    let job = CiJob {
        id: CiJobId::new("portable-job"),
        repo_id: repository,
        pull_request_id: Some(pull_request),
        commit_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
        name: "test".to_string(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Failure),
        provider_conclusion: Some("failure".to_string()),
        provider_reason: None,
        run_id: Some("591".to_string()),
        attempt: Some("2".to_string()),
        verified_failure: Some(proof),
        url: None,
        created_at,
        started_at: Some(created_at),
        completed_at: Some(created_at),
        updated_at: created_at,
    };

    let evidence = super::convergence::ci_job_evidence(&job);
    let proof = evidence.verified_failure.expect("projected proof");
    assert_eq!(proof.category, "test");
    assert_eq!(proof.repository_id, "forgejo:acme/service");
    assert_eq!(
        proof.pull_request_id.as_deref(),
        Some("forgejo:acme/service:pull:7")
    );
    assert_eq!(proof.run_id, "591");
    assert_eq!(proof.job_id, "42");
    assert_eq!(proof.attempt, "2");
    assert_eq!(proof.task_id.as_deref(), Some("9001"));
    assert_eq!(proof.producer_id, "forgejo-actions");
    assert_eq!(proof.issuer_id, "temper-proof-issuer");
    assert_eq!(proof.verification, "protected_producer");
}

fn step(id: &str, action: ManifestAction) -> ManifestStep {
    ManifestStep {
        id: id.to_string(),
        action,
    }
}
