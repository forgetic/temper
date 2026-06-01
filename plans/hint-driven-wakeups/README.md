# Hint-driven wakeups — implementation plan

The reference-delivery demo currently *claims* webhook acceleration, but a long
`POLL_MS` (for example `POLL_MS=120000`) still makes progress feel poll-bound.
This plan turns wakeups into a tested contract, starting with deterministic
backends and ending with the real Forgejo example.

Hand the prompt files to the agent loop **one phase at a time, in order**. Each
phase should land green and update this README's status.

## Goal

A workflow handoff must not wait for the long poll interval when a hint is
available. Polling remains the correctness and liveness backstop, but hints must
wake sleeping workers quickly enough that the example behaves well with a large
`POLL_MS`.

## Non-negotiable design constraints

- Do **not** add webhooks or notifications to the `Forge` trait. ADR 0009 still
  stands: hints are edge-triggered accelerators above the request/response Forge
  API.
- Promote any backend-emitted hint contract as a **separate companion surface**
  if needed (for example portable `ChangeHint`/`ChangeKind` plus an optional
  source/sink), not as authoritative state.
- Every wake path feeds the same existing worker tick: pull fresh Forge state →
  classify → plan → execute/reconcile. Hints may be duplicated, dropped, stale,
  broad, or reordered.
- Keep provider-specific receipt/verification in production/Forgejo code. The
  memory and filesystem phases should prove the generic wake driver before any
  live Forgejo work.
- Long-poll regression tests must use an intentionally large interval and assert
  elapsed time or tick counts that prove a hint, not the poll deadline, caused
  progress.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Memory backend hint source + wakeable driver.**
   `prompts/phase-1-memory-hints.md`

   Make in-process hint delivery work against `MemoryForge` with unit tests. Add
   the backend-agnostic wake driver/scheduler seam needed by later phases and
   prove that a second worker reacts to a mutation without waiting for a long
   poll interval. Done: portable hint types and the sync `ChangeSource`
   companion live in `harness-forge`; `harness-runner` re-exports hints and adds
   `WakeablePollLoop`; `MemoryForge::subscribe_hints()` publishes best-effort
   in-process hints for successful mutations.

2. ☑ **Phase 2 — Filesystem backend cross-process hints.**
   `prompts/phase-2-filesystem-hints.md`

   Extend the same contract to `FilesystemForge` with integration tests using
   distinct handles/process-style isolation. The test should fail if handoff
   latency is governed by the configured poll interval. Done: `FilesystemForge`
   appends best-effort JSON-line `ChangeHint`s to `<root>/hints.log`, exposes a
   tailing `subscribe_hints()` source, publishes only after successful durable
   mutations, and runner integration tests prove distinct handles wake well
   before a large poll deadline while restarted listeners still converge through
   the poll/tick backstop.

3. ☑ **Phase 3 — Production wake path hardening.**
   `prompts/phase-3-production-wake-path.md`

   Connect the generic wake driver semantics to the production Unix-datagram
   worker/trigger path. Add focused tests and logs/metrics that make it obvious
   whether a webhook was accepted, routed, delivered, and consumed. Done:
   production worker waits now distinguish poll deadline, stop, authenticated
   wake, and ignored unauthorized wake; trigger delivery logs structured
   accepted/rejected/no-sockets/sent/failed outcomes while discovering
   `--wake-dir` sockets on each webhook; Unix socket unit tests prove long waits
   are interrupted only by authenticated wakes.

4. ☑ **Phase 4 — Real Forgejo webhook e2e.**
   `prompts/phase-4-forgejo-e2e.md`

   Add a gated real-Forgejo end-to-end regression with `POLL_MS` set very high.
   Done: `harness-testing/tests/forgejo_webhook_wakeup.rs` boots real Forgejo +
   real `forgejo-runner`, registers the production Forgejo webhook trigger,
   launches fake-agent Forgejo workers with authenticated wake sockets and
   `--poll-ms 120000`, waits until they have completed their initial no-work
   tick, then seeds the happy path and requires convergence before the poll
   backstop could fire. Worker logs assert authenticated wakes were consumed;
   timeout diagnostics include worker logs, runner logs, trigger address, and CI
   state.

5. ☐ **Phase 5 — Working example + operator docs.**
   `prompts/phase-5-example-and-docs.md`

   Make `examples/reference-delivery/run.sh` visibly depend on and validate the
   wake path. The example should be runnable with `POLL_MS=120000` and still
   converge promptly; docs should explain how to inspect wake delivery.

## Acceptance criteria for the whole plan

- Deterministic memory and filesystem tests cover hint-triggered handoff latency.
- Gated live Forgejo e2e covers real webhooks, real workers, and a long poll.
- `examples/reference-delivery` has a validated mode or documented smoke path
  showing `POLL_MS=120000` is practical.
- Worker/trigger logs expose enough information to distinguish: no webhook,
  webhook rejected, webhook accepted but no socket, socket delivery failed, wake
  consumed but no work found.
- `cargo fmt --all`, `cargo dev-clippy`, and `cargo dev-check` pass at each
  phase; default tests remain hermetic.

## Relevant starting points

- `docs/adr/0009-triggering-model-webhook-accelerated-poll-backstopped.md`
- `docs/explanation/agentic-workflows.md` (Triggering model)
- `crates/harness-runner/src/trigger.rs`
- `crates/harness-runner/src/driver.rs`
- `crates/harness-forge-memory/`
- `crates/harness-forge-filesystem/`
- `crates/harness-production/src/{trigger,wake,worker}.rs`
- `examples/reference-delivery/run.sh`
