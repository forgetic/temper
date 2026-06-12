//! Temper-specific provider/auth/decision core for Smith.
//!
//! This crate is Smith's initial home for the concrete `pi_agent_rust` wiring
//! that Temper is splitting out: provider selection, OAuth auth-file handling,
//! per-provider request knobs, and one-turn structured decision parsing. It does
//! not mutate Forge state and depends on Temper's serialization-only process
//! protocol crate for responder wire DTOs.

#![allow(clippy::result_large_err)]

pub mod coding_agent;
pub mod decision;
pub mod interaction_profile;
mod interaction_profile_config;
mod observability;
pub mod product_manager;
pub mod prompt_overlays;
pub mod provider;
pub mod workflow_role_decision;
mod workflow_role_decision_capture;
mod workflow_role_decision_observability;

pub use coding_agent::{
    Capability, CodingAgentError, DEFAULT_MAX_ITERATIONS, WorkspaceContext, WorkspaceGuidance,
    WorkspaceRepository, WorkspaceResult, WorkspaceResultChild, WorkspaceWorkItem,
    run_coding_agent, run_coding_agent_native, run_coding_agent_native_with_options,
    system_prompt as coding_agent_system_prompt, user_context as coding_agent_user_context,
};
pub use decision::{DecisionError, run_decision};
pub use interaction_profile::{
    CONVERSATION_REPLY_V1_PROTOCOL_INSTRUCTION, GenericInteractionResponder,
    InteractionAllowedProposalKind, InteractionProfileConfig, InteractionProfileError,
    InteractionProposalPayloadContract, InteractionResponseFormat,
};
pub use product_manager::{
    PRODUCT_MANAGER_PROFILE_ID, PRODUCT_MANAGER_SYSTEM_PROMPT, ProductManagerAuthor,
    ProductManagerConversationTurn, ProductManagerDraftIssue, ProductManagerError,
    ProductManagerRequest, ProductManagerResponder, ProductManagerResponse,
};
pub use prompt_overlays::{
    CONFIG_DIR_ENV, ComposedTurns, PromptOverlays, RenderedOverlays, resolve_config_dir,
};
pub use provider::{
    ANTHROPIC_MODEL_ENV, AUTH_FILE_ENV, AuthChoice, CODEX_MODEL_ENV, DEFAULT_ANTHROPIC_MODEL,
    DEFAULT_CODEX_MODEL, ProviderConfig, ProviderError, default_auth_path,
};
pub use temper_process_protocol::{
    ConversationReply, ConversationRequest, WorkflowRoleDecisionReply, WorkflowRoleDecisionRequest,
};
pub use workflow_role_decision::{
    WorkflowRoleDecisionError, WorkflowRoleDecisionResponder, WorkflowRoleModelDecision,
    reply_for_model_decision, workflow_role_system_prompt, workflow_role_user_context,
};
pub use workflow_role_decision_capture::WORKFLOW_ROLE_DECISION_CAPTURE_DIR_ENV;
