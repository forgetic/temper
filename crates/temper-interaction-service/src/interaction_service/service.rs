//! Profile-neutral interaction runtime shared by all transport adapters.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde_json::Value;
use temper_forge::{Forge, ItemNumber, RepositoryPath};
use temper_interaction::{
    AcceptanceActionId, CompiledProfileManifest, ConversationEventLog, ConversationEventPayload,
    ConversationId, ConversationProfileId, ConversationReply, ConversationTurn,
    ForgeInteractionSession, ForgeSessionConfig, ForgeSessionOpenOptions, InteractionError,
    InteractiveResponder, ProposalId,
};

use crate::interaction_service::responses::{
    DynSession, accept_response, conversation_response, normalize_context, transcript_ref,
    transcript_response,
};
use crate::interaction_service::{
    AcceptProposalResponse, ConversationEventsResponse, ConversationResponse, ProposalsResponse,
    TurnResponse,
};

pub(super) const STREAMING_EVENTS_ENABLED: bool = false;

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
        if let Some(default) = &default_profile {
            if !profiles.contains_key(default) {
                return Err(InteractionServiceError::runtime(format!(
                    "default profile `{default}` is not bound"
                )));
            }
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
