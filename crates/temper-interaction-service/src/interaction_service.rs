//! Generic profile-neutral interaction service and HTTP adapter.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use temper_forge::{Forge, Issue, ItemNumber, RepositoryPath};
use temper_interaction::{
    AcceptanceActionId, AcceptedProposalTarget, CompiledProfileManifest, ConversationEventLog,
    ConversationEventPayload, ConversationId, ConversationProfileId, ConversationReply,
    ConversationTranscriptRef, ConversationTurn, ForgeInteractionSession, ForgeSessionConfig,
    ForgeSessionOpenOptions, InteractionError, InteractiveResponder, ProposalId,
};

pub use crate::interaction_api::{
    AcceptProposalResponse, ConversationEventsResponse, ConversationResponse, ProposalsResponse,
    TranscriptResponse, TurnResponse,
};
use crate::interaction_api::{ApiError, CreateConversationRequest, SendTurnRequest, parse_json};
pub(crate) use crate::interaction_http::{HttpRequest, HttpResponse};

const STREAMING_EVENTS_ENABLED: bool = false;

type DynSession = ForgeInteractionSession<dyn Forge, dyn Forge, dyn InteractiveResponder>;

/// One compiled profile plus its deployment-time authority bindings.
#[derive(Clone)]
pub struct InteractionProfileRuntime {
    pub manifest: CompiledProfileManifest,
    pub human_forge: Arc<dyn Forge>,
    pub agent_forge: Arc<dyn Forge>,
    pub responder: Arc<dyn InteractiveResponder>,
}

/// Generic interaction-service failures. API adapters sanitize these further.
#[derive(Debug)]
pub enum InteractionServiceError {
    Interaction(InteractionError),
    Runtime(String),
    BadRequest(String),
    NotFound(String),
    Io(std::io::Error),
}

impl InteractionServiceError {
    pub fn runtime(message: impl Into<String>) -> Self {
        Self::Runtime(message.into())
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }
}

impl fmt::Display for InteractionServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InteractionServiceError::Interaction(error) => write!(formatter, "{error}"),
            InteractionServiceError::Runtime(message)
            | InteractionServiceError::BadRequest(message)
            | InteractionServiceError::NotFound(message) => formatter.write_str(message),
            InteractionServiceError::Io(error) => write!(formatter, "service I/O failed: {error}"),
        }
    }
}

impl std::error::Error for InteractionServiceError {}

impl From<InteractionError> for InteractionServiceError {
    fn from(error: InteractionError) -> Self {
        Self::Interaction(error)
    }
}

impl From<std::io::Error> for InteractionServiceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Profile-neutral runtime used by REPL, HTTP, and future transport adapters.
pub struct InteractionService {
    base_url: String,
    repo_path: RepositoryPath,
    default_profile: Option<ConversationProfileId>,
    profiles: BTreeMap<ConversationProfileId, InteractionProfileRuntime>,
    sessions: std::sync::Mutex<BTreeMap<String, ActiveSession>>,
    events: ConversationEventLog,
}

struct ActiveSession {
    profile: CompiledProfileManifest,
    session: DynSession,
}

impl InteractionService {
    pub fn new(
        base_url: String,
        repo_path: RepositoryPath,
        profiles: Vec<InteractionProfileRuntime>,
        default_profile: Option<ConversationProfileId>,
    ) -> Result<Self, InteractionServiceError> {
        if profiles.is_empty() {
            return Err(InteractionServiceError::runtime(
                "at least one interaction profile must be bound",
            ));
        }
        let profiles = profiles
            .into_iter()
            .map(|runtime| (runtime.manifest.profile.id.clone(), runtime))
            .collect::<BTreeMap<_, _>>();
        if let Some(default) = &default_profile
            && !profiles.contains_key(default)
        {
            return Err(InteractionServiceError::runtime(format!(
                "default profile `{default}` is not bound"
            )));
        }
        Ok(Self {
            base_url,
            repo_path,
            default_profile,
            profiles,
            sessions: std::sync::Mutex::new(BTreeMap::new()),
            events: ConversationEventLog::new(),
        })
    }

    pub fn profile_manifest(&self, id: &ConversationProfileId) -> Option<&CompiledProfileManifest> {
        self.profiles.get(id).map(|runtime| &runtime.manifest)
    }

    pub fn default_profile_id(&self) -> Result<ConversationProfileId, InteractionServiceError> {
        self.resolve_profile_id(None)
    }

    pub async fn create_conversation(
        &self,
        profile_id: Option<ConversationProfileId>,
        transcript_issue: Option<u64>,
        context: Value,
    ) -> Result<ConversationResponse, InteractionServiceError> {
        let resolved = self.resolve_profile_id(profile_id.as_ref())?;
        let runtime = self
            .profiles
            .get(&resolved)
            .ok_or_else(|| InteractionServiceError::bad_request("profile is not bound"))?;
        let config = ForgeSessionConfig::from_profile_manifest(&runtime.manifest)?;
        let session = ForgeInteractionSession::open(
            Arc::clone(&runtime.human_forge),
            Arc::clone(&runtime.agent_forge),
            Arc::clone(&runtime.responder),
            config,
            ForgeSessionOpenOptions {
                base_url: self.base_url.clone(),
                repo_path: self.repo_path.clone(),
                transcript_issue: transcript_issue.map(ItemNumber::new),
                context: normalize_context(context),
            },
        )
        .await?;
        let response = conversation_response(&session, &runtime.manifest);
        self.events.record(
            session.conversation_id().clone(),
            ConversationEventPayload::ConversationOpened {
                profile_id: runtime.manifest.profile.id.clone(),
                transcript: Some(transcript_ref(&response.transcript)),
            },
        );
        self.sessions.lock().expect("sessions lock").insert(
            response.id.clone(),
            ActiveSession {
                profile: runtime.manifest.clone(),
                session,
            },
        );
        Ok(response)
    }

    pub async fn get_conversation(
        &self,
        id: &str,
    ) -> Result<ConversationResponse, InteractionServiceError> {
        let sessions = self.sessions.lock().expect("sessions lock");
        let active = sessions
            .get(id)
            .ok_or_else(|| InteractionServiceError::not_found("conversation not found"))?;
        Ok(conversation_response(&active.session, &active.profile))
    }

    pub async fn latest_proposals(
        &self,
        id: &str,
    ) -> Result<ProposalsResponse, InteractionServiceError> {
        let sessions = self.sessions.lock().expect("sessions lock");
        let active = sessions
            .get(id)
            .ok_or_else(|| InteractionServiceError::not_found("conversation not found"))?;
        Ok(ProposalsResponse {
            proposals: active.session.latest_proposals().to_vec(),
        })
    }

    pub async fn send_turn(
        &self,
        id: &str,
        body: String,
    ) -> Result<TurnResponse, InteractionServiceError> {
        if body.trim().is_empty() {
            return Err(InteractionServiceError::bad_request(
                "body must not be empty",
            ));
        }
        // Take the session out of the map for the awaited turn so no lock is
        // held across the responder I/O, then put it back regardless of the
        // outcome. Requests for one conversation are serialized by taking
        // ownership here.
        let mut active = self
            .sessions
            .lock()
            .expect("sessions lock")
            .remove(id)
            .ok_or_else(|| InteractionServiceError::not_found("conversation not found"))?;
        let conversation_id = active.session.conversation_id().clone();
        let reply = active.session.send_human_turn(&body).await;
        let outcome = reply.map(|reply| {
            let response = TurnResponse {
                reply: reply.clone(),
                transcript: transcript_response(&active.session),
                latest_proposals: active.session.latest_proposals().to_vec(),
            };
            self.record_turn_events(conversation_id, &active.profile, body, &reply, &response);
            response
        });
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(id.to_string(), active);
        Ok(outcome?)
    }

    pub async fn accept_proposal(
        &self,
        id: &str,
        proposal_id: ProposalId,
    ) -> Result<AcceptProposalResponse, InteractionServiceError> {
        self.accept_proposal_with_action(id, proposal_id, None)
            .await
    }

    pub async fn accept_proposal_with_action(
        &self,
        id: &str,
        proposal_id: ProposalId,
        acceptance_action: Option<&AcceptanceActionId>,
    ) -> Result<AcceptProposalResponse, InteractionServiceError> {
        // Same take/reinsert discipline as `send_turn`: never hold the
        // sessions lock across responder I/O.
        let active = self
            .sessions
            .lock()
            .expect("sessions lock")
            .remove(id)
            .ok_or_else(|| InteractionServiceError::not_found("conversation not found"))?;
        let conversation_id = active.session.conversation_id().clone();
        let outcome = active
            .session
            .accept_issue_proposal_with_action(&proposal_id, acceptance_action)
            .await;
        let response = outcome.map(|outcome| {
            let response = accept_response(
                &active.session,
                proposal_id,
                outcome.created,
                &outcome.issue,
            );
            self.record_accept_event(conversation_id, &response);
            response
        });
        self.sessions
            .lock()
            .expect("sessions lock")
            .insert(id.to_string(), active);
        Ok(response?)
    }

    pub async fn conversation_events(
        &self,
        id: &str,
    ) -> Result<ConversationEventsResponse, InteractionServiceError> {
        let conversation_id = {
            let sessions = self.sessions.lock().expect("sessions lock");
            sessions
                .get(id)
                .ok_or_else(|| InteractionServiceError::not_found("conversation not found"))?
                .session
                .conversation_id()
                .clone()
        };
        Ok(ConversationEventsResponse {
            streaming: STREAMING_EVENTS_ENABLED,
            events: self.events.list(&conversation_id),
        })
    }

    fn resolve_profile_id(
        &self,
        requested: Option<&ConversationProfileId>,
    ) -> Result<ConversationProfileId, InteractionServiceError> {
        if let Some(requested) = requested {
            if self.profiles.contains_key(requested) {
                return Ok(requested.clone());
            }
            return Err(InteractionServiceError::bad_request(format!(
                "profile `{requested}` is not configured for this service"
            )));
        }
        if let Some(default) = &self.default_profile {
            return Ok(default.clone());
        }
        let mut ids = self.profiles.keys();
        let first = ids.next().expect("profiles is non-empty");
        if ids.next().is_none() {
            return Ok(first.clone());
        }
        Err(InteractionServiceError::bad_request(
            "profile_id is required when multiple profiles are configured",
        ))
    }

    fn record_turn_events(
        &self,
        conversation_id: ConversationId,
        profile: &CompiledProfileManifest,
        body: String,
        reply: &ConversationReply,
        response: &TurnResponse,
    ) {
        self.events.record(
            conversation_id.clone(),
            ConversationEventPayload::HumanTurnAppended {
                turn: ConversationTurn::new(profile.profile.human_participant.clone(), body),
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
                proposals: response.latest_proposals.clone(),
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
                target: response.target.clone(),
            },
        );
    }
}

/// Profile-neutral HTTP app over [`InteractionService`].
pub struct InteractionHttpApp {
    service: InteractionService,
    service_token: Option<String>,
}

impl InteractionHttpApp {
    pub fn new(service: InteractionService, service_token: Option<String>) -> Self {
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
                .create_conversation(body.profile_id, body.transcript_issue, json!({}))
                .await?;
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
                    &self.service.send_turn(id, body).await?,
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

pub fn run_http(
    bind: SocketAddr,
    app: InteractionHttpApp,
    runtime: &temper_engine_io::EngineRuntime,
) -> Result<(), InteractionServiceError> {
    let listener = TcpListener::bind(bind)?;
    eprintln!("temper-interaction: serving on {bind}");
    let app = std::sync::Arc::new(app);
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_connection(stream, &app, runtime) {
                    eprintln!("temper-interaction: {error}");
                }
            }
            Err(error) => eprintln!("temper-interaction: accept failed: {error}"),
        }
    }
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    app: &std::sync::Arc<InteractionHttpApp>,
    runtime: &temper_engine_io::EngineRuntime,
) -> Result<(), InteractionServiceError> {
    let request = HttpRequest::read_from(&mut stream)?;
    // Each request runs as one engine task: responder subprocess calls and
    // their deadlines are engine I/O requests needing a task context.
    let app = std::sync::Arc::clone(app);
    let response = temper_engine_io::runtime::block_on_runtime(runtime, async move {
        app.handle_http_request(request).await
    });
    stream.write_all(response.to_http().as_bytes())?;
    Ok(())
}

fn conversation_response(
    session: &DynSession,
    profile: &CompiledProfileManifest,
) -> ConversationResponse {
    ConversationResponse {
        id: session.conversation_id().to_string(),
        profile_id: profile.profile.id.to_string(),
        transcript: transcript_response(session),
        turns: session.turns().to_vec(),
        latest_proposals: session.latest_proposals().to_vec(),
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
        target: Some(AcceptedProposalTarget::issue(
            issue.number.get(),
            session.issue_url_for(issue.number),
            issue.title.clone(),
        )),
        transcript: transcript_response(session),
    }
}

fn normalize_context(context: Value) -> Value {
    if context.is_null() {
        Value::Object(Map::new())
    } else {
        context
    }
}
