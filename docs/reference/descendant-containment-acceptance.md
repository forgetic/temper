# Descendant-containment acceptance

This is the aggregate traceability record for issue #445 and the architecture in
#448. The executable capstone is
`temper-testing/tests/descendant_containment_acceptance.rs`; focused tests remain
the authority for injected races and component-specific state machines.

## Fixture and backend contract

`temper-descendant-fixture` is a compiled Rust binary. No test recursively runs
Cargo. Its `parent` mode runs as a fresh process-group and session leader and
starts a nested session whose argv shape is `temper-agent-shaped`. Parent and
child append PID, PPID, PGID, SID, executable, and `/proc` start tick to one
identity log. The nested process can inherit ignored TERM and waits for an
explicit post-completion mutation trigger.

The same binary's `monitor` mode is spawned outside the containment under test.
It polls every recorded `(PID, start_tick)`, records any transition to PPID 1,
and treats a different start tick as PID reuse rather than survival. Tests stop
and join the monitor only after the production completion boundary. They then
require every exact identity to be absent, no PPID-1 observation, no mutation,
and one terminal cleanup observation.

`temper-containment-acceptance` dispatches the supervisor's private early-main
protocol and runs the fixture matrix. Forced Linux-supervisor selection always
runs. Auto runs with cgroup v2 when a delegated writable subtree is available;
otherwise stdout contains a named `CGROUP SKIP` with the capability reason.
The selector is factory-instance scoped, so the two runs cannot race through
process environment.

Every dynamic case requires TERM evidence, a recovered disposition, a bounded
survivor identity with start tick and executable, recursive-empty proof, exact
direct-child reap, and joined output/side-channel ownership before return.
Ignored TERM normally exercises KILL; deterministic injected-kernel tests make
KILL-attempt reporting non-racy. Ordinary empty cleanup
is debug evidence. Recovered descendants and fallback activation are warnings.
Blocked cleanup is throttled operator evidence and remains non-terminal.

## Production regression matrix

| Requirement | Deterministic test | Backend coverage | Completion boundary | Operator-visible evidence |
| --- | --- | --- | --- | --- |
| Managed bash direct success | `compiled_fixture_crosses_production_completion_boundaries` → `managed_bash_success`; focused `normal_exit_waits_for_detached_session_cleanup_and_reader_join` | Auto cgroup when delegated; forced supervisor always | `Tool::execute` cannot return output until exact absence, output-reader join, and one cleanup report | `worker.containment.cleanup_completed`; recovered cleanup is warning |
| Managed bash tool deadline | Capstone `managed_bash_deadline`; focused `explicit_tool_timeout_waits_for_cleanup_and_reader_join` | Auto cgroup when delegated; forced supervisor always | Timeout output follows TERM, KILL, recursive-empty proof, and reader join | completed event includes timeout trigger and signal identities |
| Capacity-one no-progress watchdog | `no_progress_timeout_quiesces_records_once_then_releases_capacity` plus capstone `out_of_process_cancellation` using the same nested fixture | Process cleanup runs under both injected backends; machine gating is backend-independent | No poll, permit release, result, or second dispatch before `AttemptQuiesced` and durable result recording | `worker.job.timeout`, `worker.job.cancellation_completed`, then `worker.capacity.released` |
| Out-of-process agent normal completion and failure | Capstone `out_of_process_agent` with exit 0 and 17; focused `worker_descendant_containment_contract` | Auto cgroup when delegated; forced supervisor always | Result-file acceptance and error return follow nested removal, stderr join, endpoint stops, and `JobQuiesced` | `worker.job.quiesced` and cleanup-completed evidence |
| Split worker and standalone signal shutdown | `shutdown_joins_active_job_without_publishing_a_cancellation_result`, `shutdown_applies_forced_and_hard_deadlines_before_joining`, and capstone held-agent cancellation | Fixture cleanup under both backends; task registry and entrypoint ordering are backend-independent | Active attempt fence closes; containment and task group join before worker return or assignment release | cancellation/cleanup events; blocked cleanup retains shutdown wait |
| Submit/pre-push and worker-managed commands | Capstone `run_pre_push_case`; `cancellation_joins_pre_push_before_late_workspace_mutation`; `dropping_command_kills_and_joins_before_late_mutation` | Pre-push production Auto plus shared primitive under forced supervisor; command owner logic is common | Gate/git/fingerprint result follows recursive cleanup and bounded stdout/stderr joins; complete git overflow is an error | cleanup-completed or cleanup-blocked with owner scope |
| TERM failure, KILL escalation, survivor and inspection faults | Capstone ignored-TERM cases; `reports_bound_survivors_attempts_and_diagnostics`; `blocked_inspection_cannot_complete_cleanup`; `cleanup_blocked_retains_fence_permit_and_rejects_unproven_completion` | Shared deterministic kernel plus both live Linux backends | Failed inspection has no terminal report; capacity, attempt fence, and permit remain held until recovery | structured TERM/KILL outcomes, bounded PID/PPID/PGID/SID/executable; throttled `worker.containment.cleanup_blocked` |
| Exact bounded `non_completed_stop` | `worker_abort_exits_nonzero_without_result_and_names_stable_reason` | Production Auto containment (cgroup or supervisor fallback) | Agent exit and lifecycle join precede assertion; no result is accepted | typed `aborted` / `worker_requested` failure, bounded diagnostics |

The exact abort regression names and asserts these limits:

- `ABORT_CANCELLATION_DEADLINE` and `ABORT_MAX_ELAPSED`;
- `CAPTURED_PROCESS_OUTPUT_BYTES` per stream;
- `PROVIDER_HISTORY_BYTES`, `PROVIDER_REQUEST_TAIL_BYTES`, and
  `RETAINED_PROVIDER_REQUESTS`;
- `MAX_OBSERVED_FIXTURE_PROCESS_COUNT`.

## #448 architecture traceability

| #448 decision | Deterministic authority | Boundary / evidence |
| --- | --- | --- |
| Prepare before spawn; cgroup Auto and injected supervisor | `prepare_preopens_controls_and_preexec_membership_precedes_payload`, `auto_selection_emits_capabilities_and_uses_the_supplied_fallback`, and this capstone | No attach window; startup capability and fallback warning name selection |
| Descendant-complete fallback, concurrent containments, PID reuse | `concurrent_nested_sessions_are_reaped_once`, `owner_channel_loss_cleans_the_containment`, and `pid_reuse_is_structured_and_never_signals_the_reused_identity_as_the_old_process` | Dedicated subreaper adopts nested sessions; exact start identity gates signals |
| Cleanup proof and exactly-once coordination | `cleanup_runs_exactly_once_and_first_trigger_wins`, `blocked_inspection_cannot_complete_cleanup`, and capstone observer count | Completion requires terminal reap plus `RecursiveEmptyProof::Proven` |
| Managed bash and MCP ownership | Managed-bash tests above; `cancellation_reaps_the_mcp_server_grandchild_group`, `request_timeout_waits_for_recursive_cleanup_and_reader_join` | Tool/MCP result follows containment and stream joins |
| Agent, managed git/fingerprint, and pre-push owners | Agent and command rows above | Outer job remains final safety net; each inner owner gates its own output |
| Active-job registry and service shutdown | `graceful_shutdown_fences_and_cancels_every_active_attempt`, `cleanup_pending_keeps_the_registry_join_blocked`; split worker `run` and standalone shutdown ordering | Worker joins before standalone `release_assignments_for_shutdown`; split worker waits without a fail-open timeout |
| Bounded process/provider records | Exact abort regression plus `complete_capture_fails_after_draining_overflow`, MCP oversized-record tests, and `process-output-inventory.md` | Complete machine output overflows explicitly; human diagnostics retain bounded tails |
| Observability and deployment | `cleanup_events_have_expected_severity_bounded_evidence_and_redaction`, `repeated_blocked_cleanup_is_throttled_by_root`, systemd deployment tests | Ordinary success debug; fallback/recovery warning; blocked cleanup warning/error with throttling |

## Aggregate verification

Run the focused suites once from the workspace root:

```text
cargo test -p temper-process-containment -p temper-agent-core \
  -p temper-agent -p temper-worker
cargo test -p temper-testing --test descendant_containment_acceptance
cargo test -p temper-agent-session --test non_completed_stop \
  worker_abort_exits_nonzero_without_result_and_names_stable_reason -- --exact
```

The first command compiles the fixture and driver as ordinary Cargo binary
targets before libtest starts. Neither binary invokes Cargo.
