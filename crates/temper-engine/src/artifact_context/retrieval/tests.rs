use async_trait::async_trait;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use temper_forge::{
    BranchRef, Comment, CreateComment, CreateIssue, CreatePullRequest, CreateRepository, Forge,
    ForgeError, ForgeResult, Issue, IssueId, IssueQuery, ItemNumber, PullRequest, PullRequestId,
    PullRequestQuery, RepositoryId, RepositoryPath,
};
use temper_forge_memory::MemoryForge;
use temper_protocol_context::{
    ArtifactType, ForgeContextErrorCode, ForgeGetItemOperation, ForgeListRelatedOperation,
    ForgeRelationType,
};
use temper_runner::RepositoryTarget;
use temper_workflow::{
    ArtifactKindId, ArtifactRef, RawWorkflowSpec, WorkflowMetadata, render_metadata_block,
};

use super::*;

const WORKFLOW: &str = include_str!("../../../../temper-workflow/fixtures/reference-delivery.json");

struct RecordingForge<'a> {
    inner: &'a MemoryForge,
    limits: Mutex<Vec<Option<usize>>>,
    fail_reads: AtomicBool,
}

impl<'a> RecordingForge<'a> {
    fn new(inner: &'a MemoryForge) -> Self {
        Self {
            inner,
            limits: Mutex::new(Vec::new()),
            fail_reads: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ArtifactContextForge for RecordingForge<'_> {
    async fn issue(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<Issue>> {
        if self.fail_reads.load(Ordering::Relaxed) {
            return Err(ForgeError::Backend("sensitive backend detail".into()));
        }
        self.inner.get_issue_by_number(repository, number).await
    }

    async fn pull_request(
        &self,
        repository: &RepositoryId,
        number: ItemNumber,
    ) -> ForgeResult<Option<PullRequest>> {
        self.inner
            .get_pull_request_by_number(repository, number)
            .await
    }

    async fn issues(
        &self,
        repository: &RepositoryId,
        query: IssueQuery,
    ) -> ForgeResult<Vec<Issue>> {
        self.limits.lock().unwrap().push(query.limit);
        self.inner.list_issues(repository, query).await
    }

    async fn pull_requests(
        &self,
        repository: &RepositoryId,
        query: PullRequestQuery,
    ) -> ForgeResult<Vec<PullRequest>> {
        self.limits.lock().unwrap().push(query.limit);
        self.inner.list_pull_requests(repository, query).await
    }

    async fn issue_comments(&self, issue: &IssueId) -> ForgeResult<Vec<Comment>> {
        self.inner.list_issue_comments(issue).await
    }

    async fn pull_request_comments(
        &self,
        pull_request: &PullRequestId,
    ) -> ForgeResult<Vec<Comment>> {
        self.inner.list_pull_request_comments(pull_request).await
    }
}

fn issue(title: &str, prose: &str, metadata: WorkflowMetadata, labels: &[&str]) -> CreateIssue {
    CreateIssue {
        title: title.into(),
        body: format!("{prose}\n{}", render_metadata_block(&metadata)),
        labels: labels.iter().map(|label| (*label).to_string()).collect(),
        assignees: Vec::new(),
    }
}

#[test]
fn bounded_service_reads_items_comments_and_every_relation_direction() {
    temper_engine_io::block_on(async move {
        let forge = MemoryForge::new();
        let repository = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "temper".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let secondary = forge
            .create_repository(CreateRepository {
                owner: "ai".into(),
                name: "other".into(),
                default_branch: "main".into(),
                description: None,
            })
            .await
            .unwrap();
        let epic = forge
            .create_issue(
                &repository.id,
                issue(
                    "epic",
                    "root",
                    WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("epic")),
                        ..Default::default()
                    },
                    &["epic"],
                ),
            )
            .await
            .unwrap();
        let design = forge
            .create_issue(
                &repository.id,
                issue(
                    "design",
                    "design",
                    WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("design")),
                        parents: vec![ArtifactRef::same_repo(epic.number)],
                        ..Default::default()
                    },
                    &["design"],
                ),
            )
            .await
            .unwrap();
        let dependency = forge
            .create_issue(
                &repository.id,
                issue(
                    "dependency",
                    "dependency",
                    WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        ..Default::default()
                    },
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        let code = forge
            .create_issue(
                &repository.id,
                issue(
                    "code",
                    &format!("See #{}", dependency.number.get()),
                    WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        parents: vec![ArtifactRef::same_repo(design.number)],
                        dependencies: vec![ArtifactRef::same_repo(dependency.number)],
                        ..Default::default()
                    },
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        let pull_request = forge
            .create_pull_request(
                &repository.id,
                CreatePullRequest {
                    title: "implementation".into(),
                    body: render_metadata_block(&WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("implementation_pr")),
                        parents: vec![ArtifactRef::same_repo(code.number)],
                        ..Default::default()
                    }),
                    source: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "feature".into(),
                    },
                    target: BranchRef {
                        repository_id: repository.id.clone(),
                        branch: "main".into(),
                    },
                    labels: vec!["implementation".into(), "needs-reviewer".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let cross_repo_child = forge
            .create_issue(
                &secondary.id,
                issue(
                    "cross-repo code",
                    "child",
                    WorkflowMetadata {
                        kind: Some(ArtifactKindId::new("code")),
                        parents: vec![ArtifactRef::in_repo(repository.id.clone(), design.number)],
                        ..Default::default()
                    },
                    &["code", "ready"],
                ),
            )
            .await
            .unwrap();
        for index in 0..21 {
            forge
                .add_issue_comment(
                    &code.id,
                    CreateComment {
                        body: format!("{index:02}{}", "x".repeat(MAX_COMMENT_BYTES + 10)),
                    },
                )
                .await
                .unwrap();
        }

        let workflow: RawWorkflowSpec = serde_json::from_str(WORKFLOW).unwrap();
        let workflow = workflow.validate().unwrap();
        let catalog = ConfiguredRepositoryCatalog::new(
            [
                RepositoryTarget::new(repository.id.clone(), RepositoryPath::new("ai", "temper")),
                RepositoryTarget::new(secondary.id.clone(), RepositoryPath::new("ai", "other")),
            ],
            "https://forge.example",
        )
        .unwrap();
        let recording = RecordingForge::new(&forge);
        let service = ArtifactContextService::new(&recording, &catalog, &workflow);

        let item = service
            .forge_get_item(ForgeGetItemOperation {
                repo: "ai/temper".into(),
                number: code.number.get(),
                artifact_type: None,
                include_comments: true,
            })
            .await
            .unwrap();
        assert_eq!(item.item.artifact.artifact_type, ArtifactType::Issue);
        assert_eq!(
            item.item.body,
            format!("See #{}\n", dependency.number.get())
        );
        assert_eq!(item.item.workflow_kind.as_deref(), Some("code"));
        let projected = item.item.workflow.as_ref().expect("workflow projection");
        assert_eq!(projected.kind.as_deref(), Some("code"));
        assert_eq!(
            projected.parents[0].repository_id,
            repository.id.to_string()
        );
        assert_eq!(projected.parents[0].number, design.number.get());
        assert_eq!(item.comments.len(), MAX_ITEM_COMMENTS);
        assert!(
            item.comments
                .iter()
                .all(|comment| comment.body.len() <= MAX_COMMENT_BYTES)
        );
        assert!(item.truncation.count_exceeded);
        assert!(item.truncation.content_truncated);
        assert!(serde_json::to_vec(&item).unwrap().len() <= MAX_FORGE_RESPONSE_BYTES);

        let direct = service
            .forge_list_related(ForgeListRelatedOperation {
                repo: repository.id.to_string(),
                number: code.number.get(),
                artifact_type: Some(ArtifactType::Issue),
                relations: vec![
                    ForgeRelationType::Parent,
                    ForgeRelationType::Dependency,
                    ForgeRelationType::ProducedPr,
                    ForgeRelationType::BodyReference,
                ],
                depth: Some(1),
                limit: Some(50),
            })
            .await
            .unwrap();
        let edge_types: BTreeSet<_> = direct.edges.iter().map(|edge| edge.relation).collect();
        assert_eq!(
            edge_types,
            BTreeSet::from([
                ForgeRelationType::Parent,
                ForgeRelationType::Dependency,
                ForgeRelationType::ProducedPr,
                ForgeRelationType::BodyReference,
            ])
        );
        assert!(
            direct
                .items
                .iter()
                .any(|item| item.artifact.number == pull_request.number.get())
        );

        let children = service
            .forge_list_related(ForgeListRelatedOperation {
                repo: "ai/temper".into(),
                number: design.number.get(),
                artifact_type: Some(ArtifactType::Issue),
                relations: vec![ForgeRelationType::Child],
                depth: None,
                limit: None,
            })
            .await
            .unwrap();
        assert_eq!(children.items.len(), 2);
        assert!(children.items.iter().any(|item| {
            item.artifact.number == code.number.get()
                && item.artifact.repository.id == repository.id.to_string()
        }));
        assert!(children.items.iter().any(|item| {
            item.artifact.number == cross_repo_child.number.get()
                && item.artifact.repository.id == secondary.id.to_string()
        }));

        let inverse = service
            .forge_list_related(ForgeListRelatedOperation {
                repo: "ai/temper".into(),
                number: dependency.number.get(),
                artifact_type: Some(ArtifactType::Issue),
                relations: vec![
                    ForgeRelationType::Dependent,
                    ForgeRelationType::ReferencedBy,
                ],
                depth: Some(1),
                limit: Some(50),
            })
            .await
            .unwrap();
        let inverse_types: BTreeSet<_> = inverse.edges.iter().map(|edge| edge.relation).collect();
        assert!(inverse_types.contains(&ForgeRelationType::Dependent));
        assert!(inverse_types.contains(&ForgeRelationType::ReferencedBy));

        let ancestors = service
            .forge_list_related(ForgeListRelatedOperation {
                repo: "ai/temper".into(),
                number: code.number.get(),
                artifact_type: None,
                relations: vec![ForgeRelationType::Parent],
                depth: Some(2),
                limit: Some(50),
            })
            .await
            .unwrap();
        assert_eq!(
            ancestors
                .items
                .iter()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>(),
            ["epic", "design"]
        );
        assert_eq!(
            service
                .forge_list_related(ForgeListRelatedOperation {
                    repo: "ai/temper".into(),
                    number: code.number.get(),
                    artifact_type: None,
                    relations: vec![ForgeRelationType::Parent],
                    depth: Some(MAX_RELATED_DEPTH + 1),
                    limit: None,
                })
                .await,
            Err(ForgeContextErrorCode::LimitExceeded)
        );
        assert_eq!(
            service
                .forge_list_related(ForgeListRelatedOperation {
                    repo: "ai/temper".into(),
                    number: code.number.get(),
                    artifact_type: None,
                    relations: vec![ForgeRelationType::Parent],
                    depth: None,
                    limit: Some(MAX_RELATED_RESULTS + 1),
                })
                .await,
            Err(ForgeContextErrorCode::LimitExceeded)
        );
        assert_eq!(
            service
                .forge_get_item(ForgeGetItemOperation {
                    repo: "unknown/repository".into(),
                    number: 1,
                    artifact_type: None,
                    include_comments: false,
                })
                .await,
            Err(ForgeContextErrorCode::NotAuthorized)
        );
        let explicit_pull_request = service
            .forge_get_item(ForgeGetItemOperation {
                repo: "ai/temper".into(),
                number: pull_request.number.get(),
                artifact_type: Some(ArtifactType::PullRequest),
                include_comments: false,
            })
            .await
            .unwrap();
        assert_eq!(
            explicit_pull_request.item.artifact.artifact_type,
            ArtifactType::PullRequest
        );
        assert_eq!(
            service
                .forge_get_item(ForgeGetItemOperation {
                    repo: "ai/temper".into(),
                    number: 999_999,
                    artifact_type: Some(ArtifactType::Issue),
                    include_comments: false,
                })
                .await,
            Err(ForgeContextErrorCode::NotFound)
        );
        let oversized = forge
            .create_issue(
                &repository.id,
                issue(
                    &"t".repeat(MAX_FORGE_RESPONSE_BYTES + 1),
                    "body",
                    WorkflowMetadata::default(),
                    &[],
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            service
                .forge_get_item(ForgeGetItemOperation {
                    repo: "ai/temper".into(),
                    number: oversized.number.get(),
                    artifact_type: Some(ArtifactType::Issue),
                    include_comments: false,
                })
                .await,
            Err(ForgeContextErrorCode::LimitExceeded)
        );
        let malformed = forge
            .create_issue(
                &repository.id,
                CreateIssue {
                    title: "malformed".into(),
                    body: format!("{}\n{{broken}}\n-->", temper_workflow::METADATA_BEGIN),
                    labels: vec!["code".into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let malformed_item = service
            .forge_get_item(ForgeGetItemOperation {
                repo: "ai/temper".into(),
                number: malformed.number.get(),
                artifact_type: Some(ArtifactType::Issue),
                include_comments: false,
            })
            .await
            .unwrap();
        assert_eq!(
            malformed_item.item.body,
            format!("{}\n{{broken}}\n-->", temper_workflow::METADATA_BEGIN)
        );
        assert!(malformed_item.item.workflow.is_none());
        assert_eq!(
            service
                .forge_list_related(ForgeListRelatedOperation {
                    repo: "ai/temper".into(),
                    number: malformed.number.get(),
                    artifact_type: Some(ArtifactType::Issue),
                    relations: vec![ForgeRelationType::Parent],
                    depth: None,
                    limit: None,
                })
                .await,
            Err(ForgeContextErrorCode::InvalidRequest)
        );
        {
            let scan_limits = recording.limits.lock().unwrap();
            assert!(!scan_limits.is_empty());
            assert!(
                scan_limits
                    .iter()
                    .all(|limit| *limit == Some(INVERSE_SCAN_LIMIT))
            );
        }
        recording.fail_reads.store(true, Ordering::Relaxed);
        assert_eq!(
            service
                .forge_get_item(ForgeGetItemOperation {
                    repo: "ai/temper".into(),
                    number: code.number.get(),
                    artifact_type: None,
                    include_comments: false,
                })
                .await,
            Err(ForgeContextErrorCode::ForgeUnavailable)
        );
    });
}
