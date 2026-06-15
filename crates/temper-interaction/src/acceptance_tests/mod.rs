mod effects;
mod resume;

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
    ProposalId, RawInteractionSpec,
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
        "../../fixtures/product-manager-interaction-spec.json"
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
