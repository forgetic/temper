use std::sync::Arc;

use async_trait::async_trait;
use temper_forge::{
    CreateRepository, Forge, Repository, RepositoryPath, UpsertLabel, User, UserId,
};
use temper_forge_memory::MemoryForge;

use temper_interaction::{
    AcceptanceEffect, ConversationReply, ConversationRequest, CreateIssueEffect, InteractionError,
    InteractiveResponder,
};

use crate::product_chat::{
    parse_transcript_session_key, product_profile_manifest, render_filing_marker,
    render_transcript_marker, ProductChatError, ProductChatOpenOptions, ProductChatSession,
    ProductManagerDraftIssue, ProductManagerResponse,
};
use crate::product_chat_args::{
    parse_with_env, ParseOutcome, DEFAULT_SERVICE_BIND, HUMAN_TOKEN_ENV,
    PROCESS_RESPONDER_COMMAND_ENV, PRODUCT_MANAGER_TOKEN_ENV, SERVICE_TOKEN_ENV,
};
use crate::product_chat_service::{HttpRequest, ProductChatHttpApp, ProductChatService};

struct FakeResponder {
    response: ProductManagerResponse,
}

struct NeverResponder;

#[async_trait]
impl InteractiveResponder for FakeResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        self.response.to_conversation_reply()
    }
}

#[async_trait]
impl InteractiveResponder for NeverResponder {
    async fn respond(
        &self,
        _request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError> {
        panic!("local product-chat command should not call the responder")
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

fn product_transcript_labels() -> Vec<String> {
    product_profile_manifest().unwrap().transcript.labels
}

fn product_issue_effect() -> CreateIssueEffect {
    let manifest = product_profile_manifest().unwrap();
    let effect = manifest.acceptance_actions[0].effects[0].clone();
    let AcceptanceEffect::CreateIssue(effect) = effect;
    effect
}

fn product_issue_labels() -> Vec<String> {
    product_issue_effect().labels().to_vec()
}

fn product_marker_namespace() -> String {
    product_issue_effect().marker_namespace().to_string()
}

fn product_profile_id() -> String {
    product_profile_manifest().unwrap().profile.id.to_string()
}

fn product_human_handle() -> String {
    product_profile_manifest()
        .unwrap()
        .profile
        .human_participant
        .display_name
        .unwrap_or_default()
}

fn product_agent_handle() -> String {
    product_profile_manifest()
        .unwrap()
        .profile
        .agent_participant
        .display_name
        .unwrap_or_default()
}

async fn seeded() -> (MemoryForge, MemoryForge, Repository) {
    let forge = MemoryForge::new();
    let human = forge.as_user(user(&product_human_handle()));
    let repo = human
        .create_repository(CreateRepository {
            owner: "ai".into(),
            name: "temper".into(),
            default_branch: "main".into(),
            description: None,
        })
        .await
        .unwrap();
    let labels = product_transcript_labels()
        .into_iter()
        .chain(product_issue_labels());
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
    (human, forge.as_user(user(&product_agent_handle())), repo)
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

fn product_chat_env(key: &str) -> Option<String> {
    match key {
        HUMAN_TOKEN_ENV => Some("human-secret".into()),
        PRODUCT_MANAGER_TOKEN_ENV => Some("pm-secret".into()),
        PROCESS_RESPONDER_COMMAND_ENV => Some("/opt/respond".into()),
        _ => None,
    }
}

async fn fake_service_app(service_token: Option<&str>) -> ProductChatHttpApp {
    service_app_with_responder(service_token, Arc::new(fake_responder())).await
}

async fn service_app_with_responder(
    service_token: Option<&str>,
    responder: Arc<dyn InteractiveResponder>,
) -> ProductChatHttpApp {
    let (human, product_manager, _repo) = seeded().await;
    let service = ProductChatService::new(
        "https://git.example.test".into(),
        RepositoryPath::new("ai", "temper"),
        Arc::new(human) as Arc<dyn Forge>,
        Arc::new(product_manager) as Arc<dyn Forge>,
        responder,
    );
    ProductChatHttpApp::new(service, service_token.map(str::to_string))
}

fn json_request(method: &str, path: &str, body: serde_json::Value) -> HttpRequest {
    HttpRequest::new(method, path, serde_json::to_vec(&body).unwrap())
        .with_header("content-type", "application/json")
}

fn response_json(response: &crate::product_chat_service::HttpResponse) -> serde_json::Value {
    serde_json::from_str(response.body()).unwrap()
}

async fn create_service_session(app: &ProductChatHttpApp) -> String {
    let response = app
        .handle_http_request(json_request("POST", "/sessions", serde_json::json!({})))
        .await;
    assert_eq!(response.status(), 201);
    response_json(&response)["id"].as_str().unwrap().to_string()
}

async fn create_service_conversation(app: &ProductChatHttpApp) -> String {
    let response = app
        .handle_http_request(json_request(
            "POST",
            "/conversations",
            serde_json::json!({ "profile_id": product_profile_id() }),
        ))
        .await;
    assert_eq!(response.status(), 201);
    response_json(&response)["id"].as_str().unwrap().to_string()
}

#[test]
fn product_chat_markers_render_and_parse() {
    let marker = render_transcript_marker("pc-abc");
    let body = format!("hello\n\n{marker}");
    assert_eq!(parse_transcript_session_key(&body), Some("pc-abc".into()));
    assert_eq!(
        render_filing_marker("pc-abc", "terminal-chat-mvp"),
        format!(
            "<!-- temper:{}-file=pc-abc:terminal-chat-mvp -->",
            product_marker_namespace()
        )
    );
}

#[tokio::test]
async fn product_chat_core_drives_transcript_and_idempotent_filing() {
    let (human, product_manager, _repo) = seeded().await;
    let responder = Arc::new(fake_responder());
    let human_session = Arc::new(human.clone());
    let product_session = Arc::new(product_manager);
    let mut session = ProductChatSession::open(
        human_session,
        product_session,
        responder,
        ProductChatOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "temper"),
            transcript_issue: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        session.transcript_issue().labels,
        product_transcript_labels()
    );

    let response = session.send_human_turn("I want a chat MVP.").await.unwrap();
    assert_eq!(response.drafts.len(), 1);
    let comments = human
        .list_issue_comments(&session.transcript_issue().id)
        .await
        .unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0].author_id, UserId::new(product_human_handle()));
    assert_eq!(comments[1].author_id, UserId::new(product_agent_handle()));

    let filed = session.file_draft(1).await.unwrap();
    assert!(filed.created);
    assert_eq!(filed.issue.labels, product_issue_labels());
    assert!(filed
        .issue
        .body
        .contains(&format!("requested-by: {}", product_human_handle())));
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
    let responder = Arc::new(fake_responder());
    let session = ProductChatSession::open(
        Arc::new(human),
        Arc::new(product_manager),
        responder,
        ProductChatOpenOptions {
            base_url: "https://git.example.test".into(),
            repo_path: RepositoryPath::new("ai", "temper"),
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

#[test]
fn product_chat_serve_args_default_to_loopback_bind() {
    let outcome = parse_with_env(
        [
            "serve",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        product_chat_env,
    )
    .expect("parses");
    let ParseOutcome::Serve(args) = outcome else {
        panic!("expected serve")
    };
    assert_eq!(args.bind.to_string(), DEFAULT_SERVICE_BIND);
    assert!(args.bind.ip().is_loopback());
}

#[test]
fn product_chat_non_loopback_bind_requires_opt_in_and_service_token() {
    let error = parse_with_env(
        [
            "serve",
            "--bind",
            "0.0.0.0:39200",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        product_chat_env,
    )
    .unwrap_err();
    assert!(error.to_string().contains("--allow-non-loopback"));

    let error = parse_with_env(
        [
            "serve",
            "--bind",
            "0.0.0.0:39200",
            "--allow-non-loopback",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        product_chat_env,
    )
    .unwrap_err();
    assert!(error.to_string().contains(SERVICE_TOKEN_ENV));

    let outcome = parse_with_env(
        [
            "serve",
            "--bind",
            "0.0.0.0:39200",
            "--allow-non-loopback",
            "--base-url",
            "https://git.example.test",
            "--repo",
            "ai/temper",
        ]
        .into_iter()
        .map(String::from),
        |key| match key {
            SERVICE_TOKEN_ENV => Some("service-secret".into()),
            other => product_chat_env(other),
        },
    )
    .expect("non-loopback is explicit and authenticated");
    let ParseOutcome::Serve(args) = outcome else {
        panic!("expected serve")
    };
    assert_eq!(args.service_token.as_deref(), Some("service-secret"));
}

#[tokio::test]
async fn product_chat_service_rejects_unauthenticated_request_when_token_configured() {
    let app = fake_service_app(Some("service-secret")).await;
    let response = app
        .handle_http_request(HttpRequest::new("GET", "/health", Vec::<u8>::new()))
        .await;
    assert_eq!(response.status(), 401);

    let response = app
        .handle_http_request(json_request(
            "POST",
            "/conversations",
            serde_json::json!({}),
        ))
        .await;
    assert_eq!(response.status(), 401);

    let response = app
        .handle_http_request(
            HttpRequest::new("GET", "/health", Vec::<u8>::new())
                .with_header("authorization", "Bearer service-secret"),
        )
        .await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn product_chat_service_post_sessions_creates_session_response() {
    let app = fake_service_app(None).await;
    let response = app
        .handle_http_request(json_request("POST", "/sessions", serde_json::json!({})))
        .await;
    assert_eq!(response.status(), 201);
    let body = response_json(&response);
    assert!(body["id"]
        .as_str()
        .unwrap()
        .starts_with(&format!("{}-", product_profile_id())));
    assert_eq!(body["transcript_issue"].as_u64(), Some(1));
    assert_eq!(
        body["transcript_url"].as_str(),
        Some("https://git.example.test/ai/temper/issues/1")
    );
}

#[tokio::test]
async fn product_chat_service_post_messages_returns_reply_and_drafts() {
    let app = fake_service_app(None).await;
    let session_id = create_service_session(&app).await;

    let response = app
        .handle_http_request(json_request(
            "POST",
            &format!("/sessions/{session_id}/messages"),
            serde_json::json!({ "message": "I want a chat MVP." }),
        ))
        .await;

    assert_eq!(response.status(), 200);
    let body = response_json(&response);
    assert_eq!(body["reply"].as_str(), Some("Let's file a small MVP."));
    assert_eq!(
        body["drafts"][0]["slug"].as_str(),
        Some("terminal-chat-mvp")
    );
    assert_eq!(
        body["transcript_url"].as_str(),
        Some("https://git.example.test/ai/temper/issues/1")
    );
}

#[tokio::test]
async fn product_chat_service_help_command_is_local() {
    let app = service_app_with_responder(None, Arc::new(NeverResponder)).await;
    let session_id = create_service_session(&app).await;

    let response = app
        .handle_http_request(json_request(
            "POST",
            &format!("/sessions/{session_id}/messages"),
            serde_json::json!({ "message": "/help" }),
        ))
        .await;

    assert_eq!(response.status(), 200);
    let body = response_json(&response);
    assert_eq!(
        body["reply"].as_str(),
        Some("Commands: /drafts, /file <n>, /issue, /help, /quit")
    );
}

#[tokio::test]
async fn product_chat_service_file_draft_returns_existing_issue_on_repeated_calls() {
    let app = fake_service_app(None).await;
    let session_id = create_service_session(&app).await;
    let message_path = format!("/sessions/{session_id}/messages");
    app.handle_http_request(json_request(
        "POST",
        &message_path,
        serde_json::json!({ "message": "I want a chat MVP." }),
    ))
    .await;
    let file_path = format!("/sessions/{session_id}/drafts/terminal-chat-mvp/file");

    let first = app
        .handle_http_request(HttpRequest::new("POST", &file_path, Vec::<u8>::new()))
        .await;
    let second = app
        .handle_http_request(HttpRequest::new("POST", &file_path, Vec::<u8>::new()))
        .await;

    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 200);
    let first = response_json(&first);
    let second = response_json(&second);
    assert_eq!(first["created"].as_bool(), Some(true));
    assert_eq!(second["created"].as_bool(), Some(false));
    assert_eq!(first["issue"]["number"], second["issue"]["number"]);
    assert_eq!(
        first["issue"]["url"].as_str(),
        Some("https://git.example.test/ai/temper/issues/2")
    );
}

#[tokio::test]
async fn product_chat_service_generic_routes_emit_events_and_accept_idempotently() {
    let app = fake_service_app(None).await;
    let conversation_id = create_service_conversation(&app).await;

    let conversation = app
        .handle_http_request(HttpRequest::new(
            "GET",
            &format!("/conversations/{conversation_id}"),
            Vec::<u8>::new(),
        ))
        .await;
    assert_eq!(conversation.status(), 200);
    let conversation = response_json(&conversation);
    assert_eq!(
        conversation["profile_id"].as_str().map(str::to_string),
        Some(product_profile_id())
    );
    assert_eq!(conversation["transcript"]["issue_number"].as_u64(), Some(1));

    let turn = app
        .handle_http_request(json_request(
            "POST",
            &format!("/conversations/{conversation_id}/turns"),
            serde_json::json!({ "body": "I want a chat MVP." }),
        ))
        .await;
    assert_eq!(turn.status(), 200);
    let turn = response_json(&turn);
    assert_eq!(
        turn["reply"]["message"].as_str(),
        Some("Let's file a small MVP.")
    );
    assert_eq!(
        turn["latest_proposals"][0]["id"].as_str(),
        Some("terminal-chat-mvp")
    );

    let proposals = app
        .handle_http_request(HttpRequest::new(
            "GET",
            &format!("/conversations/{conversation_id}/proposals"),
            Vec::<u8>::new(),
        ))
        .await;
    assert_eq!(proposals.status(), 200);
    assert_eq!(
        response_json(&proposals)["proposals"][0]["id"].as_str(),
        Some("terminal-chat-mvp")
    );

    let accept_path =
        format!("/conversations/{conversation_id}/proposals/terminal-chat-mvp/accept");
    let first = app
        .handle_http_request(HttpRequest::new("POST", &accept_path, Vec::<u8>::new()))
        .await;
    let second = app
        .handle_http_request(HttpRequest::new("POST", &accept_path, Vec::<u8>::new()))
        .await;
    assert_eq!(first.status(), 200);
    assert_eq!(second.status(), 200);
    let first = response_json(&first);
    let second = response_json(&second);
    assert_eq!(first["created"].as_bool(), Some(true));
    assert_eq!(second["created"].as_bool(), Some(false));
    assert_eq!(first["target"]["number"], second["target"]["number"]);

    let events = app
        .handle_http_request(HttpRequest::new(
            "GET",
            &format!("/conversations/{conversation_id}/events"),
            Vec::<u8>::new(),
        ))
        .await;
    assert_eq!(events.status(), 200);
    let events = response_json(&events);
    assert_eq!(events["streaming"].as_bool(), Some(false));
    let kinds = events["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"conversation_opened"));
    assert!(kinds.contains(&"human_turn_appended"));
    assert!(kinds.contains(&"agent_reply_appended"));
    assert!(kinds.contains(&"proposals_updated"));
    assert!(kinds.contains(&"proposal_accepted"));
}

#[tokio::test]
async fn product_chat_service_generic_route_rejects_unconfigured_profile() {
    let app = fake_service_app(None).await;
    let response = app
        .handle_http_request(json_request(
            "POST",
            "/conversations",
            serde_json::json!({ "profile_id": "other-profile" }),
        ))
        .await;

    assert_eq!(response.status(), 400);
}
