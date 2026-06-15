use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use temper_forge::{
    CreateComment, CreateIssue, CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel,
    UserId,
};
use temper_forge_memory::MemoryForge;

use crate::{
    ConversationReply, ConversationRequest, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, InteractionError, InteractiveResponder, IssueProposal, Proposal,
    parse_transcript_session_key, render_acceptance_marker, render_transcript_marker,
};

use super::{
    INTAKE_LABEL, MARKER_NAMESPACE, TRANSCRIPT_LABEL, product_profile_manifest, proposal_id, user,
};

fn config() -> ForgeSessionConfig {
    ForgeSessionConfig::from_profile_manifest(&product_profile_manifest())
        .expect("valid session config")
}

fn acceptance_marker(conversation_id: &str, proposal_id: &str) -> String {
    let manifest = product_profile_manifest();
    let marker_key = manifest.acceptance_actions[0]
        .effects
        .iter()
        .find_map(|effect| match effect {
            crate::AcceptanceEffect::CreateIssue(effect) => effect.marker_key(),
            crate::AcceptanceEffect::AddTranscriptComment(_) => None,
        })
        .unwrap_or_else(|| manifest.acceptance_actions[0].id.as_str());
    render_acceptance_marker(
        MARKER_NAMESPACE,
        marker_key,
        &format!("{conversation_id}:{proposal_id}"),
    )
}

async fn seeded() -> (MemoryForge, MemoryForge, Repository) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user("human"));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    for label in [TRANSCRIPT_LABEL, INTAKE_LABEL] {
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

struct FakeResponder;

#[async_trait]
impl InteractiveResponder for FakeResponder {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        assert_eq!(request.profile_id.as_str(), "product-manager");
        assert_eq!(request.context["repository"], "ai/temper");
        assert!(
            request.context["transcript_url"]
                .as_str()
                .unwrap()
                .ends_with("/ai/temper/issues/1")
        );
        assert_eq!(request.turns.last().unwrap().body, "I want a chat MVP.");
        Ok(ConversationReply {
            message: "Let's file a small MVP.".into(),
            proposals: vec![Proposal::issue(
                proposal_id("terminal-chat-mvp"),
                IssueProposal::with_rationale(
                    "Add terminal product-manager chat MVP",
                    "Create a terminal MVP for product-manager chat.",
                    "A cheap dogfood loop comes first.",
                ),
            )?],
        })
    }
}

struct NeverResponder;

#[async_trait]
impl InteractiveResponder for NeverResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        panic!("resume tests must not call responder")
    }
}

#[test]
fn forge_marker_render_and_parse() {
    let marker = render_transcript_marker(MARKER_NAMESPACE, "pc-abc");
    let body = format!("hello\n\n{marker}");
    assert_eq!(
        parse_transcript_session_key(MARKER_NAMESPACE, &body),
        Some("pc-abc".into())
    );
    assert_eq!(
        acceptance_marker("pc-abc", "terminal-chat-mvp"),
        "<!-- temper:product-chat-file=pc-abc:terminal-chat-mvp -->"
    );
}

#[test]
fn forge_session_drives_transcript_and_idempotent_issue_acceptance() {
    temper_engine_io::block_on(async move {
        let (human, agent, _repo) = seeded().await;
        let mut session = ForgeInteractionSession::open(
            Arc::new(human.clone()),
            Arc::new(agent),
            Arc::new(FakeResponder),
            config(),
            ForgeSessionOpenOptions::new(
                "https://git.example.test",
                RepositoryPath::new("ai", "temper"),
            ),
        )
        .await
        .unwrap();
        assert_eq!(session.transcript_issue().labels, vec![TRANSCRIPT_LABEL]);

        let reply = session.send_human_turn("I want a chat MVP.").await.unwrap();
        assert_eq!(reply.proposals.len(), 1);
        let comments = human
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author_id, UserId::new("human"));
        assert_eq!(comments[1].author_id, UserId::new("product-manager"));

        let filed = session
            .accept_issue_proposal(&proposal_id("terminal-chat-mvp"))
            .await
            .unwrap();
        assert!(filed.created);
        assert_eq!(filed.issue.labels, vec![INTAKE_LABEL.to_string()]);
        assert!(filed.issue.body.contains("requested-by: human"));
        assert!(filed.issue.body.contains(&acceptance_marker(
            session.conversation_id().as_str(),
            "terminal-chat-mvp"
        )));
        let retry = session
            .accept_issue_proposal(&proposal_id("terminal-chat-mvp"))
            .await
            .unwrap();
        assert!(!retry.created);
        assert_eq!(retry.issue.number, filed.issue.number);
    })
}

#[test]
fn transcript_resume_refuses_non_transcript_label_policy() {
    temper_engine_io::block_on(async move {
        let (human, agent, repo) = seeded().await;
        let issue = human
            .create_issue(
                &repo.id,
                CreateIssue {
                    title: "Workflow issue".into(),
                    body: "not a transcript".into(),
                    labels: vec![INTAKE_LABEL.into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();

        let error = match ForgeInteractionSession::open(
            Arc::new(human),
            Arc::new(agent),
            Arc::new(NeverResponder),
            config(),
            ForgeSessionOpenOptions {
                base_url: "https://git.example.test".into(),
                repo_path: RepositoryPath::new("ai", "temper"),
                transcript_issue: Some(issue.number),
                context: json!({}),
            },
        )
        .await
        {
            Ok(_) => panic!("resume should reject workflow issue"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            InteractionError::TranscriptLabelMismatch { .. }
        ));
    })
}

#[test]
fn transcript_resume_reconstructs_recent_turns_by_author_identity() {
    temper_engine_io::block_on(async move {
        let (human, agent, repo) = seeded().await;
        let marker = render_transcript_marker(MARKER_NAMESPACE, "pc-existing");
        let issue = human
            .create_issue(
                &repo.id,
                CreateIssue {
                    title: "Product conversation".into(),
                    body: marker,
                    labels: vec![TRANSCRIPT_LABEL.into()],
                    assignees: Vec::new(),
                },
            )
            .await
            .unwrap();
        let other = human.as_user(user("random-user"));
        other
            .add_issue_comment(
                &issue.id,
                CreateComment {
                    body: "ignored".into(),
                },
            )
            .await
            .unwrap();
        human
            .add_issue_comment(
                &issue.id,
                CreateComment {
                    body: "first".into(),
                },
            )
            .await
            .unwrap();
        agent
            .add_issue_comment(
                &issue.id,
                CreateComment {
                    body: "second".into(),
                },
            )
            .await
            .unwrap();
        human
            .add_issue_comment(
                &issue.id,
                CreateComment {
                    body: "third".into(),
                },
            )
            .await
            .unwrap();

        let mut cfg = config();
        cfg.transcript = cfg.transcript.with_recent_turn_limit(2);
        let session = ForgeInteractionSession::open(
            Arc::new(human),
            Arc::new(agent),
            Arc::new(NeverResponder),
            cfg,
            ForgeSessionOpenOptions {
                base_url: "https://git.example.test".into(),
                repo_path: RepositoryPath::new("ai", "temper"),
                transcript_issue: Some(issue.number),
                context: json!({}),
            },
        )
        .await
        .unwrap();

        assert_eq!(
            session
                .turns()
                .iter()
                .map(|turn| turn.body.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "third"]
        );
    })
}
