//! Temper-specific provider/auth/decision core for anvil.
//!
//! This crate is anvil's initial home for the concrete `pi_agent_rust` wiring
//! that Temper is splitting out: provider selection, OAuth auth-file handling,
//! per-provider request knobs, coding-agent execution, product-manager replies,
//! and one-turn structured decision parsing. It does not mutate Forge state.

#![allow(clippy::result_large_err)]

pub mod coding_agent;
pub mod decision;
pub mod interaction_profile;
mod interaction_profile_config;
mod interaction_profile_config_raw;
mod observability;
pub mod product_manager;
pub mod prompt_overlays;
pub mod provider;
mod tool_preview;
pub mod usage;

pub use coding_agent::{
    Capability, CheckpointHook, CodingAgentError, DEFAULT_MAX_ITERATIONS, WorkspaceContext,
    WorkspaceGuidance, WorkspaceRepository, WorkspaceResult, WorkspaceResultChild,
    WorkspaceWorkItem, run_coding_agent_native, run_coding_agent_native_with_hooks,
    run_coding_agent_native_with_options, system_prompt as coding_agent_system_prompt,
    user_context as coding_agent_user_context,
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
    CONFIG_DIR_ENV, ComposedTurns, PromptOverlays, RenderedOverlays, XDG_CONFIG_HOME_ENV,
    resolve_config_dir_from,
};
pub use provider::{
    ANTHROPIC_MODEL_ENV, ANTHROPIC_SUBAGENT_MODEL_ENV, ANTHROPIC_TOKEN_URL_ENV, API_KEY_ENV,
    API_KEY_PATH_ENV, AUTH_FILE_ENV, AuthChoice, CODEX_MODEL_ENV, CODEX_TOKEN_URL_ENV,
    DEFAULT_ANTHROPIC_MODEL, DEFAULT_CODEX_MODEL, PROVIDER_BASE_URL_ENV, ProviderConfig,
    ProviderEnv, ProviderError, default_auth_path,
};
pub use temper_protocol_interaction::{ConversationReply, ConversationRequest};
pub use usage::RunTotals;
