use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use temper_forge::{
    CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel, User, UserId,
};
use temper_forge_memory::MemoryForge;
use temper_interaction::{
    CompiledInteractionSpec, ConversationReply, ConversationRequest, InteractionError,
    InteractiveResponder, IssueProposal, Proposal, ProposalId, RawInteractionSpec,
};

use crate::interaction_service::{
    HttpRequest, InteractionHttpApp, InteractionProfileRuntime, InteractionService,
};

struct ProfileResponder;

#[async_trait]
impl InteractiveResponder for ProfileResponder {
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        let id = format!("{}-proposal", request.profile_id);
        Ok(ConversationReply {
            message: format!("{} reply", request.profile_id),
            proposals: vec![Proposal::issue(
                ProposalId::new(id)?,
                IssueProposal::with_rationale(
                    format!("File {} work", request.profile_id),
                    format!("Body for {}", request.profile_id),
                    "Manifest-driven issue proposal",
                ),
            )?],
        })
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
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    for label in ["intake-chat", "support-chat", "untriaged"] {
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
    (human, forge.as_user(user("agent")), repo)
}

async fn fake_app(service_token: Option<&str>) -> InteractionHttpApp {
    let (human, agent, _repo) = seeded().await;
    let spec = compiled_two_profile_spec();
    let responder = Arc::new(ProfileResponder) as Arc<dyn InteractiveResponder>;
    let human = Arc::new(human) as Arc<dyn Forge>;
    let agent = Arc::new(agent) as Arc<dyn Forge>;
    let runtimes = spec
        .profiles()
        .iter()
        .map(|manifest| InteractionProfileRuntime {
            manifest: manifest.clone(),
            human_forge: Arc::clone(&human),
            agent_forge: Arc::clone(&agent),
            responder: Arc::clone(&responder),
        })
        .collect();
    let service = InteractionService::new(
        "https://git.example.test".into(),
        RepositoryPath::new("ai", "temper"),
        runtimes,
        None,
    )
    .unwrap();
    InteractionHttpApp::new(service, service_token.map(str::to_string))
}

fn json_request(method: &str, path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest::new(method, path, serde_json::to_vec(&body).unwrap())
        .with_header("content-type", "application/json")
}

fn response_json(response: &crate::interaction_service::HttpResponse) -> serde_json::Value {
    serde_json::from_str(response.body()).unwrap()
}

async fn create_conversation(app: &InteractionHttpApp, profile_id: &str) -> String {
    let response = app
        .handle_http_request(json_request(
            "POST",
            "/conversations",
            json!({ "profile_id": profile_id }),
        ))
        .await;
    assert_eq!(response.status(), 201);
    let body = response_json(&response);
    assert_eq!(body["profile_id"], profile_id);
    body["id"].as_str().unwrap().to_string()
}

#[test]
fn generic_http_routes_work_for_two_profiles() {
    temper_io_engine::block_on(async move {
        let app = fake_app(None).await;
        let intake = create_conversation(&app, "intake-agent").await;
        let support = create_conversation(&app, "support-agent").await;

        let intake_turn = app
            .handle_http_request(json_request(
                "POST",
                &format!("/conversations/{intake}/turns"),
                json!({ "body": "please file intake work" }),
            ))
            .await;
        assert_eq!(intake_turn.status(), 200);
        let intake_body = response_json(&intake_turn);
        assert_eq!(intake_body["reply"]["message"], "intake-agent reply");
        assert_eq!(
            intake_body["latest_proposals"][0]["id"],
            "intake-agent-proposal"
        );

        let support_turn = app
            .handle_http_request(json_request(
                "POST",
                &format!("/conversations/{support}/turns"),
                json!({ "body": "please file support work" }),
            ))
            .await;
        assert_eq!(support_turn.status(), 200);
        assert_eq!(
            response_json(&support_turn)["reply"]["message"],
            "support-agent reply"
        );

        let proposals = app
            .handle_http_request(HttpRequest::new(
                "GET",
                &format!("/conversations/{intake}/proposals"),
                Vec::new(),
            ))
            .await;
        assert_eq!(proposals.status(), 200);
        assert_eq!(
            response_json(&proposals)["proposals"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let accepted = app
            .handle_http_request(json_request(
                "POST",
                &format!("/conversations/{intake}/proposals/intake-agent-proposal/accept"),
                json!({}),
            ))
            .await;
        assert_eq!(accepted.status(), 200);
        let accepted_body = response_json(&accepted);
        assert_eq!(accepted_body["created"], true);
        assert_eq!(accepted_body["target"]["kind"], "issue");

        let events = app
            .handle_http_request(HttpRequest::new(
                "GET",
                &format!("/conversations/{intake}/events"),
                Vec::new(),
            ))
            .await;
        assert_eq!(events.status(), 200);
        let events_body = response_json(&events);
        assert_eq!(events_body["streaming"], false);
        assert!(events_body["events"].as_array().unwrap().len() >= 5);
    })
}

#[test]
fn generic_http_auth_protects_routes_when_configured() {
    temper_io_engine::block_on(async move {
        let app = fake_app(Some("service-secret")).await;
        let unauthenticated = app
            .handle_http_request(HttpRequest::new("GET", "/health", Vec::new()))
            .await;
        assert_eq!(unauthenticated.status(), 401);

        let authenticated = app
            .handle_http_request(
                HttpRequest::new("GET", "/health", Vec::new())
                    .with_header("authorization", "Bearer service-secret"),
            )
            .await;
        assert_eq!(authenticated.status(), 200);
    })
}

fn compiled_two_profile_spec() -> CompiledInteractionSpec {
    let raw: RawInteractionSpec = serde_json::from_value(json!({
        "id": "two-profile-interactions",
        "responders": [{
            "id": "generic-responder",
            "protocol": "process-v1",
            "required": true
        }],
        "profiles": [
            profile_json("intake-agent", "intake-chat"),
            profile_json("support-agent", "support-chat")
        ]
    }))
    .unwrap();
    raw.validate().unwrap().compile()
}

fn profile_json(id: &str, marker: &str) -> serde_json::Value {
    json!({
        "id": id,
        "transcript": {
            "target": "issue",
            "title_prefix": format!("{id} conversation"),
            "labels": [marker],
            "label_policy": "exact",
            "marker_namespace": marker,
            "recent_turn_limit": 30
        },
        "participants": {
            "human": { "display_name": "human" },
            "agent": { "display_name": "agent" }
        },
        "responder": "generic-responder",
        "proposal_kinds": [{ "id": "issue", "payload": "issue_draft" }],
        "commands": [{
            "id": "file-draft",
            "aliases": ["/file"],
            "action": {
                "accept_proposal": {
                    "kind": "issue",
                    "acceptance_action": "file-draft"
                }
            }
        }],
        "acceptance_actions": [{
            "id": "file-draft",
            "proposal_kind": "issue",
            "acceptance": { "policy": "explicit", "commands": ["file-draft"] },
            "idempotency_key": "${conversation.id}:${proposal.id}",
            "effects": [{
                "kind": "create_issue",
                "title": "${proposal.payload.title}",
                "body_template": "${proposal.payload.body}\n\n${effect.marker}",
                "labels": ["untriaged"],
                "marker_namespace": marker,
                "marker_key": "file"
            }]
        }]
    })
}
