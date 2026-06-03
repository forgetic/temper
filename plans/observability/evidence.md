# Observability evidence

## 2026-06-03: reference-delivery parent intake stuck blocked

### Incident summary

Running `examples/reference-delivery/run.sh` with the checked-in multi-repo configuration seeded the source intake issue as `acme/service#1`. The architect worker made one state change, after which the issue remained open with `code,blocked` and the rest of the workflow made no progress.

The root cause was a capability mismatch between the demo expectation and the production worker path:

- `run.sh` launches production `temper-worker` role workers backed by the Smith workflow-role decision process.
- The Smith-backed production architect can only choose one compiled workflow action, such as `triage_to_code`, `triage_to_blocked_code`, or `triage_to_design`.
- No production-bound architect tool existed to create cross-repo child issues or write parent dependency metadata.
- The architect chose `triage_to_blocked_code`, so the parent intake became `code,blocked`.
- No child issues or dependency relations were created.
- The mechanical worker correctly refused to unblock a blocked code issue with zero dependency relations, so the parent stayed blocked forever.

This was easy to reproduce but only moderately easy to diagnose. The worker logs reported `actions=1`, but did not show which transition ran, why the decision was made, or whether expected fan-out side effects were absent. Confirming the cause required inspecting Forgejo state and tracing `run.sh`, `workflow.json`, the production role-decision adapter, and the deterministic `temper-testing` fake architect code.

### Initial observability suggestions

The first suggested improvements were:

- Log each role decision with `repo`, artifact number/type, queue, chosen action, and responder reason.
- Log transition execution outcomes, including the transition id, target artifact, and whether it mutated state.
- Extend `validate-multi-repo` to inspect Forge state, not only logs: child issue count, parent dependency metadata, and blocked parents with zero dependencies.
- Emit a startup warning when cross-repo fan-out is enabled but no executable architect fan-out tool/agent is bound.

### How the completed observability would diagnose this

The current trail would show the architect worker capabilities, a
`work_item_selected`/`role_decision_*` pair for `acme/service#1`, the selected
`triage_to_blocked_code` action, and a `transition_execution` event for the label
move to `code,blocked`. The mechanical worker would then emit
`mechanical_reconciliation diagnostic=blocked_artifact_without_dependencies`,
and `./run.sh validate-multi-repo` would fail with the missing fan-out and
zero-dependency parent diagnostics instead of requiring source inspection.
