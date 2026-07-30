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
            },
        ),
        step(
            "tools",
            ManifestAction::ConfigureAgentTools {
                role: "engineer".to_string(),
                tool: "codebase_memory".to_string(),
                mode: "required".to_string(),
                index: "blocking".to_string(),
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

    tune_init_config(&path, 600, 1, Some(&recovery)).expect("tune recovery config");

    let tuned = std::fs::read_to_string(path).expect("tuned config");
    let parsed = tuned.parse::<toml::Value>().expect("tuned TOML");
    let deadlines = parsed["agent"]["deadlines"]
        .as_table()
        .expect("created agent.deadlines table");
    assert_eq!(deadlines["model_retry_max_attempts"].as_integer(), Some(2));
    assert_eq!(
        deadlines["model_retry_jitter_percent"].as_integer(),
        Some(0)
    );
    assert_eq!(
        parsed["worker"]["fresh_session_limit"].as_integer(),
        Some(1)
    );
}

fn step(id: &str, action: ManifestAction) -> ManifestStep {
    ManifestStep {
        id: id.to_string(),
        action,
    }
}
