# Descendant-containment acceptance

This is the aggregate traceability record for issue #445 and the architecture in
#448. The executable capstone is
`temper-testing/tests/descendant_containment_acceptance.rs`; it accepts only
runtime PASS evidence emitted after the compiled fixture crosses each
production boundary. It does not inspect source strings or compose unrelated
unit-test names into an acceptance claim.

## Fixture and backend contract

`temper-descendant-fixture` is a compiled Rust binary. No test recursively runs
Cargo. Its `parent` mode runs as a fresh process-group and session leader and
starts a nested session whose argv shape is `temper-agent-shaped`. Parent and
child append PID, PPID, PGID, SID, executable, and `/proc` start tick to one
identity log. The nested process can inherit ignored TERM and waits for an
explicit post-completion mutation trigger.

The same binary's `monitor` mode is spawned outside the containment under test.
It polls every recorded `(PID, start_tick)`, records transitions to PPID 1, and
treats a different start tick as PID reuse rather than survival. Tests stop and
join the monitor only after the production completion boundary. They then
require every exact identity to be absent, no mutation, and one terminal cleanup
observation. The dedicated Linux supervisor must additionally prevent every
PPID-1 transition because it owns descendants as a subreaper. A delegated
cgroup remains the kernel ownership boundary across ordinary reparenting, so
its proof is recursive cgroup emptiness plus exact process absence rather than
unchanged process parentage.

`temper-containment-acceptance` dispatches the supervisor's private early-main
protocol and runs the fixture matrix. Forced Linux-supervisor selection always
runs. Auto runs with cgroup v2 when a delegated writable subtree is available;
otherwise stdout contains a named `CGROUP SKIP` with the capability reason.
The selector is factory-instance scoped, so the two runs cannot race through
process environment. The same driver starts a capacity-one worker and its real
machine/shell, watchdog, outbox, active-job registry, and signal shutdown helper.
Generated Cargo test harnesses never substitute a process-group kernel: agent
Cargo tests use a compiled helper with production Auto or an explicitly
injected forced supervisor, while worker Cargo tests route the same real
supervisor through their compiled custom-harness fixture.

Every dynamic case requires TERM evidence, a recovered disposition, a bounded
survivor identity with start tick and executable, recursive-empty proof, exact
direct-child reap, and joined output/side-channel ownership before return.
Ignored TERM normally exercises KILL; deterministic injected-kernel tests make
KILL-attempt reporting non-racy. Ordinary empty cleanup
is debug evidence. Recovered descendants and fallback activation are warnings.
Blocked cleanup is throttled operator evidence and remains non-terminal.

Ownership-loss cancellation uses the same descendant owner rather than a
parallel cleanup mechanism. The worker first closes the exact attempt's shared
fence, then requests model/tool and process cancellation. In-process and
out-of-process Forge, submit, managed workspace/git, validation, commit, push,
and result paths all reject completions after that boundary. Capacity remains
occupied while descendants, side channels, managed commands, and trace
forwarding join. The terminal `RunFinished(Cancelled)` record is persisted and
its exact sequence is acknowledged by the engine journal before
`AttemptQuiesced`, durable canceled-result recording, heartbeat removal, or
capacity release. The ordinary 250 ms terminal flush allowance is not a
cancellation quiescence proof: an unavailable daemon or failed forward leaves
the attempt in `CleanupPending` with its fence, registry entry, heartbeat
membership, and permit retained while the forwarder retries. Trace capture
`off` remains the explicit no-trace compatibility path. Accepted submit proof is
cleared both before and after joins.

## Production regression matrix

| Requirement | Deterministic test | Backend coverage | Completion boundary | Operator-visible evidence |
| --- | --- | --- | --- | --- |
| Managed bash direct success | `compiled_fixture_crosses_every_production_completion_boundary` → `managed_bash_success`; focused `normal_exit_waits_for_detached_session_cleanup_and_reader_join` | Auto cgroup when delegated; forced supervisor always | `Tool::execute` cannot return output until exact absence, output-reader join, and one cleanup report | `worker.containment.cleanup_completed`; recovered cleanup is warning |
| Managed bash tool deadline | Capstone `managed_bash_deadline`; focused `explicit_tool_timeout_waits_for_cleanup_and_reader_join` | Auto cgroup when delegated; forced supervisor always | Timeout output follows TERM, KILL, recursive-empty proof, and reader join | completed event includes timeout trigger and signal identities |
| Capacity-one no-progress or ownership-loss cancellation | Capstone `capacity-one-watchdog` starts the production worker machine/shell with a queued second assignment and held nested fixture; exact-attempt ownership loss enters the same cancellation path | Auto cgroup when delegated; forced supervisor always | The transport records exactly one initial poll while cancellation/cleanup is active; no later poll, permit, or second executor dispatch is observed until exact descendant absence, canceled trace forwarding, and the durable canceled-result path release capacity | `worker.job.timeout` or ownership-loss reason, `worker.job.cancellation_completed`, then `worker.capacity.released` |
| Out-of-process agent normal completion and failure | Capstone `out_of_process_agent` with exit 0 and 17; focused `worker_descendant_containment_contract` | Auto cgroup when delegated; forced supervisor always | Result-file acceptance and error return follow nested removal, stderr join, endpoint stops, and `JobQuiesced` | `worker.job.quiesced` and cleanup-completed evidence |
| Split worker and standalone signal shutdown | Capstone `split-signal-shutdown` and `standalone-signal-shutdown` start held compiled fixtures and enter `shutdown_worker_after_signal`, the helper called by both service composition roots | Auto cgroup when delegated; forced supervisor always | Signal intake closes, the active attempt fence and containment join, and only then may standalone assignment release run; split shutdown publishes no cancellation result and preserves the durable claim | cancellation/cleanup events; blocked cleanup retains shutdown wait |
| Submit/pre-push and worker-managed commands | Capstone `worker-managed-command` invokes the exact bounded owner used by git/fingerprint effects; `pre-push` invokes the controlled production gate owner with the same compiled fixture | Auto cgroup when delegated; forced supervisor always through instance-scoped worker-command selection | Gate/command result follows recursive cleanup and bounded stdout/stderr joins; complete git overflow remains an explicit error | cleanup-completed or cleanup-blocked with owner scope |
| TERM failure, KILL escalation, survivor and inspection faults | Capstone ignored-TERM cases plus `inspection-recovery`, which decorates the selected real backend kernel and routes its blocked snapshot through the attempt owner and active-job registry | Auto cgroup when delegated; forced supervisor always | Failed inspection leaves the registry `CleanupPending`, the queued assignment undispatched, and capacity occupied until inspection recovery and recursive-empty proof | structured TERM/KILL outcomes, bounded PID/PPID/PGID/SID/executable; throttled `worker.containment.cleanup_blocked` |
| Exact bounded `non_completed_stop` | `worker_abort_exits_nonzero_without_result_and_names_stable_reason` | Explicit helper-capable forced Linux supervisor around the test-owned agent; production Auto containment remains active for the agent's nested bash tool | Cancellation waits until a nested `setsid` child has published PID/start identity; agent exit, recursive-empty proof, exact child absence, and lifecycle join precede assertion; no result is accepted | typed `aborted` / `worker_requested` failure, bounded diagnostics |

The exact abort regression also records the nested session child's PID and
`/proc` start tick, rejects a zero start identity, and checks that the exact
identity is absent when contained agent completion returns. PID reuse therefore
counts as absence rather than allowing a stale numeric-PID assertion.

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
| Ownership-safe startup recovery | `startup_of_second_live_owner_does_not_signal_first_owner`, `zero_process_boot_fences_are_retained_without_signaling`, and `missing_owner_fence_disables_cgroup_and_preserves_auto_fallback` | Validated PID/start-time fences protect live owners; malformed roots remain untouched; an unavailable fence selects the supervisor fallback |
| Descendant-complete fallback, concurrent containments, PID reuse | `concurrent_nested_sessions_are_reaped_once`, `owner_channel_loss_cleans_the_containment`, and `pid_reuse_is_structured_and_never_signals_the_reused_identity_as_the_old_process` | Dedicated subreaper adopts nested sessions; exact start identity gates signals |
| Cleanup proof and exactly-once coordination | `cleanup_runs_exactly_once_and_first_trigger_wins`, `blocked_inspection_cannot_complete_cleanup`, and capstone observer count | Completion requires terminal reap plus `RecursiveEmptyProof::Proven` |
| Managed bash and MCP ownership | Managed-bash tests above; `cancellation_reaps_the_mcp_server_grandchild_group`, `request_timeout_waits_for_recursive_cleanup_and_reader_join` | Tool/MCP result follows containment and stream joins |
| Agent, managed git/fingerprint, and pre-push owners | Agent and command rows above | Outer job remains final safety net; each inner owner gates its own output |
| Active-job registry and service shutdown | Capstone capacity-one watchdog, inspection recovery, and both signal-shutdown cases; split worker and standalone call the same `shutdown_worker_after_signal` ordering helper | Worker registry join is exercised with the live compiled fixture, not inferred from source ordering | Worker joins before standalone `release_assignments_for_shutdown`; split worker waits without a fail-open timeout |
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
