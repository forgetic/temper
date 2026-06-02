# Workflow role decision process protocol

Temper owns workflow authority. An external workflow-role decision process may
choose one manifest action, but Temper validates the reply against the compiled
role manifest and executes only through `RoleTools`.

## Types and fixtures

The provider-neutral contract lives in `temper-runner`:

- `WorkflowRoleDecisionRequest`
- `WorkflowRoleDecisionReply`
- `AuthorizedWorkflowAction`

Version-1 JSON fixtures live at:

- `crates/temper-runner/fixtures/workflow-role-decision-request.json`
- `crates/temper-runner/fixtures/workflow-role-decision-reply.json`

## Invocation shape

Temper writes one UTF-8 JSON request to stdin, appends a newline, and closes
stdin. The decision process writes exactly one UTF-8 JSON reply to stdout. Logs
belong on stderr. Extra stdout makes the reply malformed.

A request contains:

- `protocol_version` (`1` today);
- `workflow_id`;
- the compiled `role_manifest` Temper is enforcing;
- fresh `work_item_context` JSON;
- compact `authorized_actions` derived from the manifest;
- `available_external_tools` metadata that survived declaration and runner
  binding validation.

A reply contains:

- `protocol_version`;
- `action`, either one authorized action name or `no_action`;
- `reason`, a short diagnostic string.

## Authority and secrets

The process receives no Forge token, provider secret, broad Forge handle, or
workflow mutation tool. External-tool entries are metadata only. If a chosen
action needs an executable external tool, Temper's runner invokes the bound
provider after validating the action.

Temper rejects replies with a mismatched protocol version or an action outside
`authorized_actions`; it treats `reason` as operator/debug text only.
