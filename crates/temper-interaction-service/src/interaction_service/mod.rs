//! Generic profile-neutral interaction service and HTTP adapter.

mod http;
mod responses;
mod service;

pub use crate::interaction_api::{
    AcceptProposalResponse, ConversationEventsResponse, ConversationResponse, ProposalsResponse,
    TranscriptResponse, TurnResponse,
};
pub(crate) use crate::interaction_http::{HttpRequest, HttpResponse};

pub use http::{InteractionHttpApp, run_http};
pub use service::{InteractionProfileRuntime, InteractionService, InteractionServiceError};
