# History-independent terminal recovery

This active scenario is the checked-in validation mapping for feature
`ai/temper#863` and plan `ai/temper#864` on
`agent/pr-for-feature-863`.

## Claim → stimulus → observable → assertion

- **Claim:** Periodic role and mechanical reconciliation cost is independent of
  accumulated closed workflow history without making webhook delivery
  authoritative or losing old recovery targets.
- **Stimulus:** With standalone Temper deliberately offline, the harness creates
  one old merged PR carrying a real expired `Lease`, then creates 220 newer
  closed issues, 120 newer closed PRs, and 220 same-owner sibling-repository
  rows. It starts a cold poll sweep, observes two warm role and mechanical cache
  reuses, restarts standalone Temper, and repeats the cold/warm cycle.
- **Observable:** Retained evidence includes the typed history seed and webhook
  omission; structured `candidate.discovery` and `mechanical.phase`
  measurements from both process generations; continuation, completion, cache,
  provider-request, hydration, and exact-read counts; stimuli; and terminal
  Forge state.
- **Assertion:** The old target predates every inert PR, is retained and
  hydrated, and has its abandoned lease recovered. Inert history is never
  hydrated. Both cold generations become authoritative, warm role and
  mechanical consumers reuse the cache, and same-owner sibling rows cannot
  starve the target repository.
- **Runtime budget:** 900 seconds.

## Fixed budgets

The feature documents at most 64 list requests plus 100 retained ambiguous-PR
reads per terminal bucket. This workflow has two terminal buckets and a small,
fixed open-query allowance, so the scenario-owned ceiling is **340 provider
requests per `candidate.discovery` pass**. It also requires:

- no more than 2,002 decoded rows (the documented 1,001-row bound per terminal
  bucket);
- at most one exact detail read and one hydrated artifact per pass—the old
  actionable PR—in both candidate discovery and bounded mechanical
  reconciliation;
- at least one continuation/overflow observation followed by authoritative
  completion;
- numeric reconciliation detail-cache counts and provider-request deltas from
  every mechanical phase;
- at least two cold starts and four warm cache-reuse observations for each of
  role and mechanical consumers.

The assertion script reads only retained structured run evidence. It does not
orchestrate the scenario or call Forgejo. Generated logs, request captures,
cache state, and run evidence belong in the caller-provided artifact directory
and must not be committed here.
