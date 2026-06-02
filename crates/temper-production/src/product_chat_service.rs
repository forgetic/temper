//! Loopback HTTP transport for the product-manager interactive profile.

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use serde_json::json;
use temper_agents::{AuthChoice, ProviderConfig};
use temper_forge::{Forge, Issue, ItemNumber, RepositoryPath};
use temper_forge_forgejo::{ForgejoConfig, ForgejoForge};
use temper_interaction::{
    AcceptedProposalTarget, ConversationEventLog, ConversationEventPayload, ConversationId,
    ConversationProfileId, ConversationReply, ConversationTranscriptRef, ConversationTurn,
    InteractiveResponder, Participant, Proposal, ProposalId,
};
use tokio::sync::Mutex;

use crate::product_chat::{
    build_product_profile_responder, ProductChatError, ProductChatOpenOptions, ProductChatSession,
    PRODUCT_PROFILE_ID,
};
use crate::product_chat_api::{
    parse_json, AcceptProposalResponse, ApiError, ConversationEventsResponse, ConversationResponse,
    ConversationTurnOutcome, CreateConversationRequest, CreateSessionRequest, FileDraftResponse,
    FiledIssueResponse, MessageResponse, ProposalsResponse, SendMessageRequest, SendTurnRequest,
    SessionResponse, TranscriptResponse, TurnResponse,
};
use crate::product_chat_args::{AuthKind, ProductChatServeArgs};
use crate::product_chat_commands::{render_drafts, ProductChatCommand, COMMAND_HELP};
pub(crate) use crate::product_chat_http::{HttpRequest, HttpResponse};

const STREAMING_EVENTS_ENABLED: bool = false;

type DynSession = ProductChatSession<dyn Forge, dyn Forge, dyn InteractiveResponder>;

#[derive(Debug)]
pub enum ProductChatServiceError {
    ProductChat(ProductChatError),
    Io(std::io::Error),
}

impl fmt::Display for ProductChatServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProductChatServiceError::ProductChat(error) => write!(formatter, "{error}"),
            ProductChatServiceError::Io(error) => write!(formatter, "service I/O failed: {error}"),
        }
    }
}

impl std::error::Error for ProductChatServiceError {}

impl From<ProductChatError> for ProductChatServiceError {
    fn from(error: ProductChatError) -> Self {
        Self::ProductChat(error)
    }
}

impl From<std::io::Error> for ProductChatServiceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn run_serve(args: &ProductChatServeArgs) -> Result<(), ProductChatServiceError> {
    validate_serve_safety(args)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ProductChatError::Runtime(error.to_string()))?;
    let responder = build_product_profile_responder(args.process_responder.clone(), || {
        ProviderConfig::from_auth(
            auth_choice(args.auth),
            args.codex_model.clone(),
            args.auth_file.clone(),
        )
    })?;
    let service = ProductChatService::new(
        args.base_url.clone(),
        args.repo.clone(),
        Arc::new(build_forge(&args.base_url, &args.human_token)) as Arc<dyn Forge>,
        Arc::new(build_forge(&args.base_url, &args.product_manager_token)) as Arc<dyn Forge>,
        responder,
    );
    let app = ProductChatHttpApp::new(service, args.service_token.clone());
    run_http(args.bind, app, &runtime)
}

fn validate_serve_safety(args: &ProductChatServeArgs) -> Result<(), ProductChatError> {
    if args.bind.ip().is_loopback() {
        return Ok(());
    }
    if !args.allow_non_loopback {
        return Err(ProductChatError::Runtime(
            "non-loopback bind requires --allow-non-loopback".into(),
        ));
    }
    if args
        .service_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())
        .is_none()
    {
        return Err(ProductChatError::Runtime(
            "non-loopback bind requires TEMPER_PRODUCT_CHAT_SERVICE_TOKEN".into(),
        ));
    }
    Ok(())
}

fn build_forge(base_url: &str, token: &str) -> ForgejoForge {
    ForgejoForge::new(ForgejoConfig::new(base_url.to_string(), token.to_string()))
}

fn auth_choice(auth: AuthKind) -> AuthChoice {
    match auth {
        AuthKind::DeepSeek => AuthChoice::DeepSeek,
        AuthKind::ChatGptOAuth => AuthChoice::ChatGptOAuth,
        AuthKind::AnthropicOAuth => AuthChoice::AnthropicOAuth,
    }
}

fn run_http(
    bind: SocketAddr,
    app: ProductChatHttpApp,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), ProductChatServiceError> {
    let listener = TcpListener::bind(bind)?;
    eprintln!("temper-product-manager-chat: serving on {bind}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &app, runtime) {
                    eprintln!("temper-product-manager-chat: {error}");
                }
            }
            Err(error) => eprintln!("temper-product-manager-chat: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    app: &ProductChatHttpApp,
    runtime: &tokio::runtime::Runtime,
) -> Result<(), ProductChatServiceError> {
    let request = HttpRequest::read_from(&mut stream)?;
    let response = runtime.block_on(app.handle_http_request(request));
    stream.write_all(response.to_http().as_bytes())?;
    Ok(())
}

pub struct ProductChatService {
    base_url: String,
    repo_path: RepositoryPath,
    profile_id: ConversationProfileId,
    human_forge: Arc<dyn Forge>,
    product_forge: Arc<dyn Forge>,
    responder: Arc<dyn InteractiveResponder>,
    sessions: Mutex<HashMap<String, DynSession>>,
    events: ConversationEventLog,
}

impl ProductChatService {
    pub fn new(
        base_url: String,
        repo_path: RepositoryPath,
        human_forge: Arc<dyn Forge>,
        product_forge: Arc<dyn Forge>,
        responder: Arc<dyn InteractiveResponder>,
    ) -> Self {
        Self {
            base_url,
            repo_path,
            profile_id: ConversationProfileId::new(PRODUCT_PROFILE_ID)
                .expect("product profile id is valid"),
            human_forge,
            product_forge,
            responder,
            sessions: Mutex::new(HashMap::new()),
            events: ConversationEventLog::new(),
        }
    }

    async fn create_conversation(
        &self,
        profile_id: Option<ConversationProfileId>,
        transcript_issue: Option<u64>,
    ) -> Result<ConversationResponse, ApiError> {
        self.ensure_profile(profile_id.as_ref())?;
        let session = ProductChatSession::open(
            Arc::clone(&self.human_forge),
            Arc::clone(&self.product_forge),
            Arc::clone(&self.responder),
            ProductChatOpenOptions {
                base_url: self.base_url.clone(),
                repo_path: self.repo_path.clone(),
                transcript_issue: transcript_issue.map(ItemNumber::new),
            },
        )
        .await?;
        let response = conversation_response(&session, &self.profile_id);
        self.events.record(
            session.conversation_id().clone(),
            ConversationEventPayload::ConversationOpened {
                profile_id: self.profile_id.clone(),
                transcript: Some(transcript_ref(&response.transcript)),
            },
        );
        self.sessions
            .lock()
            .await
            .insert(response.id.clone(), session);
        Ok(response)
    }

    async fn get_conversation(&self, id: &str) -> Result<ConversationResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("conversation not found"))?;
        Ok(conversation_response(session, &self.profile_id))
    }

    async fn latest_proposals(&self, id: &str) -> Result<ProposalsResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("conversation not found"))?;
        Ok(ProposalsResponse {
            proposals: session.latest_proposals().to_vec(),
        })
    }

    async fn send_turn(&self, id: &str, body: String) -> Result<ConversationTurnOutcome, ApiError> {
        if body.trim().is_empty() {
            return Err(ApiError::bad_request("body must not be empty"));
        }
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| ApiError::not_found("conversation not found"))?;
        let conversation_id = session.conversation_id().clone();
        let reply = session.send_conversation_turn(&body).await?;
        let response = TurnResponse {
            reply: reply.clone(),
            transcript: transcript_response(session),
            latest_proposals: session.latest_proposals().to_vec(),
        };
        let drafts = session.latest_drafts().to_vec();
        self.record_turn_events(conversation_id, body, &reply, &response.latest_proposals);
        Ok(ConversationTurnOutcome { response, drafts })
    }

    async fn accept_proposal(
        &self,
        id: &str,
        proposal_id: ProposalId,
    ) -> Result<AcceptProposalResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("conversation not found"))?;
        let conversation_id = session.conversation_id().clone();
        let outcome = session.accept_proposal(&proposal_id).await?;
        let response = accept_response(session, proposal_id, outcome.created, &outcome.issue);
        self.record_accept_event(conversation_id, &response);
        Ok(response)
    }

    async fn conversation_events(&self, id: &str) -> Result<ConversationEventsResponse, ApiError> {
        let conversation_id = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(id)
                .ok_or_else(|| ApiError::not_found("conversation not found"))?
                .conversation_id()
                .clone()
        };
        Ok(ConversationEventsResponse {
            streaming: STREAMING_EVENTS_ENABLED,
            events: self.events.list(&conversation_id),
        })
    }

    async fn create_session(
        &self,
        transcript_issue: Option<u64>,
    ) -> Result<SessionResponse, ApiError> {
        self.create_conversation(None, transcript_issue)
            .await
            .map(SessionResponse::from)
    }

    async fn get_session(&self, id: &str) -> Result<SessionResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("session not found"))?;
        Ok(session_response(session))
    }

    async fn send_message(&self, id: &str, message: String) -> Result<MessageResponse, ApiError> {
        if message.trim().is_empty() {
            return Err(ApiError::bad_request("message must not be empty"));
        }
        {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(id)
                .ok_or_else(|| ApiError::not_found("session not found"))?;
            if let Some(command) = ProductChatCommand::parse(&message) {
                return handle_local_message_command(session, command);
            }
        }
        let outcome = self.send_turn(id, message).await?;
        Ok(MessageResponse {
            reply: outcome.response.reply.message,
            drafts: outcome.drafts,
            transcript_url: outcome.response.transcript.url,
        })
    }

    async fn file_draft(&self, id: &str, slug: &str) -> Result<FileDraftResponse, ApiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError::not_found("session not found"))?;
        let conversation_id = session.conversation_id().clone();
        let outcome = session.file_draft_slug(slug).await?;
        let proposal_id = ProposalId::new(slug.to_string())
            .map_err(|error| ApiError::bad_request(error.to_string()))?;
        let response = accept_response(session, proposal_id, outcome.created, &outcome.issue);
        self.record_accept_event(conversation_id, &response);
        Ok(FileDraftResponse::from(response))
    }

    fn ensure_profile(&self, requested: Option<&ConversationProfileId>) -> Result<(), ApiError> {
        if let Some(requested) = requested.filter(|requested| *requested != &self.profile_id) {
            return Err(ApiError::bad_request(format!(
                "profile `{requested}` is not configured for this service"
            )));
        }
        Ok(())
    }

    fn record_turn_events(
        &self,
        conversation_id: ConversationId,
        body: String,
        reply: &ConversationReply,
        proposals: &[Proposal],
    ) {
        self.events.record(
            conversation_id.clone(),
            ConversationEventPayload::HumanTurnAppended {
                turn: ConversationTurn::new(Participant::human("human"), body),
            },
        );
        self.events.record(
            conversation_id.clone(),
            ConversationEventPayload::AgentReplyAppended {
                reply: reply.clone(),
            },
        );
        self.events.record(
            conversation_id,
            ConversationEventPayload::ProposalsUpdated {
                proposals: proposals.to_vec(),
            },
        );
    }

    fn record_accept_event(
        &self,
        conversation_id: ConversationId,
        response: &AcceptProposalResponse,
    ) {
        self.events.record(
            conversation_id,
            ConversationEventPayload::ProposalAccepted {
                proposal_id: response.proposal_id.clone(),
                created: response.created,
                target: Some(AcceptedProposalTarget::issue(
                    response.target.number,
                    response.target.url.clone(),
                    response.target.title.clone(),
                )),
            },
        );
    }
}

fn handle_local_message_command(
    session: &mut DynSession,
    command: ProductChatCommand<'_>,
) -> Result<MessageResponse, ApiError> {
    let reply = match command {
        ProductChatCommand::Help => COMMAND_HELP.to_string(),
        ProductChatCommand::Drafts => render_drafts(session.latest_drafts()),
        ProductChatCommand::Issue => session.transcript_url(),
        ProductChatCommand::Quit => "Close the client to end this product conversation.".into(),
        ProductChatCommand::File(_) => {
            return Err(ApiError::bad_request(
                "file drafts through POST /sessions/{id}/drafts/{slug}/file",
            ))
        }
        ProductChatCommand::Unknown(command) => {
            return Err(ApiError::bad_request(format!(
                "unknown command '{command}'; try /help"
            )))
        }
    };
    Ok(MessageResponse {
        reply,
        drafts: session.latest_drafts().to_vec(),
        transcript_url: session.transcript_url(),
    })
}

pub struct ProductChatHttpApp {
    service: ProductChatService,
    service_token: Option<String>,
}

impl ProductChatHttpApp {
    pub fn new(service: ProductChatService, service_token: Option<String>) -> Self {
        Self {
            service,
            service_token,
        }
    }

    pub(crate) async fn handle_http_request(&self, request: HttpRequest) -> HttpResponse {
        if let Err(error) = self.authorize(&request) {
            return error.into_response();
        }
        match self.route(request).await {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }

    async fn route(&self, request: HttpRequest) -> Result<HttpResponse, ApiError> {
        let path = request.path.split('?').next().unwrap_or(&request.path);
        if request.method == "GET" && path == "/health" {
            return Ok(HttpResponse::json(200, &json!({ "ok": true })));
        }
        if request.method == "POST" && path == "/conversations" {
            let body = parse_json::<CreateConversationRequest>(&request)?;
            let response = self
                .service
                .create_conversation(body.profile_id, body.transcript_issue)
                .await?;
            return Ok(HttpResponse::json(201, &response));
        }
        if request.method == "POST" && path == "/sessions" {
            let body = parse_json::<CreateSessionRequest>(&request)?;
            let response = self.service.create_session(body.transcript_issue).await?;
            return Ok(HttpResponse::json(201, &response));
        }
        let segments: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        match (request.method.as_str(), segments.as_slice()) {
            ("GET", ["conversations", id]) => Ok(HttpResponse::json(
                200,
                &self.service.get_conversation(id).await?,
            )),
            ("POST", ["conversations", id, "turns"]) => {
                let body = parse_json::<SendTurnRequest>(&request)?.into_body()?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.send_turn(id, body).await?.response,
                ))
            }
            ("GET", ["conversations", id, "proposals"]) => Ok(HttpResponse::json(
                200,
                &self.service.latest_proposals(id).await?,
            )),
            ("GET", ["conversations", id, "events"]) => Ok(HttpResponse::json(
                200,
                &self.service.conversation_events(id).await?,
            )),
            ("POST", ["conversations", id, "proposals", proposal_id, "accept"]) => {
                let proposal_id = ProposalId::new((*proposal_id).to_string())
                    .map_err(|error| ApiError::bad_request(error.to_string()))?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.accept_proposal(id, proposal_id).await?,
                ))
            }
            ("GET", ["sessions", id]) => Ok(HttpResponse::json(
                200,
                &self.service.get_session(id).await?,
            )),
            ("POST", ["sessions", id, "messages"]) => {
                let body = parse_json::<SendMessageRequest>(&request)?;
                Ok(HttpResponse::json(
                    200,
                    &self.service.send_message(id, body.message).await?,
                ))
            }
            ("POST", ["sessions", id, "drafts", slug, "file"]) => Ok(HttpResponse::json(
                200,
                &self.service.file_draft(id, slug).await?,
            )),
            _ => Err(ApiError::not_found("endpoint not found")),
        }
    }

    fn authorize(&self, request: &HttpRequest) -> Result<(), ApiError> {
        let Some(token) = self.service_token.as_deref() else {
            return Ok(());
        };
        let supplied = request
            .headers
            .get("authorization")
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied == Some(token) {
            Ok(())
        } else {
            Err(ApiError::unauthorized("missing or invalid bearer token"))
        }
    }
}

fn conversation_response(
    session: &DynSession,
    profile_id: &ConversationProfileId,
) -> ConversationResponse {
    ConversationResponse {
        id: session.session_key().to_string(),
        profile_id: profile_id.to_string(),
        transcript: transcript_response(session),
        latest_proposals: session.latest_proposals().to_vec(),
    }
}

fn session_response(session: &DynSession) -> SessionResponse {
    SessionResponse {
        id: session.session_key().to_string(),
        transcript_issue: session.transcript_issue().number.get(),
        transcript_url: session.transcript_url(),
        drafts: session.latest_drafts().to_vec(),
    }
}

fn transcript_response(session: &DynSession) -> TranscriptResponse {
    TranscriptResponse {
        issue_number: session.transcript_issue().number.get(),
        url: session.transcript_url(),
    }
}

fn transcript_ref(response: &TranscriptResponse) -> ConversationTranscriptRef {
    ConversationTranscriptRef::forge_issue(response.issue_number, response.url.clone())
}

fn accept_response(
    session: &DynSession,
    proposal_id: ProposalId,
    created: bool,
    issue: &Issue,
) -> AcceptProposalResponse {
    AcceptProposalResponse {
        proposal_id,
        created,
        target: filed_issue_response(issue, session.issue_url_for(issue.number)),
        transcript: transcript_response(session),
    }
}

fn filed_issue_response(issue: &Issue, url: String) -> FiledIssueResponse {
    FiledIssueResponse {
        number: issue.number.get(),
        url,
        title: issue.title.clone(),
    }
}
