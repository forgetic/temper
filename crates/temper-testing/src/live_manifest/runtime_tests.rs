use std::path::PathBuf;
use std::time::Duration;

use super::runtime::{action_name, walk_steps};
use super::{ConvergenceStrategy, ManifestAction, ManifestStep, StimulusKind, StimulusSpec};

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

fn step(id: &str, action: ManifestAction) -> ManifestStep {
    ManifestStep {
        id: id.to_string(),
        action,
    }
}
