# Native Forge state: implementation roadmap

This roadmap promotes three pieces of workflow state from **workflow-owned
labels / metadata** into **native Forge state the workflow observes**. It is a
distinct backlog from `reference-workflow-roadmap.md` because it changes what
state the workflow treats as native Forge-owned state; later phases also extend
the `harness-forge` interface.

## Terminology this backlog sharpens

"Projection" is currently overloaded. This backlog distinguishes:

- **Projected / owned state** — the workflow writes it onto the Forge (lifecycle
  labels, post-merge `landed`/`owner-pending`).
- **Observed / native state** — the Forge owns it; the workflow reads it as a
  gate condition (CI conclusions, review decisions, dependency links, the PR's
  `merged` state).

Each phase below moves a fact from the first category to the second, or — for
merge eligibility — dissolves a stored label into a *derived gate*.

## Sequencing rationale

Ordered smallest-blast-radius first:

1. **Phase A — derive merge eligibility.** Mostly a spec/gate refactor; reuses
   the already-native `merged` state and CI jobs. No new backend resource type.
2. **Phase B — native dependency links.** One new portable link concept on the
   Forge interface, implemented in both reference backends.
3. **Phase C — native reviews.** The largest: a new review model on the Forge
   interface. Done last so A and B de-risk the gate-signal plumbing.

## Conventions for every phase

- Follow `AGENTS.md` session bootstrap and `end-a-development-session.md`.
- Each phase writes its ADR **first** (it extends a primitive or the Forge
  interface), then implements.
- Any Forge-interface change updates `docs/reference/forge-interface.md`, both
  `docs/reference/filesystem-backend.md` and `docs/reference/in-memory-backend.md`,
  and adds backend conformance tests in **both** reference backends (ADR 0008
  observable-contract parity).
- Keep the pure `Planner` pure: gate conditions are evaluated against the
  classified artifact plus a small **runtime-supplied signal**, exactly like
  `plan::DependencyStatus` today. The executor computes signals from fresh Forge
  state and threads them into `plan_transition_with`.
- `reference-delivery.json` is the evolving planning fixture; `five-role-delivery.json`
  is the stable executor/safety fixture. Extend the stable one only when an
  execution capability it exercises actually lands.
- Docs ≤150 lines (split before 350); Rust source/test files ≤600 lines.
- Land green: `cargo fmt --all`, `cargo dev-clippy`, `cargo dev-check`, tests.

## Phases

Status legend: ☐ pending · ☑ done.

- ☑ **A — Derive merge eligibility from gates (ADR 0014).** Stop storing
  `merge-ready` as an owned label. The merge transition is gated on the AND of
  the review/testing/CI gates; merge idempotency relies on the PR's native
  `merged` state (already checked by the executor) plus the post-merge `landed`
  projection as the planner re-run guard. Add a `ci_passed` gate condition fed by
  a runtime CI signal computed from `list_ci_jobs` (native CI conclusions),
  retiring the `ci` state dimension and its `ci-passed`/`ci-failed`/`ci-pending`
  adapter labels. Prompt: `native-forge-state-phase-A-merge-gate.md`.

- ☐ **B — Native dependency links (ADR 0015).** Add a portable
  `depends_on` artifact-link concept to `harness-forge` (multiple links, the
  intersection both Forgejo and GitHub support). Classification reads native
  links; `DependencyStatus` is derived from the Forge instead of runtime-supplied
  metadata. The metadata `dependencies` field becomes a compatibility fallback.
  `parent` and `produced_pr` stay metadata-projected because Forgejo has no
  native parent/child (GitHub sub-issues are a richer, non-portable superset —
  out of scope, noted in the ADR). Prompt:
  `native-forge-state-phase-B-dependencies.md`.

- ☐ **C — Native review model (ADR).** Add a minimal portable review concept to
  `harness-forge`: request reviewers, list reviews, per-reviewer decision
  (`approved`/`changes_requested`/`commented`/`pending`), and a portable
  aggregate. Add `review_approved` / `review_changes_requested` gate conditions
  fed by a runtime review signal. Retire the `review-approved` /
  `review-changes-requested` owned labels and the `approve_review` /
  `request_changes` sibling-transition gate wiring. Document provider-specific
  review rules (CODEOWNERS, dismiss-on-push) as out of the portable contract.
  Prompt: `native-forge-state-phase-C-reviews.md`.

> ADR numbering above is indicative; the implementing agent claims the next free
> number at the time of writing and fixes the cross-references.

## Done definition for the whole backlog

All phases ☑; `merge-ready` and the `review-*` / `ci-*` adapter labels are gone
from the fixtures; merge eligibility, dependency resolution, and review state are
all derived from native Forge state read fresh before each transition; both
reference backends implement the new Forge surface with conformance tests; and
`docs/reference/forge-interface.md` plus both backend docs describe the additions.
