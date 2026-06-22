use super::*;
use temper_forge::{CreateIssue, CreateRepository, Forge, IssueState, UpdateIssue};
use temper_forge_memory::MemoryForge;
use temper_workflow::{ArtifactKindId, render_metadata_block};

#[test]
fn parse_requires_token_from_env_and_redacts_debug() {
    let outcome = parse_with_env(
        [
            "--base-url",
            "http://127.0.0.1:3000",
            "--repo",
            "acme/service",
            "--source-repo",
            "acme/service",
            "--parent-number",
            "1",
            "--expected-children",
            "2",
        ]
        .into_iter()
        .map(String::from),
        |key| (key == VALIDATOR_TOKEN_ENV).then(|| "secret-token".to_string()),
    )
    .expect("parses");
    let ParseOutcome::Run(args) = outcome else {
        panic!("expected run")
    };
    assert!(!format!("{args:?}").contains("secret-token"));
}

#[test]
fn zero_dependency_blocked_parent_reports_original_incident_shape() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let source = create_repo(&forge, "acme", "service").await;
        let parent = create_issue(
            &forge,
            &source,
            "Coordinate greeting",
            &["code", "blocked"],
            WorkflowMetadata {
                kind: Some(ArtifactKindId::new("code")),
                ..WorkflowMetadata::default()
            },
            false,
        )
        .await;

        let report = validate_state(
            &forge,
            &ValidatorConfig {
                source_repo: RepositoryPath::new("acme", "service"),
                repositories: vec![RepositoryPath::new("acme", "service")],
                parent_number: parent.number,
                expected_children: 2,
            },
        )
        .await
        .expect("validation reads state");

        assert!(!report.is_ok());
        let rendered = report.render();
        assert!(rendered.contains("blocked parent acme/service#1 has zero dependencies"));
        assert!(
            rendered.contains(
                "cross-repo parent acme/service#1 expected 2 child dependencies, found 0"
            )
        );
        assert!(
            rendered
                .contains("architect blocked the parent but no fan-out side effects were recorded")
        );
    })
}

#[test]
fn parent_with_landed_child_backrefs_and_correlation_passes() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let source = create_repo(&forge, "acme", "service").await;
        let target = create_repo(&forge, "acme", "service-canary").await;
        let parent = create_issue(
            &forge,
            &source,
            "parent",
            &["code"],
            WorkflowMetadata {
                kind: Some(ArtifactKindId::new("code")),
                ..WorkflowMetadata::default()
            },
            false,
        )
        .await;
        let child_a = create_issue(
            &forge,
            &source,
            "service child",
            &["code", "ready"],
            child_metadata(&source, parent.number, "child-a"),
            true,
        )
        .await;
        let child_b = create_issue(
            &forge,
            &target,
            "canary child",
            &["code", "ready"],
            child_metadata(&source, parent.number, "child-b"),
            true,
        )
        .await;
        let parent_metadata = WorkflowMetadata {
            kind: Some(ArtifactKindId::new("code")),
            dependencies: vec![
                ArtifactRef::in_repo(source.clone(), child_a.number),
                ArtifactRef::in_repo(target.clone(), child_b.number),
            ],
            ..WorkflowMetadata::default()
        };
        forge
            .update_issue(
                &parent.id,
                UpdateIssue {
                    body: Some(render_metadata_block(&parent_metadata)),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("parent dependencies updated");

        let report = validate_state(
            &forge,
            &ValidatorConfig {
                source_repo: RepositoryPath::new("acme", "service"),
                repositories: vec![
                    RepositoryPath::new("acme", "service"),
                    RepositoryPath::new("acme", "service-canary"),
                ],
                parent_number: ItemNumber::new(1),
                expected_children: 2,
            },
        )
        .await
        .expect("validation reads state");

        assert!(report.is_ok(), "{}", report.render());
        assert!(
            report
                .lines()
                .iter()
                .any(|line| line.contains("expected 2 child dependencies, found 2"))
        );
        assert!(
            report
                .lines()
                .iter()
                .any(|line| line.contains("child landed count 2/2"))
        );
    })
}

async fn create_repo(forge: &MemoryForge, owner: &str, name: &str) -> RepositoryId {
    forge
        .create_repository(CreateRepository {
            owner: owner.to_string(),
            name: name.to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repo created")
        .id
}

async fn create_issue(
    forge: &MemoryForge,
    repo: &RepositoryId,
    title: &str,
    labels: &[&str],
    metadata: WorkflowMetadata,
    closed: bool,
) -> Issue {
    let issue = forge
        .create_issue(
            repo,
            CreateIssue {
                title: title.to_string(),
                body: render_metadata_block(&metadata),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue created");
    if closed {
        forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("issue closed")
    } else {
        issue
    }
}

fn child_metadata(source: &RepositoryId, parent: ItemNumber, key: &str) -> WorkflowMetadata {
    WorkflowMetadata {
        kind: Some(ArtifactKindId::new("code")),
        parents: vec![ArtifactRef::in_repo(source.clone(), parent)],
        correlation_key: Some(key.to_string()),
        ..WorkflowMetadata::default()
    }
}
