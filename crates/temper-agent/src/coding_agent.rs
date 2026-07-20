//! The coding workspace agent, on anvil's native loop + tongs tools.
//!
//! This module implements temper's external coding-workspace command. Where
//! [`crate::decision`] runs a single tool-less turn that only *decides*, this
//! module runs a tool-using agent that *acts*: it reads the work-item context
//! temper prepared, runs a real LLM agent loop with a
//! [`tongs::tools::ToolRegistry`] scoped to the checkout, and produces the ADR
//! 0022 work product (a working-tree diff and/or a verdict) for the role.
//!
//! # Protocol
//!
//! temper writes a context JSON file and passes its path as the agent's
//! `--context` flag, runs the agent in the prepared checkout (`--workspace`,
//! also cwd), and reads a result JSON file back from the `--result` path. The
//! result shape is temper's `WorkspaceResult` (`{ verdict?, title?, summary?, body?,
//! review_body?, labels?, children? }`); see [`WorkspaceResult`].
//! Reading the context and writing the result is the binary's job
//! ([`crate::coding_agent`]
//! only models and runs the agent); this module owns the schema and the agent
//! loop.
//!
//! # Capability / role awareness
//!
//! The three reference-delivery roles map to distinct capabilities:
//!
//! - **engineer** (`coding_workspace`): use edit tools to implement the issue,
//!   leaving a real product diff in the working tree. Successful implementation
//!   follows the no-verdict head path to `open_pr`.
//! - **architect** (`triage_workspace`): perform read-only repository analysis
//!   and author the workflow product required by the declared outcome.
//! - **reviewer** (`review_workspace`): perform a read-only review of the actual
//!   diff and CI evidence against the base branch.
//!
//! Role duties are invariant, while outcome vocabulary is workflow-dependent.
//! When temper supplies `allowed_verdicts` (W3), the prompt names only those
//! outcomes and renders product requirements only from their `VerdictContract`
//! entries. When no outcomes are supplied, the prompt uses the legacy per-role
//! menus (`needs_architect` / `needs_human`; `ready_code` / `needs_design` /
//! `needs_breakdown`; or `approve` / `changes` / `escalate`) for compatibility.
//! Optional named guidance is added only after the provider tool registry is
//! finalized, so submit, Forge, codebase-memory, and sub-agent prose cannot
//! advertise an unavailable tool. The engineer's no-verdict success path
//! remains available in both outcome modes, and
//! any emitted verdict outside a non-empty declared vocabulary is rejected.
//!
//! This file is a thin facade: the role→capability mapping, errors, prompt
//! construction, tool/sub-agent wiring, and the agent run + parse/validate logic
//! live in the submodules below and are re-exported here.

mod capability;
mod codebase_memory;
mod error;
mod forge;
mod prompt;
mod result;
mod run;
mod submit;
mod tools;

// Re-export of the test-visible symbols and the public API so callers (and the
// `super::*` in the test module) see one flat `coding_agent` surface.
pub use capability::Capability;
pub use error::{AgentAbortAuthority, CodingAgentError};
pub use forge::{ForgeContextFuture, ForgeContextHost};
pub use prompt::{system_prompt, system_prompt_with_contracts, user_context};
pub use run::{
    run_coding_agent_native, run_coding_agent_native_with_options,
    run_coding_agent_native_with_options_and_submit_for_pr,
    run_coding_agent_native_with_options_tool_config_and_submit_for_pr,
    run_coding_agent_native_with_submit_for_pr, run_coding_agent_native_with_tool_config,
    run_coding_agent_native_with_totals, run_coding_agent_native_with_totals_and_submit_for_pr,
    run_coding_agent_native_with_totals_tool_config_and_hosts,
    run_coding_agent_native_with_totals_tool_config_and_submit_for_pr,
    run_coding_agent_native_with_totals_tool_config_hosts_and_containment,
};
pub use submit::{
    SubmitForPrCallback, SubmitForPrFuture, SubmitForPrHost, bind_submit_for_pr_host,
    default_submit_for_pr_host, submit_for_pr_available,
};
pub use tools::tool_registry;

// Internal items the unit tests reach through `super::*`.
#[cfg(test)]
pub(crate) use codebase_memory::codebase_memory_prompt_section;
#[cfg(test)]
pub(crate) use prompt::{system_prompt_with_registry, user_context_with_registry};
#[cfg(test)]
pub(crate) use result::{parse_result, validate_contract, validate_verdict_vocabulary};
#[cfg(test)]
pub(crate) use run::{classify_model_failure, classify_run_error, ensure_completed_outcome};
#[cfg(test)]
pub(crate) use tools::{SubAgentTier, add_subagents, subagent_specs, tool_registry_for_context};

// The provider/tool types the unit tests construct through `super::*`.
#[cfg(test)]
pub(crate) use crate::provider::ProviderConfig;
#[cfg(test)]
pub(crate) use tongs::tools::ToolRegistry;

/// Default ceiling on tool-using iterations for one workspace run. The agent
/// must do real multi-step work (read, edit, verify) on substantial work items,
/// so this is well above the tool-less decision path's ceiling of 1, but bounded
/// so a confused run cannot loop forever. Raised to 250 so the engineer can take
/// larger, self-contained work items without exhausting the budget mid-run (we
/// otherwise pay the per-round-trip cost of over-splitting issues).
pub const DEFAULT_MAX_ITERATIONS: usize = 250;

// ---------------------------------------------------------------------------
// Wire DTOs — the worker ↔ agent process protocol.
//
// The context (input, the `--context` file) and result (output, the `--result`
// file) shapes are owned by the serde-only
// `smith-agent-protocol` crate — the contract a third-party agent speaks and
// the worker consumes without linking anvil's internals. We re-export them here
// so this crate's API (and all its callers) are unchanged by the move. The
// result shape must still match temper's `WorkspaceResult` /
// `WorkspaceResultChild` exactly (temper deserializes with
// `deny_unknown_fields`).
// ---------------------------------------------------------------------------

pub use temper_protocol_agent::{
    AgentSessionState, WorkspaceContext, WorkspaceGuidance, WorkspaceRepository, WorkspaceResult,
    WorkspaceResultChild, WorkspaceWorkItem,
};

#[cfg(test)]
mod tests;
