# ADR 0009: Webhook-accelerated, poll-backstopped triggering off the Forge trait

## Status

Accepted

## Context

The workflow runtime is pull-based by construction: queues are queries over
Forge state, and the executor re-loads fresh state before every transition (see
`docs/explanation/agentic-workflows.md` and `crates/harness-workflow/src/execute.rs`).
Nothing in the runtime trusts a value it was handed; it always re-reads.

What the codebase does *not* specify is the *cadence* of that querying. The
`harness-forge` trait is a request/response query+mutation contract; it has no
notion of change notification. The "agent runner" layer that would decide when
to scan a queue does not exist yet.

The intended real-world Forgejo deployment wants low latency between one agent
delivering work and the next agent reacting to it. Forgejo webhooks can deliver
that. The open question this ADR settles: should webhook triggering (and the
periodic-poll fallback) live in the `harness-forge` interface, or outside it as
an implementation detail?

## Decision

Keep triggering — webhooks and the poll fallback — **outside the `harness-forge`
trait**. The trait stays a clean query/mutation contract.

Adopt a **level-triggered / edge-triggered** model (the Kubernetes-controller
pattern):

- **Polling is level-triggered**: authoritative, lossless, eventually
  consistent. It is the **liveness backstop**, not an optional extra.
- **Webhooks are edge-triggered**: low-latency *hints* that something may have
  changed. They are lossy — deliveries can be dropped, duplicated, delayed, or
  reordered — so they are a **latency optimization**, never a source of truth.

Both trigger sources feed the *same* reaction path: pull fresh state → classify
→ plan → execute → reconcile, which already lives in `harness-workflow`. The
trigger source is pluggable; the reaction is the one real thing.

### Layering

```
Forgejo HTTP POST ─► forgejo webhook adapter   (provider-specific: HTTP, verify HMAC, parse)
                       │  emits a normalized ChangeHint
                       ▼
                    trigger scheduler / runner ◄── periodic resync timer (backstop)
                       │  coalesce + debounce bursts, decide which queue(s) to scan
                       ▼
                    harness-workflow Executor / Reconciler
                       │
                       ▼
                    Forge query + mutation methods   (existing trait, unchanged)
```

Three separable concerns, only one of which is even a candidate for a portable
type:

1. **Receipt / verify / parse** (HTTP server, HMAC, payload schema): entirely
   Forgejo-specific; lives in the Forgejo backend crate or a dedicated webhook
   adapter. Never in core.
2. **Normalizing a payload into a small `ChangeHint`**
   (`{ repo, artifact kind, item number, change kind }`): may become a portable
   type so the scheduler stays backend-agnostic, but it does **not** belong on
   the `Forge` trait.
3. **The coalescing trigger loop**: backend-agnostic, but depends on queues and
   the executor, so it sits in the workflow/runner layer above `harness-forge`.

### Portable push, if ever needed

Do not add a notification method to `Forge`. If backend-agnostic push is wanted,
model it as a **separate optional companion trait** (e.g. `ChangeSource` /
`WatchableForge`) that yields a stream of `ChangeHint`s, implemented only by
backends that can. The filesystem (inotify) and memory (in-process broadcast
channel) backends *could* implement it too, which would let the trigger loop be
tested deterministically without a real Forgejo. Defer building it until the
runner exists and the need is concrete.

## Consequences

- `harness-forge` stays request/response and backend-agnostic; the filesystem
  and memory reference backends are not forced to fake a webhook contract.
- Webhooks are safe to bolt on **because of** the existing design: the executor
  re-loads fresh state and trusts nothing, so a duplicated/stale/forged hint can
  at worst trigger a redundant scan that finds nothing to do.
- The periodic poll is mandatory, not optional: it is the correctness/liveness
  guarantee. Webhooks only lower latency.
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
