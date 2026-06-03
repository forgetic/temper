# Reference-delivery observability guide

Use this page while `./run.sh start` is still running. `./run.sh stop` removes
the throwaway Forgejo data, so Forge-state validation must happen before
teardown. Logs stay under `logs/` for later inspection.

## Where to look

- **Worker startup and capabilities** — each worker log starts with
  `worker_capabilities` JSON plus a `temper-worker: resolved repositories ...`
  line. Check `worker_kind`, `worker`, `role`, `repositories`,
  `authorized_actions`, and `available_external_tools`.
- **Scan and work-item selection** — role worker logs emit `scan_summary` and
  `work_item_selected` when a scan finds work. The important join fields are
  `tick_id`, `work_item_id`, `decision_id`, `repo`, `role`, `queue`,
  `artifact_type`, `artifact_number`, and `artifact_kind`.
- **Smith decision correlation** — Temper emits `role_decision_request` before
  invoking Smith and `role_decision_reply` after Smith returns. The same
  authority-neutral observability fields are nested in
  `work_item_context.observability`, so Smith logs or captures can be joined by
  `decision_id`/`work_item_id`. Smith receives metadata only: no Forge token,
  Forge handle, or mutation tool. Smith-owned provider details are documented in
  `~/src/rust/smith/plans/observability/README.md`.
- **Action and transition outcomes** — `action_dispatch` records the selected
  manifest action, transition id, and external-executor availability.
  `transition_execution` records `outcome`, `stale_work`, compact effect
  summaries, failure class, diagnostic classes, and postcondition outcome.
- **Mechanical reconciliation** — `mechanical.log` emits
  `mechanical_reconciliation` for reconciler findings/actions. A blocked code
  issue with no dependency relations is named as
  `diagnostic=blocked_artifact_without_dependencies` with `dependency_count=0`;
  this explains why dependency-gated unblocking intentionally does not proceed.
- **Validator diagnostics** — `./run.sh validate-multi-repo` checks logs and live
  Forge state. For the original incident shape, expect messages like:

  ```text
  missing: cross-repo parent acme/service#1 expected 2 child dependencies, found 0
  diagnosis: architect blocked the parent but no fan-out side effects were recorded
  missing: blocked parent acme/service#1 has zero dependencies
  diagnosis: dependency-gated unblocking intentionally cannot proceed without at least one recorded dependency
  ```

## Minimal movement trail

For one moving item, the expected trail is:

```text
worker_capabilities -> scan_summary -> work_item_selected
role_decision_request -> role_decision_reply -> action_dispatch
transition_execution -> completed tick ... tick_id=...
```

For a stuck cross-repo parent, add:

```text
mechanical_reconciliation diagnostic=blocked_artifact_without_dependencies
validate-multi-repo missing: ... expected N child dependencies, found 0
```

All event payloads are bounded and omit full bodies, comments, provider args,
auth paths, and secrets.
