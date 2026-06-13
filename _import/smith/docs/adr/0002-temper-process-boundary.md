# ADR 0002: Keep concrete LLM responders behind Temper process protocols

## Status

Accepted.

## Context

Temper owns workflow and interaction runtime contracts. The concrete LLM
implementation originally lived in Temper and pulled provider SDKs, auth-file
quirks, live-provider tests, and profile prompt behavior into the core runtime
repository.

That coupling made Temper sensitive to model-provider churn and risked blurring
the authority boundary between LLM choice and workflow mutation.

## Decision

Smith owns concrete pi-SDK-backed responders. Temper calls them through
provider-neutral process protocols:

- interactive responder: `ConversationRequest` → `ConversationReply`;
- workflow-role decision: `WorkflowRoleDecisionRequest` →
  `WorkflowRoleDecisionReply`.

Smith receives request context, authorized action/proposal metadata, and provider
credentials from its own environment/auth files. It does not receive Forge
handles, Forge tokens, broad SDK tools, or workflow mutation tools.

## Consequences

Temper can build and test without `pi_agent_rust` or provider auth logic. Smith
can change provider implementation details without changing Temper's runtime
contracts. Every state change still goes through Temper validation and
Temper-owned mutation paths.
