# Forgejo exact-head dedicated CI-poll repair

This active provisional live scenario is the focused mapping for feature
`ai/temper#850`, plan `ai/temper#851`, on
`agent/pr-for-feature-850`. It is intentionally separate from the historical
#152/#621 mapping and from `forgejo-v16-api-ci`, which remains the #766
status-only-`Unknown` regression.

## Causal contract

The scenario boots disposable Forgejo v16, the host `forgejo-runner`, standalone
Temper, and scenario-owned Jig agents. The effective standalone configuration
uses a one-second dedicated CI poll while broad role-feed and mechanical
backstops are both 600 seconds.

The engineer's first commit contains an invalid shell entrypoint. Because the
offline host runner does not install marketplace actions, the protected real
Actions workflow is loaded from the protected target branch and reads that source
file at the exact PR head through Forgejo's token-authenticated contents API,
then runs ordinary `sh -n` against it. Only
after that command fails, the workflow reads its authoritative Forgejo run/job
coordinates, serializes and HMAC-signs a bounded `source` failure statement,
and publishes it to the generic loopback evidence service. The job then remains
red. Temper correlates that proof to the exact PR head and emits `ci.completed`
from `ci_poll`, selecting `pr_ci_failed` for the engineer.

The repair agent briefly holds the failed head so the live harness can retain
two independent exact-head observations, then updates the same file on the same
PR branch, creating a different head. Its ordinary check passes, so no new
failure statement is published. The dedicated monitor remains active while
exact-head green evidence is observed and the PR lands before either
600-second broad backstop can establish causality.

Checked-in fixtures contain no credentials or runtime evidence. The live harness
mints per-run endpoint, publication, and integrity secrets, installs them as
repository Actions secrets, and keeps signatures and secret values out of run
evidence. Retained evidence includes:

- one proof publication and redacted GET/POST request provenance;
- initial and repaired heads with observation timing and stable run/job/attempt
  identities;
- proof category, producer, issuer, verifier, and exact-head coordinates;
- the three read-back effective cadences and structured CI-poll event sequence;
- one PR publication, no stale failure on the repaired head, no `needs-human`,
  and final merged-PR/closed-issue state.

## Running

From the repository root:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/forgejo-exact-head-ci-repair
cargo dev-scenario-run scenarios/forgejo-v16-api-ci
cargo dev-scenario-validate-feature \
  --feature ai/temper#850 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-850 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation-850
./.temper/pre-pr
```

Runtime logs and generated validation evidence belong only in the caller-owned
artifact directory.
