# ADR 0009: Webhook-accelerated, poll-backstopped triggering off the Forge trait

## Status

Accepted

## Context

The workflow runtime is pull-based by construction: queues are queries over
Forge state, and the executor re-loads fresh state before every transition (see
`docs/explanation/agentic-workflows.md` and `crates/temper-workflow/src/execute.rs`).
Nothing in the runtime trusts a value it was handed; it always re-reads.

What the codebase does *not* specify is the *cadence* of that querying. The
`temper-forge` trait is a request/response query+mutation contract; it has no
notion of change notification. The "agent runner" layer that would decide when
to scan a queue does not exist yet.

The intended real-world Forgejo deployment wants low latency between one agent
delivering work and the next agent reacting to it. Forgejo webhooks can deliver
that. The open question this ADR settles: should webhook triggering (and the
periodic-poll fallback) live in the `temper-forge` interface, or outside it as
an implementation detail?

## Decision

Keep triggering — webhooks and the poll fallback — **outside the `temper-forge`
trait**. The trait stays a clean query/mutation contract.

Adopt a **level-triggered / edge-triggered** model (the Kubernetes-controller
pattern):

- **Polling is level-triggered**: authoritative, lossless, eventually
  consistent. It is the **liveness backstop**, not an optional extra.
- **Webhooks are edge-triggered**: low-latency *hints* that something may have
  changed. They are lossy — deliveries can be dropped, duplicated, delayed, or
  reordered — so they are a **latency optimization**, never a source of truth.

Both trigger sources feed the *same* reaction path: pull fresh state → classify
→ plan → execute → reconcile, which already lives in `temper-workflow`. The
trigger source is pluggable; the reaction is the one real thing.

Operationally, the supported Forgejo webhook runtime is the engine HTTP surface:
Forgejo posts HMAC-signed webhooks to `POST /forgejo/webhook` on `temper serve
engine` or `temper serve standalone` when `[engine] webhook_secret` or
`webhook_secret_file` is configured. There is intentionally no separate
`temper serve trigger` process. `trigger` remains the logging/service plane name
for inbound facts and wake hints, not a runnable `serve` component.

### Layering

```
Forgejo HTTP POST ─► engine/standalone HTTP adapter (`POST /forgejo/webhook`)
                       (provider-specific: verify HMAC, parse)
                       │  emits a normalized ChangeHint
                       ▼
                    trigger scheduler / runner ◄── periodic resync timer (backstop)
                       │  coalesce + debounce bursts, decide which queue(s) to scan
                       ▼
                    temper-workflow Executor / Reconciler
                       │
                       ▼
                    Forge query + mutation methods   (existing trait, unchanged)
```

Three separable concerns, only one of which is even a candidate for a portable
type:

1. **Receipt / verify / parse** (HTTP server, HMAC, payload schema): entirely
   Forgejo-specific; the supported operator path is the engine/standalone HTTP
   route at `POST /forgejo/webhook`. The `temper-trigger-forgejo` crate remains
   legacy/internal adapter code for older wake-socket fixtures and is not a
   `temper serve` process.
2. **Normalizing a payload into a small `ChangeHint`**
   (`{ repo, artifact kind, item number, change kind }`): may become a portable
   type so the scheduler stays backend-agnostic, but it does **not** belong on
   the `Forge` trait.
3. **The coalescing trigger loop**: backend-agnostic, but depends on queues and
   the executor, so it sits in the workflow/runner layer above `temper-forge`.

### Bounded coordinator ownership and wake scope

The daemon machine is the single owner of volatile wake state. It keys one
bounded lane set by configured repository and role/mechanical lane, retains at
most 32 targeted artifact addresses per lane, admits at most the configured
global number of repository runs, and permits only one dirty follow-up while a
repository is in flight. The executor may run only `WakeWork` admitted by that
machine; webhook handlers, startup recovery, polls, and mechanical cadence do
not bypass it. Apply-window deferral is owned by the same machine, so no new
scan starts while any result apply is active and the final apply completion
promotes at most one generation per affected repository.

An unambiguous issue or pull-request address takes the targeted path. Review
and PR-scoped CI hints retain pull-request identity. Pushes, repository-scoped
or unknown/ambiguous payloads, explicit recovery, polls, startup, and targeted
capacity overflow promote to broad discovery. Broad role work shares candidate
discovery across configured roles; targeted results reconcile only the named
artifact and never prune unrelated pending jobs. Every path rechecks the
`metadata.staged` dispatch guard.

Pending, dirty, and apply-deferred hints are intentionally not persisted. A
restart can lose them without losing correctness because startup schedules a
broad generation for every configured repository and mandatory periodic polls
continue to do the same level-triggered discovery. **Webhook receipt is never
required for correctness**: no queue transition, recovery, or dispatch safety
property may depend on a delivery arriving.

Mechanical scope compaction is deliberately asymmetric. A role-lane broad scan
subsumes exact role targets, but a mechanical broad scope retains exact artifact
addresses alongside its broad marker. Execution serializes those retained
addresses first (PR CI targets, then other PR targets, then issues), performs any
landing mutation before broad reconciliation, and starts role scans only after
mechanical work finishes. Thus a repository poll or ambiguous issue hint cannot
hide a later exact CI/PR reaction even when both are admitted to one mixed
follow-up generation.

The exact-target set is bounded at 32 addresses per lane. Crossing that limit
promotes the scope to `target_overflow` broad discovery while retaining the 32
highest-priority exact mechanical addresses; additional low-priority addresses
are represented by the broad scan rather than an unbounded delivery queue. The
ordering and eviction tie-break on artifact kind and item number, so compaction
is deterministic.

The former `MechanicalTrigger::run`, `run_hinted`, and
`spawn_mechanical_backstop` compatibility path has been removed. It owned a
second lossy boolean admission guard and could bypass the daemon coordinator.
`MechanicalTrigger` now executes only already-admitted work, and production,
startup, webhook, and cadence callers all enter through the same coordinator
and mutation serialization path.

### Portable push, if ever needed

Do not add a notification method to `Forge`. If backend-agnostic push is wanted,
model it as a **separate optional companion trait** (e.g. `ChangeSource` /
`WatchableForge`) that yields a stream of `ChangeHint`s, implemented only by
backends that can. The filesystem (inotify) and memory (in-process broadcast
channel) backends *could* implement it too, which would let the trigger loop be
tested deterministically without a real Forgejo. Defer building it until the
runner exists and the need is concrete.

## Consequences

- `temper-forge` stays request/response and backend-agnostic; the filesystem
  and memory reference backends are not forced to fake a webhook contract.
- Webhooks are safe to bolt on **because of** the existing design: the executor
  re-loads fresh state and trusts nothing, so a duplicated/stale/forged hint can
  at worst trigger a redundant scan that finds nothing to do.
- The periodic poll is mandatory, not optional: it is the correctness/liveness
  guarantee. Webhooks only lower latency.
- Operators run webhook intake through `temper serve engine` or `temper serve
  standalone`; a separate `temper serve trigger` component would add topology
  ambiguity without changing correctness.
- Treat a payload as a **signal**, not data: use it to decide *that* (and at
  most *which* queue) to scan; pull the actual state fresh. Keeps a single
  source of truth.
- Faster reaction widens the concurrency window, so it raises the priority of
  the documented non-compare-and-swap lease gap
  (`docs/reference/robustness-guarantees.md`). Webhooks do not create that bug,
  but make it likelier to bite.

## Follow-up work

- Implement the `ChangeHint` abstraction (concern 2 above) when the runner layer
  is built; consider the optional `ChangeSource` companion trait alongside it.
- ~~Solve the compare-and-swap / conditional-update lease problem so faster
  triggering cannot produce lost-update lease races.~~ Done in ADR 0013: the
  portable optimistic-concurrency `Version`/`expected_version` primitive makes
  lease acquisition a compare-and-swap, so a wider trigger window can no longer
  produce a lost-update lease race.
