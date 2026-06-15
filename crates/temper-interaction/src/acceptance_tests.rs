use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use temper_forge::{
    CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel, User, UserId,
};
use temper_forge_memory::MemoryForge;

use crate::{
    ConversationReply, ConversationRequest, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, InteractionError, InteractiveResponder, IssueProposal, Proposal,
    ProposalId, ProposalKind, RawInteractionSpec,
};

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

fn proposal_id(value: &str) -> ProposalId {
    ProposalId::new(value).expect("valid proposal id")
}

fn support_manifest() -> crate::CompiledProfileManifest {
    let raw: RawInteractionSpec = serde_json::from_value(json!({
        "id": "support-interactions",
        "responders": [{
            "id": "support-responder",
            "protocol": "process-v1",
            "required": true
        }],
        "profiles": [{
            "id": "support-agent",
            "transcript": {
                "target": "issue",
                "title_prefix": "Support conversation",
                "labels": ["support-transcript"],
                "label_policy": "exact",
                "marker_namespace": "support-chat",
                "recent_turn_limit": 30
            },
            "participants": {
                "human": { "display_name": "customer" },
                "agent": { "display_name": "support-agent" }
            },
            "responder": "support-responder",
            "proposal_kinds": [{
                "id": "issue",
                "payload": "issue_draft"
            }],
            "commands": [{
                "id": "accept-issue",
                "aliases": ["/accept"],
                "action": {
                    "accept_proposal": {
                        "kind": "issue",
                        "acceptance_action": "accept-issue"
                    }
                }
            }],
            "acceptance_actions": [{
                "id": "accept-issue",
                "proposal_kind": "issue",
                "acceptance": {
                    "policy": "explicit",
                    "commands": ["accept-issue"]
                },
                "idempotency_key": "${conversation.id}:${proposal.id}",
                "effects": [{
                    "kind": "create_issue",
                    "title": "${proposal.payload.title}",
                    "body_template": "${proposal.payload.body}\n\n${effect.marker}",
                    "labels": ["support-intake", "proposal-${proposal.id}"],
                    "assignees": ["${human.handle}"],
                    "marker_namespace": "support-chat",
                    "backlink": {
                        "label": "Transcript",
                        "url": "${conversation.transcript_url}"
                    }
                }, {
                    "kind": "add_transcript_comment",
                    "body_template": "Accepted ${proposal.id}\n\n${effect.marker}",
                    "marker_namespace": "support-chat"
                }]
            }]
        }]
    }))
    .expect("raw support spec shape");
    raw.validate()
        .expect("support spec validates")
        .compile()
        .profiles()[0]
        .clone()
}

fn product_manifest() -> crate::CompiledProfileManifest {
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

async fn seeded(
    manifest: &crate::CompiledProfileManifest,
) -> (MemoryForge, MemoryForge, Repository) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user(
        manifest
            .profile
            .human_participant
            .display_name
            .as_deref()
            .unwrap_or("human"),
    ));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    for label in &manifest.transcript.labels {
        human
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label.clone(),
                    color: Some("ededed".into()),
                    description: None,
                },
            )
            .await
            .unwrap();
    }
    let agent = forge.as_user(user(
        manifest
            .profile
            .agent_participant
            .display_name
            .as_deref()
            .unwrap_or("agent"),
    ));
    (human, agent, repo)
}

struct StaticResponder {
    reply: ConversationReply,
}

#[async_trait]
impl InteractiveResponder for StaticResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        Ok(self.reply.clone())
    }
}

struct NeverResponder;

#[async_trait]
impl InteractiveResponder for NeverResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        panic!("resume acceptance must not call the responder")
    }
}

fn issue_reply() -> ConversationReply {
    ConversationReply {
        message: "I can file that.".into(),
        proposals: vec![
            Proposal::issue(
                proposal_id("support-mvp"),
                IssueProposal::with_rationale("Support MVP", "Build a support MVP.", "priority"),
            )
            .unwrap(),
        ],
    }
}

async fn open_session<R: InteractiveResponder + ?Sized>(
    human: MemoryForge,
    agent: MemoryForge,
    responder: Arc<R>,
    manifest: crate::CompiledProfileManifest,
    transcript_issue: Option<temper_forge::ItemNumber>,
) -> ForgeInteractionSession<MemoryForge, MemoryForge, R> {
    let config = ForgeSessionConfig::from_profile_manifest(&manifest).unwrap();
    ForgeInteractionSession::open(
        Arc::new(human),
        Arc::new(agent),
        responder,
        config,
        ForgeSessionOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "temper"),
            transcript_issue,
            context: json!({}),
        },
    )
    .await
    .unwrap()
}

#[test]
fn arbitrary_profile_issue_creation_acceptance_uses_manifest_effects() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human.clone(),
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;

        session
            .send_human_turn("Please file support work.")
            .await
            .unwrap();
        let outcome = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert!(outcome.created);
        assert_eq!(outcome.issue.title, "Support MVP");
        assert_eq!(
            outcome.issue.labels,
            ["proposal-support-mvp", "support-intake"]
        );
        assert_eq!(outcome.issue.assignees, [UserId::new("customer")]);
        assert!(outcome.issue.body.contains("Build a support MVP."));
        assert!(
            outcome
                .issue
                .body
                .contains("Transcript: https://git.example.test/ai/temper/issues/1")
        );
        assert!(
            outcome
                .issue
                .body
                .contains("temper:support-chat-accept-issue=")
        );

        let comments = human
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        assert!(
            comments
                .iter()
                .any(|comment| comment.body.contains("Accepted support-mvp"))
        );
    })
}

#[test]
fn product_manager_fixture_acceptance_preserves_filed_issue_shape() {
    temper_engine_io::block_on(async move {
        let manifest = product_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human,
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;

        session
            .send_human_turn("Please file product work.")
            .await
            .unwrap();
        let outcome = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert_eq!(outcome.issue.labels, ["untriaged"]);
        assert!(
            outcome
                .issue
                .body
                .contains("Transcript: https://git.example.test/ai/temper/issues/1")
        );
        assert!(outcome.issue.body.contains("requested-by: human"));
        assert!(outcome.issue.body.contains("temper:product-chat-file="));
    })
}

#[test]
fn acceptance_retry_is_idempotent_for_issue_and_comment_effects() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut session = open_session(
            human.clone(),
            agent,
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest,
            None,
        )
        .await;
        session
            .send_human_turn("Please file support work.")
            .await
            .unwrap();

        let first = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();
        let second = session
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();

        assert!(first.created);
        assert!(!second.created);
        assert_eq!(first.issue.number, second.issue.number);
        let comments = human
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        let acceptance_comments = comments
            .iter()
            .filter(|comment| comment.body.contains("Accepted support-mvp"))
            .count();
        assert_eq!(acceptance_comments, 1);
    })
}

#[test]
fn restart_resume_reconstructs_latest_proposals_and_accepts() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let mut first = open_session(
            human.clone(),
            agent.clone(),
            Arc::new(StaticResponder {
                reply: issue_reply(),
            }),
            manifest.clone(),
            None,
        )
        .await;
        first
            .send_human_turn("Please file support work.")
            .await
            .unwrap();
        let transcript_number = first.transcript_issue().number;
        drop(first);

        let resumed = open_session(
            human,
            agent,
            Arc::new(NeverResponder),
            manifest,
            Some(transcript_number),
        )
        .await;

        assert_eq!(resumed.latest_proposals()[0].id.as_str(), "support-mvp");
        let outcome = resumed
            .accept_issue_proposal(&proposal_id("support-mvp"))
            .await
            .unwrap();
        assert!(outcome.created);
    })
}

#[test]
fn unsupported_proposal_kind_is_rejected_before_persistence() {
    temper_engine_io::block_on(async move {
        let manifest = support_manifest();
        let (human, agent, _repo) = seeded(&manifest).await;
        let reply = ConversationReply {
            message: "custom proposal".into(),
            proposals: vec![Proposal::custom(
                proposal_id("custom-work"),
                ProposalKind::new("custom-kind").unwrap(),
                "Custom work",
                None,
                json!({ "title": "Custom" }),
            )],
        };
        let mut session = open_session(
            human,
            agent,
            Arc::new(StaticResponder { reply }),
            manifest,
            None,
        )
        .await;
        let error = session
            .send_human_turn("Please file custom work.")
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            InteractionError::UnsupportedProposalKind { .. }
        ));
    })
}

#[test]
fn responder_boundary_exposes_no_forge_mutation_handle() {
    assert!(!include_str!("agent.rs").contains("temper_forge"));
    assert!(!include_str!("process.rs").contains("temper_forge"));
}
