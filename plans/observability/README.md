# Observability story — implementation plan

This plan turns the 2026-06-03 reference-delivery stall into the motivation for
strong observability across Temper and Smith. The evidence lives in
[`evidence.md`](evidence.md): a cross-repo parent issue moved to `code,blocked`,
no child issues or dependencies were created, and operators had to inspect logs,
Forgejo state, shell launchers, workflow config, Temper's production adapter, and
Smith behavior to understand why.

Hand the prompt files to agents one phase at a time, in order. Each phase should
land green and update this README's status.

## Goal

For any stuck workflow, an operator should be able to answer from logs,
Forge-state validators, and optional traces:

- Which worker saw the item, in which repo, queue, role, and tick?
- Which role decision was requested from Smith, with which authorized actions and
  bound external tools?
- Which action did Smith choose, and why?
- Which Temper transition ran, which gates/preconditions were checked, and which
  effects landed?
- Which expected side effects were absent, such as fan-out children or dependency
  links?
- Which invariant explains why mechanical recovery did or did not proceed?

For the incident in `evidence.md`, diagnostics should have made this obvious:

```text
repo=acme/service issue=1 role=architect queue=intake
selected_action=triage_to_blocked_code reason=...
transition=triage_to_blocked_code effects=labels(intake->code,blocked)
warning=blocked_artifact_without_dependencies dependency_count=0
warning=cross_repo_parent_without_children expected_children=2 observed_children=0
```

## Ownership boundary

- Temper owns Forge/workflow observability: scans, queues, compiled authority,
  process-adapter validation, transition execution, reconciliation, Forge-state
  validators, and operator demo logs.
- Smith owns LLM/provider observability: prompt/context construction, provider
  and model identity, latency, model decision, final reply, unauthorized-action
  downgrade, parse/provider failures, and optional redacted captures.
- Both sides share a small trace identity propagated through Temper's
  `work_item_context`; Temper must not depend on Smith as a Rust crate.
- Secrets never appear in logs, captures, argv, or failure messages. Reasons and
  bodies should be previewed/truncated when logged.

## Event shape

Use stable structured text or JSON events before introducing any telemetry
backend. Each event should carry the fields it can know:

- `event`, `timestamp`, `run_id`, `tick_id`, `work_item_id`, `decision_id`
- `worker_kind`, `worker`, `workflow_id`, `role`, `queue`
- `repo`, `artifact_type`, `artifact_number`, `artifact_kind`
- `authorized_actions`, `available_external_tools`, `selected_action`
- `transition`, `gates`, `effects`, `outcome`, `latency_ms`, `reason_preview`

Important Temper events:

- worker startup/capability summary
- scan summary and work-item selection
- role-decision request/reply/validation
- transition execution start/finish/postcondition failure
- mechanical reconciliation finding/action
- named invariant warning

## Phases

Status legend: ☐ pending · ☑ done · ⚠ blocked

1. ☑ **Phase 1 — Trace context and structured event foundations.**
   `prompts/phase-1-trace-context-and-event-foundations.md`

   Add a provider-neutral trace/work-item identity and stable structured event
   formatting in Temper. Propagate the trace through `work_item_context` without
   granting authority or leaking secrets.

2. ☑ **Phase 2 — Role-decision and transition execution logs.**
   `prompts/phase-2-role-decision-and-transition-logs.md`

   Instrument the production role-decision adapter and action execution path so
   `actions=N` is supplemented by per-work-item decision, action, transition,
   stale/no-op, and outcome diagnostics.

3. ☐ **Phase 3 — Reconciliation invariants and Forge-state validator.**
   `prompts/phase-3-reconciliation-invariants-and-validator.md`

   Make blocked-without-dependencies and missing cross-repo fan-out explicit
   diagnostics. Extend `examples/reference-delivery/run.sh validate-multi-repo`
   to inspect Forge state, not only process logs.

4. ☐ **Phase 4 — Operator docs and observability smoke proof.**
   `prompts/phase-4-operator-docs-and-smoke-proof.md`

   Document the observability story, wire it into the reference-delivery how-to,
   and add a focused smoke/e2e proof that a stuck or moving workflow leaves an
   intelligible trail.

## Whole-plan acceptance criteria

- A reference-delivery run exposes enough information to diagnose the incident
  in `evidence.md` without reading Temper or Smith source code.
- Smith decisions can be correlated with Temper work items by shared IDs.
- Validators fail loudly for a blocked parent with zero dependency relations and
  for cross-repo parent issues with missing children.
- Logs distinguish no work, deliberate `no_action`, unauthorized model action,
  stale work, execution failure, and successful mutation.
- Logs and captures are redacted and bounded.
- Default tests remain hermetic; live Forgejo/provider checks stay explicitly
  env-gated.

## Relevant starting points

- `plans/observability/evidence.md`
- `docs/reference/workflow-role-decision-process-protocol.md`
- `docs/reference/workflow-layer.md`
- `docs/reference/cross-repo-workflows.md`
- `docs/how-to/run-cross-repo-reference-delivery-demo.md`
- `crates/temper-runner/src/{scan,worker,role_decision,role_decision_process,role_process_tools}.rs`
- `crates/temper-workflow/src/{execute,reconcile,recover}.rs`
- `crates/temper-production/src/worker.rs`
- `examples/reference-delivery/run.sh`
- Smith sibling plan: `~/src/rust/smith/plans/observability/README.md`
