// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use temper_engine::{
    CoordinatedMechanical, MechanicalBackstopConfig, MechanicalScope, MechanicalTrigger,
    run_mechanical_backstop_tick,
};
use temper_forge::{
    ChangeHint, ChangeKind, CreateIssue, CreateRepository, Forge, HintArtifactKind, IssueState,
    ItemNumber, RepositoryId, RepositoryPath, UpdateIssue,
};
use temper_forge_memory::MemoryForge;
use temper_runner::{ArtifactAddress, Progress, RepositorySet, RepositoryTarget};
use temper_workflow::{
    ArtifactKindId, InMemoryJournal, LeasePolicy, RawWorkflowSpec, ValidatedWorkflow,
    WorkflowMetadata, render_metadata_block,
};

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
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
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
            pull_request_merge_observer: None,
        };
        let journals = vec![InMemoryJournal::new()];

        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
                &MechanicalScope::All,
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
                &MechanicalScope::All,
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

fn hint(owner: &str, name: &str) -> ChangeHint {
    ChangeHint::repository(RepositoryPath::new(owner, name), ChangeKind::Push)
}

#[test]
fn hinted_scope_ticks_only_the_matching_repository() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
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
            pull_request_merge_observer: None,
        };
        let journals = vec![InMemoryJournal::new()];

        // A hint for an unrelated repo matches nothing: the pass is a no-op, so
        // the blocked issue is untouched (a forged/stale hint is safe).
        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
                &MechanicalScope::Hinted(vec![hint("other", "repo")]),
            )
            .await
            .expect("tick succeeds"),
            Progress::unchanged(),
            "a hint for an unconfigured repo must not tick the configured one"
        );
        assert_eq!(
            issue_labels(&forge, &repo.id, blocked).await,
            vec!["blocked".to_string(), "code".to_string()]
        );

        // A hint naming the configured repo runs the same reconciliation a broad
        // backstop pass would — the webhook accelerates exactly that repo.
        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:01Z"),
                &config,
                &journals,
                &MechanicalScope::Hinted(vec![hint("acme", "service")]),
            )
            .await
            .expect("tick succeeds"),
            Progress {
                changed: true,
                actions: 1,
            },
            "a hint for the configured repo must tick it"
        );
        assert_eq!(
            issue_labels(&forge, &repo.id, blocked).await,
            vec!["code".to_string(), "ready".to_string()]
        );
    })
}

#[test]
fn targeted_mechanical_wake_does_not_mutate_staged_artifact() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = MemoryForge::new();
        let repo = new_repo(&forge).await;
        let dependency = create_issue(&forge, &repo.id, &["code", "ready"]).await;
        close_issue(&forge, &repo.id, dependency).await;
        let staged = forge
            .create_issue(
                &repo.id,
                CreateIssue {
                    title: "staged blocked child".to_string(),
                    body: render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        staged: true,
                        ..WorkflowMetadata::default()
                    }),
                    labels: vec!["code".to_string(), "blocked".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("staged issue is created")
            .number;
        add_issue_dependency(&forge, &repo.id, staged, dependency).await;
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![repo.clone()]),
            cadence: Duration::from_millis(10),
            lease_policy: lease_policy(),
            pull_request_merge_observer: None,
        };

        let journals = [InMemoryJournal::new()];
        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow(),
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
                &MechanicalScope::Targeted(vec![(
                    repo.path.clone(),
                    ArtifactAddress::new(HintArtifactKind::Issue, staged),
                    ChangeKind::Dependency,
                )]),
            )
            .await
            .expect("targeted tick succeeds"),
            Progress::unchanged()
        );
        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow(),
                ts("2026-05-29T00:00:01Z"),
                &config,
                &journals,
                &MechanicalScope::All,
            )
            .await
            .expect("broad tick succeeds"),
            Progress::unchanged()
        );
        assert_eq!(
            issue_labels(&forge, &repo.id, staged).await,
            vec!["blocked".to_string(), "code".to_string()]
        );
    })
}

#[test]
fn mechanical_trigger_executes_only_coordinator_admitted_broad_work() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let dependency = create_issue(&forge, &repo.id, &["code", "ready"]).await;
        close_issue(&forge, &repo.id, dependency).await;
        let blocked = create_issue(&forge, &repo.id, &["code", "blocked"]).await;
        add_issue_dependency(&forge, &repo.id, blocked, dependency).await;
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![repo.clone()]),
            cadence: Duration::from_secs(300),
            lease_policy: lease_policy(),
            pull_request_merge_observer: None,
        };
        let clock: temper_engine::WallClock = Arc::new(|| ts("2026-05-29T00:00:00Z"));
        let trigger =
            MechanicalTrigger::new(Arc::clone(&forge), Arc::new(workflow()), config, clock);

        trigger
            .run_coordinated_broad(RepositoryPath::new("acme", "service"))
            .await
            .expect("coordinator-admitted broad pass succeeds");
        assert_eq!(
            issue_labels(&forge, &repo.id, blocked).await,
            vec!["code".to_string(), "ready".to_string()]
        );
    })
}

#[test]
fn mechanical_trigger_clones_share_cache_while_new_triggers_restart_empty() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = Arc::new(MemoryForge::new());
        let repo = new_repo(&forge).await;
        let dependency = create_issue(&forge, &repo.id, &["design", "draft"]).await;
        let blocked = create_issue(&forge, &repo.id, &["code", "blocked"]).await;
        add_issue_dependency(&forge, &repo.id, blocked, dependency).await;
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(vec![repo.clone()]),
            cadence: Duration::from_secs(300),
            lease_policy: lease_policy(),
            pull_request_merge_observer: None,
        };
        let clock: temper_engine::WallClock = Arc::new(|| ts("2026-05-29T00:00:00Z"));
        let trigger = MechanicalTrigger::new(
            Arc::clone(&forge),
            Arc::new(workflow()),
            config.clone(),
            clock.clone(),
        );
        let clone = trigger.clone();

        clone
            .run_coordinated_broad(repo.path.clone())
            .await
            .expect("first broad pass succeeds");
        assert_eq!(trigger.reconciliation_detail_cache().len(), 1);
        trigger
            .run_coordinated_broad(repo.path.clone())
            .await
            .expect("second broad pass succeeds");
        assert_eq!(clone.reconciliation_detail_cache().len(), 1);

        let restarted = MechanicalTrigger::new(forge, Arc::new(workflow()), config, clock);
        assert!(
            restarted.reconciliation_detail_cache().is_empty(),
            "a reconstructed runtime must start with an authoritative cold cache"
        );
    })
}

#[test]
fn run_mechanical_backstop_tick_with_no_repositories_is_unchanged() {
    temper_engine_io::block_on_with(move |_cx, _handle| async move {
        let forge = MemoryForge::new();
        let workflow = workflow();
        let config = MechanicalBackstopConfig {
            repositories: RepositorySet::new(Vec::new()),
            cadence: Duration::from_millis(10),
            lease_policy: lease_policy(),
            pull_request_merge_observer: None,
        };
        let journals = Vec::new();

        assert_eq!(
            run_mechanical_backstop_tick(
                &forge,
                &workflow,
                ts("2026-05-29T00:00:00Z"),
                &config,
                &journals,
                &MechanicalScope::All,
            )
            .await
            .expect("tick succeeds"),
            Progress::unchanged()
        );
    })
}
