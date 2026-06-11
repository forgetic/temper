// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_daemon::{run_mechanical_backstop_tick, MechanicalBackstopConfig};
use temper_forge::{
    CreateIssue, CreateRepository, Forge, IssueState, ItemNumber, RepositoryId, RepositoryPath,
    UpdateIssue,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{Progress, RepositorySet, RepositoryTarget};
use temper_workflow::{InMemoryJournal, LeasePolicy, RawWorkflowSpec, ValidatedWorkflow};

const FIXTURE: &str = include_str!("../../temper-workflow/fixtures/reference-delivery.json");

fn workflow() -> ValidatedWorkflow {
    let spec: RawWorkflowSpec = serde_json::from_str(FIXTURE).expect("workflow parses");
    spec.validate().expect("workflow validates")
}

fn ts(value: &str) -> DateTime<Utc> {
    value.parse().expect("valid RFC 3339 timestamp")
}

fn lease_policy() -> LeasePolicy {
    LeasePolicy::new(chrono::Duration::minutes(30))
}

async fn new_repo(forge: &MemoryForge) -> RepositoryTarget {
    let repo = forge
        .create_repository(CreateRepository {
            owner: "acme".to_string(),
            name: "service".to_string(),
            default_branch: "main".to_string(),
            description: None,
        })
        .await
        .expect("repository is created");
    RepositoryTarget::new(repo.id, RepositoryPath::new(repo.owner, repo.name))
}

async fn create_issue(forge: &MemoryForge, repo: &RepositoryId, labels: &[&str]) -> ItemNumber {
    forge
        .create_issue(
            repo,
            CreateIssue {
                title: "mechanical issue".to_string(),
                body: String::new(),
                labels: labels.iter().map(|label| (*label).to_string()).collect(),
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue is created")
        .number
}

async fn close_issue(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) {
    let issue = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("lookup succeeds")
        .expect("issue exists");
    forge
        .update_issue(
            &issue.id,
            UpdateIssue {
                state: Some(IssueState::Closed),
                ..UpdateIssue::default()
            },
        )
        .await
        .expect("issue closes");
}

async fn add_issue_dependency(
    forge: &MemoryForge,
    repo: &RepositoryId,
    source: ItemNumber,
    target: ItemNumber,
) {
    let issue = forge
        .get_issue_by_number(repo, source)
        .await
        .expect("lookup succeeds")
        .expect("issue exists");
    forge
        .add_issue_dependency(&issue.id, target)
        .await
        .expect("dependency link added");
}

async fn issue_labels(forge: &MemoryForge, repo: &RepositoryId, number: ItemNumber) -> Vec<String> {
    let mut labels = forge
        .get_issue_by_number(repo, number)
        .await
        .expect("lookup succeeds")
        .expect("issue exists")
        .labels;
    labels.sort();
    labels
}

#[test]
fn run_mechanical_backstop_tick_applies_dependency_unblock_once() {
    temper_io_engine::block_on(async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let dependency = create_issue(&forge, &repo.id, &["code", "ready"]).await;
        close_issue(&forge, &repo.id, dependency).await;
        let blocked = create_issue(&forge, &repo.id, &["code", "blocked"]).await;
        add_issue_dependency(&forge, &repo.id, blocked, dependency).await;
        let workflow = workflow();
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![repo.clone()]),
            cadence: Duration::from_millis(10),
            lease_policy: lease_policy(),
        };
        let journals = vec![InMemoryJournal::new()];

        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
            )
            .await
            .expect("tick succeeds"),
            Progress {
                changed: true,
                actions: 1,
            }
        );
        assert_eq!(
            issue_labels(&forge, &repo.id, blocked).await,
            vec!["code".to_string(), "ready".to_string()]
        );

        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:01Z"),
                &config,
                &journals,
            )
            .await
            .expect("second tick succeeds"),
            Progress::unchanged()
        );
        assert_eq!(
            issue_labels(&forge, &repo.id, blocked).await,
            vec!["code".to_string(), "ready".to_string()]
        );
    })
}

#[test]
fn run_mechanical_backstop_tick_with_no_repositories_is_unchanged() {
    temper_io_engine::block_on(async move {
        let forge = MemoryForge::new();
        let workflow = workflow();
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(Vec::new()),
            cadence: Duration::from_millis(10),
            lease_policy: lease_policy(),
        };
        let journals = Vec::new();

        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
            )
            .await
            .expect("tick succeeds"),
            Progress::unchanged()
        );
    })
}
