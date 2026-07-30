# Model-failure recovery

This active scenario is the sole checked-in mapping for feature
`ai/temper#806` and plan `ai/temper#807` on
`agent/pr-for-feature-806`.

## Claim → stimulus → observable → assertion

- **Claim:** An unclassified late streamed failure retries only its failed model
  turn. If immediate and fresh-session recovery later exhaust, Temper defers the
  provider without `needs-human`, preserves work, and resumes exactly once.
- **Stimulus:** The scenario-owned Jig adapter injects one late SSE failure after
  the first engineer tool effect, then four consecutive failures after further
  successful tool turns. Declarative steps observe generation-one deferral and
  issue one authenticated, generation-fenced provider-health wake.
- **Observable:** Structured Forge state, Temper recovery/tool/lease events,
  durable session evidence, Jig request identities and counts, retained
  workspace artifacts, branch inventory, CI, and stimulus outcomes.
- **Assertion:** Retry and recovery budgets are exact; tracked and untracked
  edits are not replayed or lost; publication stays blocked while deferred; no
  human parking occurs; and one diff, submit, PR, CI, merge, and marker-clear
  path completes.
- **Runtime budget:** 900 seconds.

## Topology and deterministic request schedule

The bundle starts disposable real Forgejo, registers the real host
`forgejo-runner`, launches the standalone Temper binary supplied by the
exact-head validator, and serves model traffic from its local Jig script. A
15-second declarative harness poll cadence bounds normal fresh-session
rediscovery after claim release; no scenario script drives recovery.

The engineer receives ten requests:

1. write a tracked marker;
2. fail late without status/code;
3. retry the same turn and write a new untracked product file;
4. probe both preserved edits;
5–8. exhaust one immediate retry in each of two sessions;
9. submit the preserved diff after the authenticated wake;
10. return the successful PR handoff.

Together with the architect's two requests, the scenario has a hard total of
12 uniquely identified provider requests. The four-request exhaustion burst is
fully consumed before deferral, so the declared health wake makes the next
request healthy without mutating Jig state.

Runtime evidence belongs only in the validator's caller-provided artifact
directory and must not be committed to this scenario corpus.
