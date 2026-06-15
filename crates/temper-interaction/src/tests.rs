use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use temper_forge::{
    CreateComment, CreateIssue, CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel,
    User, UserId,
};
use temper_forge_memory::MemoryForge;

use crate::{
    ConversationId, ConversationProfileId, ConversationReply, ConversationRequest,
    ConversationTurn, ForgeInteractionSession, ForgeSessionConfig, ForgeSessionOpenOptions,
    InteractionError, InteractionProtocolError, InteractiveResponder, IssueProposal, Participant,
    Proposal, ProposalId, ProposalKind, RawInteractionSpec, is_valid_proposal_slug,
    parse_transcript_session_key, render_acceptance_marker, render_transcript_marker,
};

const TRANSCRIPT_LABEL: &str = "product";
const INTAKE_LABEL: &str = "untriaged";
const MARKER_NAMESPACE: &str = "product-chat";

fn proposal_id(value: &str) -> ProposalId {
    ProposalId::new(value).expect("valid proposal id")
}

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

fn product_profile_manifest() -> crate::CompiledProfileManifest {
    let raw: RawInteractionSpec = serde_json::from_str(include_str!(
        "../fixtures/product-manager-interaction-spec.json"
    ))
    .expect("fixture deserializes");
    raw.validate()
        .expect("fixture validates")
        .compile()
        .profiles()[0]
        .clone()
}

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
fn process_boundary_request_and_reply_json_round_trip() {
    let request_json = include_str!("../fixtures/interactive-responder-request.json");
    let request: ConversationRequest =
        serde_json::from_str(request_json).expect("process request deserializes");
    assert_eq!(request.profile_id.as_str(), "product-manager");
    assert_eq!(request.conversation_id.as_str(), "conversation-1");
    assert_eq!(request.turns[0].id.as_ref().unwrap().as_str(), "turn-1");

    let encoded_request = serde_json::to_string(&request).expect("request serializes");
    let decoded_request: ConversationRequest =
        serde_json::from_str(&encoded_request).expect("request round-trips");
    assert_eq!(decoded_request, request);

    let reply_json = include_str!("../fixtures/interactive-responder-reply.json");
    let reply: ConversationReply =
        serde_json::from_str(reply_json).expect("process reply deserializes");
    reply.validate().expect("reply validates");

    let issue = IssueProposal::with_rationale(
        "Add mobile chat adapter",
        "Expose the interaction service through a mobile-friendly adapter.",
        "Mobile access lets humans keep the conversation moving.",
    );
    assert_eq!(
        reply.proposals[0]
            .issue_payload()
            .expect("issue payload decodes"),
        Some(issue)
    );

    let encoded_reply = serde_json::to_string_pretty(&reply).expect("reply serializes");
    assert!(encoded_reply.contains("mobile-chat-adapter"));
    assert!(encoded_reply.contains("issue"));
    let decoded_reply: ConversationReply =
        serde_json::from_str(&encoded_reply).expect("reply round-trips");
    assert_eq!(decoded_reply, reply);
}

#[test]
fn validates_deterministic_proposal_slugs() {
    for slug in ["mvp", "matrix-text-adapter", "api-v1", "a1-b2"] {
        assert!(is_valid_proposal_slug(slug), "{slug} should be valid");
        ProposalId::new(slug).expect("valid proposal id");
        ProposalKind::new(slug).expect("valid proposal kind");
    }

    for slug in [
        "",
        "Matrix",
        "matrix_text",
        "matrix--text",
        "-matrix",
        "matrix-",
        "matrix text",
        "mátřix",
    ] {
        assert!(!is_valid_proposal_slug(slug), "{slug} should be invalid");
        assert!(matches!(
            ProposalId::new(slug),
            Err(InteractionProtocolError::InvalidSlug { .. })
        ));
    }

    let too_long = "a".repeat(81);
    assert!(!is_valid_proposal_slug(&too_long));
}

#[test]
fn rejects_invalid_ids_during_deserialization() {
    let json = r#"{
        "message": "bad id",
        "proposals": [{
            "id": "bad_id",
            "kind": "issue",
            "title": "Bad id",
            "summary": null,
            "payload": {}
        }]
    }"#;

    let error = serde_json::from_str::<ConversationReply>(json).expect_err("invalid id fails");
    assert!(error.to_string().contains("invalid proposal id"));
}

#[test]
fn rejects_malformed_issue_proposal_payloads() {
    let reply = ConversationReply {
        message: "bad payload".to_string(),
        proposals: vec![Proposal::custom(
            proposal_id("bad-issue"),
            ProposalKind::issue(),
            "Bad issue",
            None,
            json!({}),
        )],
    };

    assert!(matches!(
        reply.validate(),
        Err(InteractionProtocolError::Json(_))
    ));
}

#[test]
fn rejects_duplicate_proposal_ids() {
    let first = Proposal::custom(
        proposal_id("same-proposal"),
        ProposalKind::issue(),
        "First",
        None,
        json!({}),
    );
    let second = Proposal::custom(
        proposal_id("same-proposal"),
        ProposalKind::new("custom-kind").expect("valid kind"),
        "Second",
        None,
        json!({}),
    );
    let reply = ConversationReply {
        message: "duplicate".to_string(),
        proposals: vec![first, second],
    };

    assert!(matches!(
        reply.validate(),
        Err(InteractionProtocolError::DuplicateProposalId { .. })
    ));
}

struct EchoResponder;

#[async_trait]
impl InteractiveResponder for EchoResponder {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        Ok(ConversationReply::message(format!(
            "{}:{} turn(s)",
            request.profile_id,
            request.turns.len()
        )))
    }
}

fn accepts_trait_object(_responder: &dyn InteractiveResponder) {}

#[test]
fn interactive_responder_is_object_safe() {
    let responder: Box<dyn InteractiveResponder> = Box::new(EchoResponder);
    accepts_trait_object(responder.as_ref());

    let request = ConversationRequest::new(
        ConversationProfileId::new("echo-profile").expect("valid profile"),
        ConversationId::new("conversation-1").expect("valid conversation"),
        vec![ConversationTurn::new(Participant::human("human"), "hello")],
    );
    let future = responder.respond(&request);
    drop(future);
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
