# Workflow-role observability

Smith emits provider-side observability for Temper workflow-role decisions. Temper
still owns worker scans, Forge state, transition validation/execution, gates,
leases, and every Forge mutation. For Temper's process contract and worker-side
events, read the sibling checkout docs:

- `../temper/docs/reference/workflow-role-decision-process-protocol.md`
- `examples/reference-delivery/observability.md` (daemon/worker example logs)
- `plans/observability/README.md`

## Structured stderr events

`anvil-workflow-role-decision` writes one JSON object per line on stderr. Stdout
remains the single Temper `WorkflowRoleDecisionReply` JSON value.

Common fields, when Temper supplied them in
`work_item_context.observability`, are `run_id`, `tick_id`, `work_item_id`,
`decision_id`, `repository`, `role`, `work_item_role`, `queue`, `kind`,
`artifact_type`, and `artifact_number`. Smith also logs non-secret provider
identity: `provider`, `model`, and `auth_mode`.

Events:

- `anvil.workflow_role_decision.request` — allowed actions, bound external-tool
  ids, prompt/context character counts, workflow/role/work-item identity.
- `anvil.workflow_role_decision.provider_call.start` — provider call start with
  the same identity fields.
- `anvil.workflow_role_decision.provider_call.finish` — `latency_ms`, `outcome`,
  `model_action` on success, or compact provider/parse failure classes.
- `anvil.workflow_role_decision.reply` — `outcome`, `model_action`,
  `returned_action`, `unauthorized_action_downgraded`, and bounded
  `reason_preview`.
- `anvil.workflow_role_decision.capture.written` / `.capture.write_failed` — the
  capture path or a bounded warning when optional capture writing failed.

Join Smith events to Temper `role_decision_request` / `role_decision_reply`
events by `decision_id` first, then `work_item_id`; use `tick_id` when present
and `repo`/`repository`, `role`, `queue`, and artifact fields as checks.

## Optional redacted captures

Captures are disabled by default. Enable them only for an operator debugging run:

```sh
mkdir -p /tmp/anvil-role-captures
export ANVIL_WORKFLOW_ROLE_DECISION_CAPTURE_DIR=/tmp/anvil-role-captures
```

When Temper launches the responder, allow-list that one variable in the worker's
role-decision process configuration. Smith requires the directory to already
exist and writes one bounded JSON file per decision. Missing or unwritable
capture directories only produce warning logs; they do not require captures or
change a successful decision.

Each capture contains trace/work-item ids, workflow/repo/role/queue/artifact
metadata, provider/model/auth mode, allowed actions, external-tool ids,
prompt/context sizes with redacted previews, parsed model decision, final reply,
latency, outcome, and failure class. It does not contain provider credentials,
auth-file contents, Forge tokens, raw environment dumps, process argv, full
prompts, or unbounded issue/comment bodies.

## Fields Smith intentionally does not log

Because Temper owns workflow and Forge authority, Smith events do not include:

- Forge tokens, Forge handles, role credentials, or mutation tools;
- transition effect bodies, dependency graph state, lease state, gate details, or
  postcondition results;
- child issue/PR creation results, reviewer/merge execution, or reconciliation
  actions;
- full artifact bodies, comments, raw prompts, raw model payloads, auth-file
  paths, API keys, OAuth access tokens, or refresh tokens.

Look in Temper daemon logs and validators for transition execution,
`action_dispatch`, `transition_execution`, mechanical reconciliation, and
reference-delivery Forge-state diagnostics.

## Secret handling

Smith logs only provider id, model id, and auth mode. Credentials are read from
Smith-owned auth files or env surfaces, but token bytes and auth-file contents are
never formatted into structured events or captures. Free-form reason, error, and
capture previews are bounded and pass through Smith's secret-like redactor.
