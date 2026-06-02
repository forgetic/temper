use async_trait::async_trait;
use harness_agents::{
    ProductManagerDraftIssue, ProductManagerError, ProductManagerRequest, ProductManagerResponse,
};
use harness_forge::{
    CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel, User, UserId,
};
use harness_forge_memory::MemoryForge;

use crate::product_chat::{
    parse_transcript_session_key, render_filing_marker, render_transcript_marker, ProductChatError,
    ProductChatOpenOptions, ProductChatSession, ProductManagerResponder, PRODUCT_LABEL,
    WORKFLOW_INTAKE_LABEL,
};

struct FakeResponder {
    response: ProductManagerResponse,
}

#[async_trait]
impl ProductManagerResponder for FakeResponder {
    async fn respond(
        &self,
        _request: &ProductManagerRequest,
    ) -> Result<ProductManagerResponse, ProductManagerError> {
        Ok(self.response.clone())
    }
}

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

async fn seeded() -> (MemoryForge, MemoryForge, Repository) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user("human"));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "harness".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    for label in [PRODUCT_LABEL, WORKFLOW_INTAKE_LABEL] {
        human
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label.into(),
                    color: Some("ededed".into()),
                    description: None,
                },
            )
            .await
            .unwrap();
    }
    (human, forge.as_user(user("product-manager")), repo)
}

fn fake_responder() -> FakeResponder {
    FakeResponder {
        response: ProductManagerResponse {
            reply: "Let's file a small MVP.".into(),
            drafts: vec![ProductManagerDraftIssue {
                slug: "terminal-chat-mvp".into(),
                title: "Add terminal product-manager chat MVP".into(),
                body: "Create a terminal MVP for product-manager chat.".into(),
                rationale: Some("A cheap dogfood loop comes first.".into()),
            }],
        },
    }
}

#[test]
fn product_chat_markers_render_and_parse() {
    let marker = render_transcript_marker("pc-abc");
    let body = format!("hello\n\n{marker}");
    assert_eq!(parse_transcript_session_key(&body), Some("pc-abc".into()));
    assert_eq!(
        render_filing_marker("pc-abc", "terminal-chat-mvp"),
        "<!-- harness:product-chat-file=pc-abc:terminal-chat-mvp -->"
    );
}

#[tokio::test]
async fn product_chat_core_drives_transcript_and_idempotent_filing() {
    let (human, product_manager, _repo) = seeded().await;
    let responder = fake_responder();
    let mut session = ProductChatSession::open(
        &human,
        &product_manager,
        &responder,
        ProductChatOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "harness"),
            transcript_issue: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        session.transcript_issue().labels,
        vec![PRODUCT_LABEL.to_string()]
    );

    let response = session.send_human_turn("I want a chat MVP.").await.unwrap();
    assert_eq!(response.drafts.len(), 1);
    let comments = human
        .list_issue_comments(&session.transcript_issue().id)
        .await
        .unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author_id, UserId::new("human"));
    assert_eq!(comments[1].author_id, UserId::new("product-manager"));

    let filed = session.file_draft(1).await.unwrap();
    assert!(filed.created);
    assert_eq!(filed.issue.labels, vec![WORKFLOW_INTAKE_LABEL.to_string()]);
    assert!(filed.issue.body.contains("requested-by: human"));
    assert!(filed.issue.body.contains(&render_filing_marker(
        session.session_key(),
        "terminal-chat-mvp"
    )));
    let retry = session.file_draft(1).await.unwrap();
    assert!(!retry.created);
    assert_eq!(retry.issue.number, filed.issue.number);
}

#[tokio::test]
async fn product_chat_file_refuses_invalid_draft_numbers() {
    let (human, product_manager, _repo) = seeded().await;
    let responder = fake_responder();
    let session = ProductChatSession::open(
        &human,
        &product_manager,
        &responder,
        ProductChatOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "harness"),
            transcript_issue: None,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        session.file_draft(1).await,
        Err(ProductChatError::InvalidDraftNumber { .. })
    ));
}
