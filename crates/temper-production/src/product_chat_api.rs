//! JSON DTOs and sanitized API errors for product-chat HTTP transports.

use serde::{Deserialize, Serialize};
use serde_json::json;
use temper_interaction::{
    ConversationEvent, ConversationProfileId, ConversationReply, InteractionError, Proposal,
    ProposalId,
};

use crate::product_chat::{ProductChatError, ProductManagerDraftIssue};
use crate::product_chat_http::{HttpRequest, HttpResponse};

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

impl From<ProductChatError> for ApiError {
    fn from(error: ProductChatError) -> Self {
        match error {
            ProductChatError::TranscriptNotFound { .. }
            | ProductChatError::DraftNotFound { .. } => ApiError::not_found(error.to_string()),
            ProductChatError::InvalidDraftNumber { .. }
            | ProductChatError::TranscriptNotProduct { .. }
            | ProductChatError::RepositoryNotFound { .. } => {
                ApiError::bad_request(error.to_string())
            }
            ProductChatError::Interaction(error) => interaction_error_response(error),
            ProductChatError::Forge(_) => ApiError::internal("forge operation failed"),
            ProductChatError::ProductManager(_) => {
                ApiError::internal("interactive responder failed")
            }
            ProductChatError::Runtime(_) | ProductChatError::Io(_) => {
                ApiError::internal("interaction service failed")
            }
        }
    }
}

fn interaction_error_response(error: InteractionError) -> ApiError {
    match error {
        InteractionError::ProposalNotFound { .. } => ApiError::not_found(error.to_string()),
        InteractionError::UnsupportedProposalKind { .. } => {
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
        InteractionError::InvalidSlug { .. } => ApiError::bad_request(error.to_string()),
    }
}

#[derive(Deserialize)]
pub(crate) struct CreateConversationRequest {
    #[serde(default)]
    pub(crate) profile_id: Option<ConversationProfileId>,
    #[serde(default)]
    pub(crate) transcript_issue: Option<u64>,
}

#[derive(Deserialize)]
pub(crate) struct CreateSessionRequest {
    #[serde(default)]
    pub(crate) transcript_issue: Option<u64>,
}

#[derive(Deserialize)]
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

#[derive(Deserialize)]
pub(crate) struct SendMessageRequest {
    pub(crate) message: String,
}

#[derive(Serialize)]
pub(crate) struct ConversationResponse {
    pub(crate) id: String,
    pub(crate) profile_id: String,
    pub(crate) transcript: TranscriptResponse,
    pub(crate) latest_proposals: Vec<Proposal>,
}

#[derive(Serialize)]
pub(crate) struct TurnResponse {
    pub(crate) reply: ConversationReply,
    pub(crate) transcript: TranscriptResponse,
    pub(crate) latest_proposals: Vec<Proposal>,
}

#[derive(Serialize)]
pub(crate) struct ProposalsResponse {
    pub(crate) proposals: Vec<Proposal>,
}

#[derive(Serialize)]
pub(crate) struct AcceptProposalResponse {
    pub(crate) proposal_id: ProposalId,
    pub(crate) created: bool,
    pub(crate) target: FiledIssueResponse,
    pub(crate) transcript: TranscriptResponse,
}

#[derive(Serialize)]
pub(crate) struct ConversationEventsResponse {
    pub(crate) streaming: bool,
    pub(crate) events: Vec<ConversationEvent>,
}

pub(crate) struct ConversationTurnOutcome {
    pub(crate) response: TurnResponse,
    pub(crate) drafts: Vec<ProductManagerDraftIssue>,
}

#[derive(Serialize)]
pub(crate) struct TranscriptResponse {
    pub(crate) issue_number: u64,
    pub(crate) url: String,
}

#[derive(Serialize)]
pub(crate) struct SessionResponse {
    pub(crate) id: String,
    pub(crate) transcript_issue: u64,
    pub(crate) transcript_url: String,
    pub(crate) drafts: Vec<ProductManagerDraftIssue>,
}

impl From<ConversationResponse> for SessionResponse {
    fn from(response: ConversationResponse) -> Self {
        Self {
            id: response.id,
            transcript_issue: response.transcript.issue_number,
            transcript_url: response.transcript.url,
            drafts: Vec::new(),
        }
    }
}

#[derive(Serialize)]
pub(crate) struct MessageResponse {
    pub(crate) reply: String,
    pub(crate) drafts: Vec<ProductManagerDraftIssue>,
    pub(crate) transcript_url: String,
}

#[derive(Serialize)]
pub(crate) struct FileDraftResponse {
    created: bool,
    issue: FiledIssueResponse,
    transcript_url: String,
}

impl From<AcceptProposalResponse> for FileDraftResponse {
    fn from(response: AcceptProposalResponse) -> Self {
        Self {
            created: response.created,
            issue: response.target,
            transcript_url: response.transcript.url,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct FiledIssueResponse {
    pub(crate) number: u64,
    pub(crate) url: String,
    pub(crate) title: String,
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
