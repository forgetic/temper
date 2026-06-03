//! JSON DTOs and sanitized API errors for generic interaction HTTP transports.

use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_interaction::{
    AcceptedProposalTarget, ConversationEvent, ConversationProfileId, ConversationReply,
    ConversationTurn, InteractionError, Proposal, ProposalId,
};

use crate::interaction_http::{HttpRequest, HttpResponse};
use crate::interaction_service::InteractionServiceError;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: u16,
    message: String,
}

impl ApiError {
    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: 401,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: 404,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }

    pub(crate) fn into_response(self) -> HttpResponse {
        HttpResponse::json(self.status, &json!({ "error": self.message }))
    }
}

impl From<InteractionServiceError> for ApiError {
    fn from(error: InteractionServiceError) -> Self {
        match error {
            InteractionServiceError::BadRequest(message) => ApiError::bad_request(message),
            InteractionServiceError::NotFound(message) => ApiError::not_found(message),
            InteractionServiceError::Interaction(error) => interaction_error_response(error),
            InteractionServiceError::Runtime(_) | InteractionServiceError::Io(_) => {
                ApiError::internal("interaction service failed")
            }
        }
    }
}

fn interaction_error_response(error: InteractionError) -> ApiError {
    match error {
        InteractionError::ProposalNotFound { .. } => ApiError::not_found(error.to_string()),
        InteractionError::UnsupportedProposalKind { .. } | InteractionError::InvalidSlug { .. } => {
            ApiError::bad_request(error.to_string())
        }
        InteractionError::Forge(_) => ApiError::internal("forge operation failed"),
        InteractionError::ProcessResponderIo { .. }
        | InteractionError::ProcessResponderTimeout { .. }
        | InteractionError::ProcessResponderExit { .. }
        | InteractionError::ProcessResponderMalformedJson { .. }
        | InteractionError::Responder { .. }
        | InteractionError::Profile { .. }
        | InteractionError::Provider { .. }
        | InteractionError::DuplicateProposalId { .. }
        | InteractionError::Json(_) => ApiError::internal("interactive responder failed"),
        InteractionError::InvalidConfig { .. }
        | InteractionError::InvalidMarkerNamespace { .. }
        | InteractionError::RepositoryNotFound { .. }
        | InteractionError::TranscriptNotFound { .. }
        | InteractionError::TranscriptLabelMismatch { .. } => {
            ApiError::internal("interaction service configuration failed")
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateConversationRequest {
    #[serde(default)]
    pub(crate) profile_id: Option<ConversationProfileId>,
    #[serde(default)]
    pub(crate) transcript_issue: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SendTurnRequest {
    #[serde(default)]
    body: Option<String>,
}

impl SendTurnRequest {
    pub(crate) fn into_body(self) -> Result<String, ApiError> {
        self.body
            .ok_or_else(|| ApiError::bad_request("body is required"))
    }
}

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub id: String,
    pub profile_id: String,
    pub transcript: TranscriptResponse,
    pub turns: Vec<ConversationTurn>,
    pub latest_proposals: Vec<Proposal>,
}

#[derive(Debug, Serialize)]
pub struct TurnResponse {
    pub reply: ConversationReply,
    pub transcript: TranscriptResponse,
    pub latest_proposals: Vec<Proposal>,
}

#[derive(Debug, Serialize)]
pub struct ProposalsResponse {
    pub proposals: Vec<Proposal>,
}

#[derive(Debug, Serialize)]
pub struct AcceptProposalResponse {
    pub proposal_id: ProposalId,
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<AcceptedProposalTarget>,
    pub transcript: TranscriptResponse,
}

#[derive(Debug, Serialize)]
pub struct ConversationEventsResponse {
    pub streaming: bool,
    pub events: Vec<ConversationEvent>,
}

#[derive(Debug, Serialize)]
pub struct TranscriptResponse {
    pub issue_number: u64,
    pub url: String,
}

pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(
    request: &HttpRequest,
) -> Result<T, ApiError> {
    if request.body.is_empty() {
        serde_json::from_str("{}").map_err(|error| ApiError::bad_request(error.to_string()))
    } else {
        serde_json::from_slice(&request.body)
            .map_err(|error| ApiError::bad_request(format!("invalid JSON body: {error}")))
    }
}
