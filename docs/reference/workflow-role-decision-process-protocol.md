# Workflow role decision process protocol

Temper owns workflow authority. An external workflow-role decision process may
choose one manifest action, but Temper validates the reply against the compiled
role manifest and executes only through `RoleTools`. `temper-runner` provides the
provider-neutral `WorkflowRoleDecisionProcessAgent` adapter; it has no Smith,
pi-SDK, or provider-auth dependency.

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
belong on stderr. Extra stdout makes the reply malformed. The adapter clears the
child environment and copies only explicitly allow-listed variable names.

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

Temper treats `reason` as operator/debug text only. Replies with malformed JSON,
extra JSON, duplicate reply fields, unknown reply fields, or a mismatched
`protocol_version` fail the role decision. An action outside `authorized_actions`
is validated and then treated as no-action to match the existing generic
role-agent behavior for unauthorized model output.

## Production worker selection

`temper-worker` keeps the in-process LLM role agents by default. A role worker
uses the process adapter only when explicitly configured with
`--role-decision-command` or `TEMPER_WORKER_ROLE_DECISION_COMMAND`. Matching
options are `--role-decision-arg`, `--role-decision-cwd`,
`--role-decision-env`, and `--role-decision-timeout-secs`; environment fallbacks
are `TEMPER_WORKER_ROLE_DECISION_ARGS_JSON`,
`TEMPER_WORKER_ROLE_DECISION_CWD`,
`TEMPER_WORKER_ROLE_DECISION_ENV_ALLOWLIST`, and
`TEMPER_WORKER_ROLE_DECISION_TIMEOUT_SECS`. Do not allow-list Forge tokens or
Temper-owned provider secrets; Smith/provider credentials should be supplied
through Smith-owned env/auth paths rather than broad ambient env. Temper never
passes Forge handles or workflow mutation tools.
