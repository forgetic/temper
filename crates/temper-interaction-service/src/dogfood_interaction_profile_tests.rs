use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use temper_forge::{CreateRepository, Forge, RepositoryPath, UpsertLabel, User, UserId};
use temper_forge_memory::MemoryForge;
use temper_interaction::{
    AcceptanceEffect, CommandActionManifest, CompiledProfileManifest, ConversationReply,
    ConversationRequest, ForgeInteractionSession, ForgeSessionConfig, ForgeSessionOpenOptions,
    InteractionError, InteractiveResponder, IssueProposal, Proposal, ProposalId,
    RawInteractionSpec,
};

const DOGFOOD_PRODUCT_MANAGER_SPEC: &str =
    include_str!("../../temper-interaction/fixtures/product-manager-interaction-spec.json");

struct FixtureResponder;

#[async_trait]
impl InteractiveResponder for FixtureResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        Ok(ConversationReply {
            message: "Let's file a small MVP.".into(),
            proposals: vec![Proposal::issue(
                ProposalId::new("terminal-chat-mvp")?,
                IssueProposal::with_rationale(
                    "Add terminal product-manager chat MVP",
                    "Create a terminal MVP for product-manager chat.",
                    "A cheap dogfood loop comes first.",
                ),
            )?],
        })
    }
}

#[test]
fn dogfood_fixture_product_manager_profile_validates_and_compiles() {
    let profile = compiled_profile();

    assert_eq!(profile.profile.id.as_str(), "product-manager");
    assert_eq!(profile.responder.id.as_str(), "product-manager-responder");
    assert_eq!(profile.transcript.labels, ["product"]);
    assert_eq!(profile.transcript.title_prefix, "Product conversation");
    assert_eq!(profile.transcript.marker_namespace.as_str(), "product-chat");
    assert_eq!(profile.commands[0].aliases, ["/file"]);

    match &profile.commands[0].action {
        CommandActionManifest::AcceptProposal {
            proposal_kind,
            acceptance_action,
        } => {
            assert_eq!(proposal_kind.as_str(), "issue");
            assert_eq!(acceptance_action.as_str(), "file-draft");
        }
    }

    let action = &profile.acceptance_actions[0];
    assert_eq!(action.proposal_kind.as_str(), "issue");
    let AcceptanceEffect::CreateIssue(effect) = &action.effects[0] else {
        panic!("dogfood accepted action should create an issue");
    };
    assert_eq!(effect.labels(), ["untriaged"]);
    assert_eq!(effect.marker_namespace(), "product-chat");
    assert_eq!(effect.marker_key(), Some("file"));
}

#[test]
fn dogfood_fixture_runs_through_generic_session_and_acceptance() {
    temper_io_engine::block_on(async move {
        let profile = compiled_profile();
        let (human, agent) = seeded(&profile).await;
        let human_reader = human.clone();
        let mut session = ForgeInteractionSession::open(
            Arc::new(human) as Arc<dyn Forge>,
            Arc::new(agent) as Arc<dyn Forge>,
            Arc::new(FixtureResponder) as Arc<dyn InteractiveResponder>,
            ForgeSessionConfig::from_profile_manifest(&profile).unwrap(),
            ForgeSessionOpenOptions {
                base_url: "https://git.example.test".into(),
                repo_path: RepositoryPath::new("ai", "temper"),
                transcript_issue: None,
                context: json!({}),
            },
        )
        .await
        .unwrap();

        assert_eq!(session.transcript_issue().labels, profile.transcript.labels);

        let reply = session.send_human_turn("I want a chat MVP.").await.unwrap();
        assert_eq!(reply.message, "Let's file a small MVP.");
        assert_eq!(reply.proposals[0].id.as_str(), "terminal-chat-mvp");

        let comments = human_reader
            .list_issue_comments(&session.transcript_issue().id)
            .await
            .unwrap();
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].author_id, UserId::new(human_handle(&profile)));
        assert_eq!(comments[1].author_id, UserId::new(agent_handle(&profile)));

        let proposal_id = ProposalId::new("terminal-chat-mvp").unwrap();
        let action_id = &profile.acceptance_actions[0].id;
        let filed = session
            .accept_issue_proposal_with_action(&proposal_id, Some(action_id))
            .await
            .unwrap();
        assert!(filed.created);
        assert_eq!(filed.issue.labels, accepted_issue_labels(&profile));
        assert!(filed.issue.body.contains("requested-by: human"));
        assert!(filed
            .issue
            .body
            .contains(&format!("Transcript: {}", session.transcript_url())));

        let retry = session
            .accept_issue_proposal_with_action(&proposal_id, Some(action_id))
            .await
            .unwrap();
        assert!(!retry.created);
        assert_eq!(retry.issue.number, filed.issue.number);
    })
}

fn compiled_profile() -> CompiledProfileManifest {
    let raw: RawInteractionSpec = serde_json::from_str(DOGFOOD_PRODUCT_MANAGER_SPEC).unwrap();
    raw.validate()
        .unwrap()
        .compile()
        .profiles()
        .first()
        .cloned()
        .expect("profile compiles")
}

async fn seeded(profile: &CompiledProfileManifest) -> (MemoryForge, MemoryForge) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user(&human_handle(profile)));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    let labels = profile
        .transcript
        .labels
        .iter()
        .cloned()
        .chain(accepted_issue_labels(profile));
    for label in labels {
        human
            .upsert_label(
                &repo.id,
                UpsertLabel {
                    name: label,
                    color: Some("ededed".into()),
                    description: None,
                },
            )
            .await
            .unwrap();
    }
    (human, forge.as_user(user(&agent_handle(profile))))
}

fn user(handle: &str) -> User {
    User {
        id: UserId::new(handle),
        handle: handle.to_string(),
        display_name: None,
        email: None,
    }
}

fn human_handle(profile: &CompiledProfileManifest) -> String {
    profile
        .profile
        .human_participant
        .display_name
        .clone()
        .unwrap_or_else(|| "human".into())
}

fn agent_handle(profile: &CompiledProfileManifest) -> String {
    profile
        .profile
        .agent_participant
        .display_name
        .clone()
        .unwrap_or_else(|| "agent".into())
}

fn accepted_issue_labels(profile: &CompiledProfileManifest) -> Vec<String> {
    profile
        .acceptance_actions
        .iter()
        .flat_map(|action| &action.effects)
        .find_map(|effect| match effect {
            AcceptanceEffect::CreateIssue(effect) => Some(effect.labels().to_vec()),
            AcceptanceEffect::AddTranscriptComment(_) => None,
        })
        .unwrap_or_default()
}
