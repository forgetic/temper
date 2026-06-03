# Scalable scans and faster Forgejo e2e — implementation plan

This plan fixes the slow Forgejo ignored tests by fixing the underlying
production scaling issues they exposed. The current runner scan is too broad: a
role tick lists all issues and all pull requests (`state=all` on Forgejo), then
loads dependency/review/CI gate signals for every classified PR before deciding
whether the role has any work. Webhook hints wake workers, but multi-repo workers
still scan the full configured repository set. The Forgejo e2e compounds this by
booting and provisioning a fresh server+runner world for each scenario.

Hand the prompt files to agents **one phase at a time, in order**. Each phase
should land green, update this README's status, and add/adjust tests that would
fail if the old broad behavior returned.

## Goals

- Role ticks fetch the minimum state needed for that role's subscribed queues.
- CI/review/dependency signals are fetched lazily and only when a queue or
  transition gate needs them.
- Closed issues and PRs are fetched only when they still carry labels that make
  them candidates for some workflow queue or recovery path. Historical closed
  unlabelled/uninteresting artifacts must not be part of every tick.
- Webhook wakes narrow the immediate tick to hinted repositories where possible;
  polling remains the correctness backstop.
- A low-frequency audit scan remains available for missed hints/drift, but it
  must still avoid unlabelled closed history.
- The ignored Forgejo tests get faster because they exercise the improved
  production behavior, and the full Forgejo multiprocess suite stops paying
  repeated server+runner setup where isolation can be preserved with fresh repos.

## Non-goals and constraints

- Do not make webhooks authoritative. Hints may be stale, duplicated, dropped, or
  broad. A poll/audit path must still converge from Forge state alone.
- Do not silently skip closed artifacts that are still workflow-active. A closed
  PR with `landed`, `needs-*`, `blocked`, or any other queue label must still be
  seen by the queue that owns it.
- Dependency targets should be read by exact reference when needed; do not keep
  all closed dependency targets in the global scan just because dependency gates
  exist.
- Preserve backend contracts unless a phase deliberately changes the portable
  Forge API. Any public Forge API change must update
  `docs/reference/forge-interface.md` and the backend reference pages.
- Keep default tests hermetic and fast. Live Forgejo tests remain `#[ignore]`.

## Design sketch

The target shape is a staged scan:

1. **Plan interest.** From the compiled workflow, role, tick reason, and hints,
   compute the queues and repositories worth inspecting.
2. **Fetch candidates.** Query open artifacts plus closed artifacts with queue or
   recovery labels. Prefer provider-side labels/state filters. For PR labels on
   Forgejo, use a provider-specific efficient path rather than fetching every
   historical PR.
3. **Classify cheaply.** Classify labels/body/metadata first. Do not read CI,
   reviews, or dependency target state unless a matched queue/transition gate
   needs them.
4. **Fetch signals on demand.** Load only the gate signals required by the
   candidate queue or transition (`ci`, `review`, `dependency` independently).
5. **Audit rarely.** A configurable audit tick scans all configured repos and all
   workflow-labelled active/closed-interest artifacts, but still not unlabelled
   closed history.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Scan interest model and lazy gate signals.**
   `prompts/phase-1-scan-interest-and-lazy-signals.md`

   Added `SignalNeeds`, queue/transition/role signal-need derivation, cheap
   queue matching, lazy scan signal reads, and transition-specific executor
   signal reads. Runner regressions cover no-CI role ticks, CI-gated queues,
   review-only queues, and dependency-gated queues. Notable finding: all-role
   scans still inspect all queues by design in this phase; Phase 2 must narrow
   candidate listing and closed-history reads.

2. ☑ **Phase 2 — Candidate query planning and closed-history pruning.**
   `prompts/phase-2-candidate-query-planning.md`

   Added a runner `CandidateQueryPlan` and `ScanMode`. Normal role scans now use
   role-subscribed queue interest to build state+label issue/PR queries, split by
   artifact target, and deduplicate artifacts returned by overlapping queries.
   Open candidates use `state=open` with an open-all fallback only for queues
   without useful labels. Closed issues and closed/merged PRs are queried only
   with non-empty queue labels; audit scans add workflow-label recovery interest
   while still avoiding unlabelled closed history. Regressions cover closed
   unlabelled history pruning, merged `landed` PR discovery, and open-all
   condition queues.

3. ☐ **Phase 3 — Forgejo scalable backend paths.**
   `prompts/phase-3-forgejo-scalable-backend-paths.md`

   Make the Forgejo backend honor the new query shape efficiently, especially
   labelled PR queries and dependency detail. Avoid the current `state=all` PR
   fetch when a small labelled closed set is requested, and avoid N+1 dependency
   enrichment unless the caller requested/needs dependency detail.

4. ☐ **Phase 4 — Hint-narrowed multi-repo ticks.**
   `prompts/phase-4-hint-narrowed-ticks.md`

   Change production and testing worker wake ticks so known repo-specific hints
   scan only matching repositories for the immediate wake tick. Poll/audit ticks
   still scan the configured set. Mechanical workers get the same narrowing where
   safe.

5. ☐ **Phase 5 — Faster Forgejo e2e topology.**
   `prompts/phase-5-faster-forgejo-e2e-topology.md`

   Rework the ignored Forgejo multiprocess suite so setup cost is not repeated
   unnecessarily. Prefer one server+runner per test binary with fresh repo names
   per scenario, or another design with equivalent isolation. Keep tests serial
   and make logs report scan counts/CI-read counts on timeout.

6. ☐ **Phase 6 — Documentation, knobs, and performance acceptance.**
   `prompts/phase-6-docs-and-performance-acceptance.md`

   Document the new scan contract, audit behavior, production knobs, and e2e
   commands. Add regression/performance assertions that prove closed history and
   CI web-UI reads do not grow with unrelated historical artifacts.

## Whole-plan acceptance criteria

- A role worker with no CI-gated subscribed queues performs zero `list_ci_jobs`
  calls during a no-op tick.
- A repository with many closed unlabelled issues/PRs does not cause a role scan
  to fetch or classify that history.
- Closed workflow-active artifacts are still processed when they carry queue
  labels, including landed/reconciliation paths.
- Dependency-gated work still unblocks by fetching exact dependency targets, not
  by scanning all closed dependency targets.
- Multi-repo webhook wakes for repo A do not scan repo B on the immediate wake
  tick; repo B is still covered by poll/audit ticks.
- Forgejo backend tests prove labelled PR queries do not call `/pulls?state=all`
  as the first step when a narrower labelled query is available.
- The ignored Forgejo multiprocess and webhook tests are materially faster on a
  warmed checkout, with before/after timings recorded in this plan or the final
  phase notes.

## Relevant starting points

- `crates/temper-runner/src/scan.rs`
- `crates/temper-runner/src/worker.rs`
- `crates/temper-runner/src/multi_repo.rs`
- `crates/temper-workflow/src/execute/signals.rs`
- `crates/temper-workflow/src/plan/queue.rs`
- `crates/temper-forge/src/forge.rs`
- `crates/temper-forge-forgejo/src/{issues,pulls,ci,ci_ui}.rs`
- `crates/temper-production/src/worker.rs`
- `crates/temper-testing/src/worker_bin/forgejo.rs`
- `crates/temper-testing/tests/{forgejo_multiprocess,forgejo_webhook_wakeup,forgejo_multi_repo_webhook}.rs`
- `docs/reference/forge-interface.md`
- `docs/reference/forgejo-backend.md`
- `docs/how-to/run-forgejo-multiprocess-e2e.md`
