# Production worker runtime

In the current two-tier deployment, `temper serve engine` owns webhook intake,
the dedicated CI-status and full role-feed poll backstops, mechanical cadence,
and queue scheduling while one or more `temper serve worker` processes long-
poll it for jobs. `ci_poll_cadence_secs` bounds webhook-less terminal red-repair
and green-landing detection. The positive `ci_missing_grace_secs` (300 seconds
by default) bounds how long no CI run/status for an exact current head may
remain continuously visible before safe parking becomes actionable. Setting
`ci_poll_cadence_secs = 0` disables terminal-CI acceleration, missing-current-
head detection, and missing-CI parking; the grace remains configured and
visible but inactive. `poll_cadence_secs` remains the full correctness/liveness
backstop. `mechanical_cadence_secs` alone does not discover red engineer repair
work. A `ci_failed` queue matches only after every latest-per-name job for the
current PR head is terminal; a visible failure mixed with queued/running work
remains pending.

Forgejo webhooks are posted to the
engine/standalone HTTP surface at `POST /forgejo/webhook` when `[engine]
webhook_secret` or `webhook_secret_file` is configured; there is no separate
`temper serve trigger` process. The older per-role worker, mechanical worker,
and standalone daemon wording below remains useful for legacy or test-only
operation, but operator-facing docs should prefer `serve engine` / `serve
worker`.

## Operating a bounded model-recovery park

Terminal model failures are governed by a durable, finite policy per engineer
workstream. A non-retryable terminal diagnostic consumes the current session
immediately. A retryable terminal diagnostic may run at most three times on the
same session. In either case Temper may rotate automatically **once** to a fresh
session over the unchanged coordination-scoped checkout. If that fresh session
is consumed before an authoritative success, Temper records a permanent
boundary, removes queue eligibility and active claim presentation, adds
`needs-human`, and publishes one deduplicated audit. An authoritative success
starts a new failure epoch. Individual provider stream retries do not count as
worker runs.

The typed model diagnostic and recovery decision travel in the worker result and
are persisted in `.temper-agent-session/state.json`; agent activity capture,
child stderr, and daemon memory are not recovery authority. Daemon or worker
replacement therefore cannot reset the failure count, create a third session,
or make a parked item claimable. Malformed, unsupported, or mismatched existing
session state fails closed and is not overwritten.

For a parked item, use the audit's role, workstream/session identities, attempt,
and `evidence_location` to find the configured worker workspace:

```text
<worker.workspace-root>/<role>/<percent-encoded-coordination-key>/
├── .temper-agent-session/state.json
└── <repository checkout directories...>
```

Inspect the ledger and every checkout in place. Record `git status --short
--branch`, `git diff`, `git diff --cached`, untracked files, the active and prior
session IDs, failure epoch/count, and the normalized provider/model category.
Correct the provider credentials, entitlement, model availability, context
limit, or other session condition outside this directory; then verify the
preserved changes are still the intended product. Never place credentials or
raw provider responses in the audit or ledger.

After correction, deliberately remove `needs-human` and restore the queue label
that the compiled workflow requires for that artifact (for example `ready` for
a code issue). Do not merely clear `needs-human`: without the proper queue label
the item remains intentionally ineligible. Conversely, do not restore a queue
label before the cause is corrected, because that begins a new operator-approved
claim. **Never delete the checkout, `.temper-agent-session`, or its ledger as
routine recovery**, and do not reset, stash-drop, or commit away predecessor
changes just to make the workstream clean. Preserve or copy evidence before any
exceptional manual ledger repair.

## Operating an interrupted-CI park

A provider-reported cancellation, interruption, timeout, runner loss, startup
failure, action-required, neutral/skipped result, or unknown terminalization
stays red but does not enter writable repair. Temper first attempts only the
backend's verified exact-attempt retry capability (GitHub supports it; Forgejo
does not), then at most one configured read-only diagnostic. If those paths are
unsupported or exhausted, the PR receives `needs-human` and one deduplicated
comment.

Before remediation, verify the comment names the PR's current head and includes
run/attempt, latest job IDs and URLs, created/started/completed/updated times,
typed and provider conclusion/reason, provider retry outcome, and diagnostic
outcome. Missing any of that evidence is a reason to leave the PR parked rather
than infer a source failure. Inspect and restore runner/provider infrastructure,
then retrigger CI for that exact head through the provider. Keep the attention
label until a newer exact-head attempt is visible; clear it only to resume
automation after that verification. Do not push an empty commit, rewrite the PR
head, or edit CI workflow/test scripts to manufacture liveness. A newer explicit
ordinary test/build failure is the only result that belongs in the writable CI
repair route. A comment saying no current-head run exists is instead the
separate missing-current-head recovery case and has no run/attempt to retry.

This page records the operator-visible knobs on the Forgejo `temper-worker`
binary. The deployable entrypoint lives in the root `temper` package and
delegates to `crates/temper-worker`; its legacy/internal wake socket support is
shared through `crates/temper-wake`, and optional local-git edits are bound
through `crates/temper-coding-workspace`.

It complements the workflow and Forge references; workflow authority still comes
from the compiled workflow and every mutation still goes through `Forge`.

## Scan cadence

`temper-worker` accepts one or more scan-shard repositories with `--repo
owner/name` or `--repo-list <path>`. The shard is not an authorization boundary:
the Forgejo token decides what the worker may read or mutate.

- `--poll-ms <n>` controls the active normal poll cadence. Role poll ticks scan
  the configured repository set using open-state queue candidate queries only;
  they do not list closed issues or closed/merged pull requests. Mechanical poll
  ticks run bounded reconciliation first, where workflow-labelled terminal
  recovery remains available, then run automated-queue scans for queues with
  `automation` metadata using the same open-state active candidate rule.
- `--idle-poll-max-ms <n>` caps the adaptive idle cadence for mechanical
  workers; the default cap is `60000` ms, raised to `--poll-ms` when the active
  poll interval is already longer. Consecutive successful normal mechanical
  ticks with `changed=false` and `actions=0` keep the first next poll at
  `--poll-ms`, then double the delay up to this cap. Any action/progress, wake
  tick, audit tick, or tick error resets the next normal poll to `--poll-ms`.
- `--audit-ms <n>` controls the low-frequency audit cadence. The default is
  `600000` ms; `0` disables audit ticks. Role audit ticks inspect all configured
  repositories and active/recoverable workflow-labelled recovery interest, but
  still avoid unlabelled closed history and pure identity-only terminal labels.
  Mechanical audit ticks are the explicit deep-audit path and may run
  all-history reconciliation.
- `--wake-socket <path>` plus optional `--wake-secret-file <path>` enables
  authenticated webhook wakeups for the legacy/internal wake-socket topology.
  Pull-request, review, CI/status, label-change, and push hints are all safe
  triggers for the same normal scan path. A wake with
  known repository hints immediately narrows role scans to the hinted configured
  repositories. No-hint or unknown hints fall back to a broad configured-repo
  scan. Mechanical wake scans still visit all configured repositories in
  production so cross-repo recovery can see dependency sources, but each per-repo
  reconciliation and automated-queue scan is bounded.
  `TEMPER_WAKE_DEBOUNCE_MS` can override the default 500ms local wake drain
  window when a deployment or fixture needs different burst coalescing. The
  worker binary reads it once at startup and passes a concrete duration to the
  wake bus (`temper-wake` itself never reads the environment).

Every tick re-reads fresh Forge state before planning or mutating. Engine or
standalone webhooks only accelerate latency; polling and audits remain the
correctness backstops.

## Agent liveness supervision

The deterministic cross-component and restart evidence is indexed in the
[agent-run liveness acceptance matrix](agent-run-liveness-acceptance.md).

`temper serve worker` owns one watchdog state per occupied permit. The effective
settings are visible in the generated configuration template and in
`temper config show`:

- `worker.max_no_progress_secs` (default 900) is reset only by accepted model,
  tool, steering, and terminal lifecycle boundaries; heartbeats do not count;
- optional `worker.max_run_secs` independently bounds the whole attempt;
- `worker.graceful_cancellation_grace_secs` and
  `worker.forced_termination_grace_secs` bound cooperative cancellation and
  escalation requests; a worker still fails closed and retains the attempt
  fence and permit while descendant inspection cannot prove emptiness;
- `deployment.standalone_shutdown_budget_secs` (default 30) is the separate
  absolute signal-to-exit bound for the co-resident `serve standalone` process.
  It must strictly exceed both worker graces plus the fixed 5-second HTTP-drain
  and 5-second final emergency-kill allowances;
- first-party operation limits live in `agent.deadlines` (with profile
  overrides) and must remain below the no-progress bound.

Use `temper config show` to verify resolved values without revealing provider or
worker credentials. During a run, inspect `GET /v1/state` (all workers) or
`GET /v1/state/job/<job-id>` (one assignment). The optional `worker_report`
shows attempt/phase, monotonic run and no-progress elapsed values, at most eight
content-free active model/tool summaries, timeout/cancellation status, and
pending-result state. It is a latest-report diagnostic, not lease or watchdog
authority.

On timeout the worker fences the attempt, requests escalation, and joins all
owned resources. It records a transient result and releases its local permit
only after direct-child reap, recursive descendant emptiness, and endpoint joins
are proven. A blocked inspection remains in `cleanup_pending`, retaining the
attempt fence and permit while throttled operator diagnostics continue. Result
delivery/replay and durable-claim convergence continue after a proven cleanup
and permit release. A Forge outage therefore appears as `worker.result.delivery`
retry warnings and eventually `assignment.convergence`, rather than
monopolizing capacity. Both graceful and forced termination append a synthetic
canonical terminal activity with `status=cancelled` even if the child cannot
send one.

## Process-containment capabilities and cleanup evidence

On Linux, the preferred backend uses a systemd-delegated cgroup-v2 subtree. A
job cgroup is prepared before agent spawn; nested tool, MCP, worker-command, and
pre-push cgroups are created below it and membership is established before
`exec`. The production contract requires a unified cgroup-v2 mount, writable
delegation and nested-subtree controls, and pidfd support. `cgroup.kill` is used
for hard escalation when available; otherwise Temper repeatedly enumerates and
signals every nested member. Direct-child reap and independent recursive-empty
verification gate completion in both cases.

When delegation or pidfd capability is unavailable, Linux activates its
subreaper/supervisor fallback. The fallback owns re-parented descendants and
tracks them independently of process groups and sessions. Windows uses nested,
kill-on-close Job Objects with assignment-before-resume and empty-job
verification. A host without one of those production backends fails
containment preparation rather than silently degrading to direct-child or
process-group-only cleanup.

The worker emits exactly one `worker.containment.startup_capability` diagnostic
per process. It reports the bounded cgroup-v2 mount identity, delegation,
nested-subtree writability, `cgroup.kill`, pidfd, selected backend, and fallback
reason. The same attempt-bound observer is installed in managed bash and MCP
containment for split-agent and standalone execution, so nested blocked,
fallback, and completed cleanup carries worker/job/attempt plus bounded
owner/tool identity instead of appearing only in the final job cleanup.
Unavailable delegation and `worker.containment.fallback_activated` are
warnings. `worker.containment.cleanup_completed` is debug for an ordinary
already-empty owner and warning when cleanup recovered leaked descendants or
inspection failures. `worker.containment.cleanup_blocked` is warning/error,
throttled by bounded containment root, and includes bounded survivor
PID/PPID/PGID/session/start-time/executable evidence without command arguments,
prompts, output, or credentials.

At startup, stale cgroups are considered owned only below Temper's dedicated
subtree and only when their logical-worker/process-boot fence parses. A fence
contains a non-zero creating PID and kernel start-time identity: an exact live
match is preserved, while a missing process or PID reuse proves that owner
stale. If the process-incarnation fence cannot be established, cgroup-v2 Auto
selection records the capability failure and uses the Linux supervisor
fallback. Only proven-stale members are killed; trees that independently become
empty are removed deepest-first. Legacy, malformed, still-populated, or
uninspectable trees remain without being signaled.
`worker.containment.startup_scavenge` reports removed/live-protected/retained
counts, a bounded list of retained path diagnostics, and the omitted-diagnostic
count; retained or omitted evidence is warning level. During `SIGINT` or
`SIGTERM`, a split worker stops intake, fences all attempts,
escalates every active owner, and joins the active-job registry before
returning. With the example worker unit, `Delegate=yes` permits nested ownership
and `KillMode=control-group` kills the complete service cgroup after its
five-minute external `TimeoutStopSec` backstop. Abrupt `SIGKILL`, kernel
failure, or power loss cannot produce a terminal cleanup event; the next
startup performs stale ownership inspection. Do not use `KillMode=process`:
it can leave the worker's agent, MCP, or managed-command descendants alive.

## Standalone shutdown budget and blocker diagnostics

Standalone keeps the ordinary worker proof rules above while the process is
healthy: ownership loss still waits for recursive-empty/direct-child evidence
and the exact terminal-trace acknowledgement before quiescence, result
recording, heartbeat removal, or permit release. The standalone signal path is
a distinct bounded process-loss contract, not a timeout that weakens those
rules.

The default `deployment.standalone_shutdown_budget_secs = 30` is one absolute
interval from signal receipt across daemon admission fencing, attempt
cancellation/join, already-admitted operations, trace retention, exact joined-
assignment release, and HTTP drain. The checked standalone unit uses
`TimeoutStopSec=45s`, keeping systemd strictly outside Temper's deadline with a
15-second safety margin. The deadline exit is core-dump-free, so core generation
cannot consume that margin. When tuning either value, rerun `temper check` and
keep `TimeoutStopSec` strictly greater than Temper's budget; never configure it
at or below the budget.

Before cancellation, standalone fences new claims, worker results, result
application, assignment-scoped Forge context, and active attempts. A complete
join emits `standalone.shutdown.summary` with `disposition=graceful_exit` and
releases only exact attempts in the proven worker report. At deadline expiry it
emits `disposition=bounded_crash_handoff`, retains unproven durable assignments
and trace spool, drives attempt-owned emergency process termination, and exits
with status 70 through a core-dump-free primitive. It does not unwind, run C/Rust
exit handlers or owner drops, or flush userspace buffers. Restart then uses
startup assignment staging, orphan/feed convergence, durable-result replay, and
trace-spool forwarding. Existing fences and exact durable claim checks reject
late results and Forge operations from the previous process.

`standalone.shutdown.blocker` uses the closed kinds `containment`,
`terminal_trace_ack`, `result_delivery`, `component_task`, and `registry_state`.
Bounded/redacted fields include worker/job/attempt identity, owner scope/name and
root, root PID, sampled survivor PIDs and omitted count, containment phase,
trace run/sequence, first-seen timestamp, monotonically increasing age,
escalation stage, deadline remaining, and occurrence count. The terminal
summary carries the final disposition and a bounded blocker rollup. Unknown
root/PID evidence is not interpreted as recursive-empty proof.

The standalone unit also requires `Delegate=yes` for its preferred cgroup-v2
ownership, pidfd signaling, and recursive-empty checks, and
`KillMode=control-group` as the service-manager backstop. Never use
`KillMode=process`.

## Diagnostics

Completed tick logs include:

```text
trigger=<initial|poll|wake|audit> actions=<n> tick_id=<id> \
scanned_repositories=<n> scanned_repository_paths=<owner/repo,...> \
next_poll_ms=<n> idle_no_action_ticks=<n>
```

Use these counters to verify hint narrowing (`wake` for repo A should list repo
A only for role workers) and to spot unexpected broad scans. Coordinated engine
runs additionally distinguish `wake.phase=start` / `wake.outcome=started` from
`wake.phase=finish` / `wake.outcome=completed`; failed finishes retain their
error and timing fields.

At worker debug level, every mechanical phase that starts emits one terminal
`measurement=mechanical.phase` record. Filter on `mechanical.phase` for
`reconciliation`, `automated_scan`, or `transition_application`, then compare
numeric `duration_ms` and `provider.request_total`. `mechanical.scope` is
`broad` or `targeted`, exact work includes `artifact.ref`, and all records inherit
the enclosing `wake.run_id`. `provider.requests_available=false` means the
backend has no portable request counter; use Forge HTTP `operation` records with
the same wake id instead.

Broad role and mechanical list work emits `measurement=candidate.discovery`.
Use `candidate.logical_bucket_count`, `candidate.unique_count`,
`candidate.provider_request_total`, and `duration_ms` to separate workflow
breadth, candidate volume, pagination/provider traffic, and latency. For the
17-label reference workflow, role and reconciliation discovery each stay at no
more than four one-page buckets; automation adds its two populated open buckets.
Pagination multiplies provider requests per bucket but never by label or role
count.

Broad reconciliation emits `measurement=mechanical.reconciliation` with numeric
`detail_cache.hit_count`, `miss_count`, `forced_refresh_count`,
`invalidation_count`, and `eviction_count`. Cold startup begins with misses; a
second unchanged pass should have hits and no exact/dependency reads. Forced
refreshes occur by 15 minutes even after missed hints. All candidate/cache
measurements inherit `wake.run_id` and never perform Forge I/O themselves.

The local real-Forgejo check is ignored by default and uses the cached fixture:

```sh
cargo test -p temper-testing --test idle_request_budgets \
  local_forgejo_two_pass_idle_broad_benchmark \
  -- --ignored --exact --nocapture
```

It prints cold/warm durations and normalized warm method/path counts, then
requires six one-page candidate lists and no per-artifact exact or
`/dependencies` requests on the warm pass. See the
[Forgejo backend reference](forgejo-backend.md) for bucket and pagination details.

`gate.evaluated` is also debug: repeated lines are repeatable read-side
observations, not merge execution. Actual direct merge execution is paired as
`measurement=mechanical.landing_attempt`: `landing.outcome=started` followed by
a duration-bearing terminal `applied`, `gate_not_satisfied`,
`conflict_routed`, `stale`, or `failed`. If an eligible gate appears without an
attempt, confirm the compiled transition contains `merge_pull_request`; non-merge
automation intentionally emits no attempt.

Mechanical workers also emit `mechanical_reconciliation_summary` with `mode`,
`snapshot_count`, `finding_count`, and applied/advisory action counts. Normal
mechanical ticks emit `mechanical_automation_execution` per automated item and
`mechanical_automation_summary` with candidate, applied, unchanged,
gate-not-satisfied, and error counts.
