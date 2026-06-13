# Process responders

Smith provides provider-backed LLM implementations for Temper's process
protocols. Protocol structs, validation, transcripts, proposal acceptance, and
state mutation are Temper-owned.

## Workflow-role decision

Binary:

```text
anvil-workflow-role-decision [--auth ...] [--codex-model MODEL] [--auth-file PATH]
```

Optional capture env (allow-list it in Temper worker process config when the
responder is launched by Temper):

```text
ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR=/existing/writable/dir
```

Input: one `temper_process_protocol::WorkflowRoleDecisionRequest` JSON value on stdin.
Output: one `temper_process_protocol::WorkflowRoleDecisionReply` JSON value on stdout.
Errors/logs go to stderr as bounded JSON events. See
`workflow-role-observability.md` for event names, correlation fields, capture
contents, and omitted authority/secret fields.

Smith reads the role manifest, work-item context, authorized action list, and
bound external-tool metadata. It returns `no_action` or one authorized action
name. Unauthorized model actions are downgraded to `no_action`; unsupported
protocol versions fail before a model call.

Redacted decision captures are disabled by default. When
`ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR` names an existing writable directory,
Smith writes one bounded JSON capture per decision with trace/work-item ids,
workflow/repo/role/queue/artifact metadata, provider/model/auth mode, allowed
actions, external-tool ids, prompt/context sizes and redacted previews, model and
final actions/reason previews, latency, outcome, and failure class. Smith never
captures provider credentials, auth-file contents, Forge tokens, raw environment
dumps, or unbounded prompt/body text. Missing or unwritable capture directories
produce bounded warning logs and do not change a successful decision result.

## Product-manager example interactive profile

Binary:

```text
anvil-product-manager-responder [--auth ...] [--codex-model MODEL] [--auth-file PATH]
```

Input: one `temper_process_protocol::ConversationRequest` JSON value on stdin.
Output: one `temper_process_protocol::ConversationReply` JSON value on stdout.
Errors/logs go to stderr.

This binary is Smith's dogfood/example implementation for the `product-manager`
profile id declared by Temper's interaction profile fixture or dogfood spec. It
rejects other profile ids. Replies contain display text and inert issue-draft
proposals only. Temper's generic interaction service owns transcript storage,
reply/proposal validation, durable proposal snapshots, explicit acceptance, and
issue filing.

The Smith library also exposes a generic `InteractionProfileConfig` /
`GenericInteractionResponder` core for JSON prompt/profile configs. Until the
future `smith-interaction-responder` binary lands, the product-manager binary
remains the compatibility process surface and does not parse Temper acceptance
commands or effects.

## Smith worker

Binary:

```text
smith-worker --daemon-url <url> --worker-id <id> --capability <owner/name>:<role> [--capability ...] [--max-concurrent <n>] [--poll-wait-ms <n>] [--heartbeat-interval-ms <n>]
```

The `smith-worker` process speaks Temper Worker/Daemon Wire Protocol v1 over the
configured daemon URL. It registers all configured `(repo, role)` capabilities,
long-polls for assignments, runs each assignment through the current stub
executor seam, and posts structured `result` messages back to Temper. This
skeleton does not run role-decision/coding agents, manage git workspaces, or
call deploy/Forge services.

Authority boundary: `smith-worker` never receives Forge credentials and never
calls the Forge API. Temper remains authoritative for workflow state, leases,
PR create/update, and all Forge mutations; the worker only exchanges protocol
messages with the daemon.

## Authority boundary

Responder processes receive no Forge credentials or mutation tools. Clients
enter through Temper's generic interaction or workflow services; Temper clears
the child environment except for configured allow-listed names, validates reply
shape/action/proposals, applies timeouts, and executes all state changes through
Temper-owned code.
