# Reference workflow: confirmed gap backlog

This page records the gaps between the reference delivery design
(`reference-workflow.md`) and what the current `harness-workflow` spec types,
compiler, and planner can express and run. It was produced by transcribing the
design into `crates/harness-workflow/fixtures/reference-delivery.json` and
running it through validate / compile / plan
(`crates/harness-workflow/tests/reference_delivery.rs`).

The label-state-machine core — roles, type/state labels, exclusive and
non-exclusive dimensions, queues, role-authorized label transitions, and
transition-satisfied gates — **validates, compiles, and plans today**. The gaps
below are everything the design needs beyond that core.

## What the fixture had to work around

Two design decisions could not be transcribed faithfully and were encoded as
workarounds, which are themselves gaps:

- **CI as a gate.** `ci_gate` is modeled with two zero-role "adapter"
  transitions (`record_ci_pass`/`record_ci_failure`) so the gate has a
  `satisfied_by`. They validate and let the three-gate merge plan, but no role
  can perform them and nothing distinguishes an external/adapter transition from
  a misconfigured zero-role one.
- **Work-return and flag queues split by hand.** The design's single
  `pr_changes_requested` queue (changes-requested **or** testing-failed) became
  two queues; `escalated`/`needs-human` were bound to `implementation_pr` only.
  See P2 items below.

## Prioritized backlog

### P0 — the delivery loop cannot complete without these

1. **Non-label effects (execution).** The spec can now express assignee,
   comment, pull-request create, and pull-request merge effects, and the
   planner emits them in order. `Executor::execute` still rejects non-label
   effects with `UnsupportedEffect`; claim-time lease effects are also not yet
   emitted. Without merge execution the loop never closes.
2. **Pull-request idempotent create.** `open_pr` needs the
   `Executor::ensure_issue` correlation-key pattern for PRs so a retry never
   double-creates. Tied to the `CreatePullRequest` effect above.

### P1 — modeled behavior is unsafe or under-gated without these

3. **External-signal gates.** A gate class satisfied by a Forge condition
   (`ci = passed`), not only by a sibling transition's labels. Removes the
   zero-role adapter-transition workaround and lets CI be a real gate.
4. **First-class relations + `dependency_gate`.** Relations exist only in
   metadata (`parents`/`dependencies`), not as a spec primitive. The code→code
   dependency could not be expressed, so `dependency_gate` was omitted and
   `mark_code_ready` is a bare architect label flip with no prerequisite check.
   Needs the `relation` primitive, a relation-driven `dependency_gate`, and a
   reconcile action for mechanical unblocking.

### P2 — fidelity/efficiency; expressible workarounds exist

5. **Queue activation policy (`min_depth`/`max_age`).** `RawQueue` has no
   activation fields, so `owner_alignment` is a plain per-item queue instead of
   a batched cohort. A read-side spec field plus a pure planner predicate.
   *Workaround in fixture:* plain queue.
6. **Multi-artifact-kind queues.** A queue matches exactly one `artifact_kind`,
   but `escalated`/`needs-human` can sit on issues *or* PRs. *Workaround in
   fixture:* bound `escalations`/`needs_human` to `implementation_pr` only;
   issue-level routing would need duplicate per-kind queues. **Not in the
   design's anticipated list.**
7. **Disjunctive (OR) queue label-sets.** Queue matching is AND-only, but the
   single conceptual "return to engineer" queue is changes-requested **or**
   testing-failed. *Workaround in fixture:* two queues
   (`pr_changes_requested`, `pr_testing_failed`), engineer subscribed to both.
   **Not in the design's anticipated list.**

## Corrections to the design's anticipated list

The design's "Anticipated harness additions" listed items 1–5 above. All five
are confirmed. Two further gaps were hit that the list did not mention: items 6
(multi-artifact-kind queues) and 7 (disjunctive queue label-sets). Both extend
the queue primitive.

## Decisions to flag (no ADR written yet)

Several backlog items extend the spec primitives and may each warrant an ADR.
These are surfaced for a human decision, not decided here:

- **External-signal gate (item 3)** — new gate class; extends the gate
  primitive.
- **Queue activation policy (item 5)** — new queue fields; extends the queue
  primitive.
- **Queue matching extensions (items 6–7)** — multi-kind and disjunctive
  matching also extend the queue primitive; open question whether they fold
  into the activation-policy ADR or stand alone.
