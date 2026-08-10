# Run focused feature validation

Use focused validation for an aggregate feature pull request targeting `main`.
The lane resolves the feature mapping at the checked-out PR head and runs exactly
one active scenario. It does not pick `basic-delivery` or run the whole corpus.

## Mapping contract

The source branch normally uses Temper's derived
`agent/pr-for-feature-<issue>` form. The legacy `feature/<issue>-<slug>` form
remains accepted for existing feature branches. The mapped scenario must have
matching `[validation]` metadata, be active, and be new or deliberately updated
relative to the landing base. Resolution rejects zero or multiple matches,
dirty scenario content, unchanged content, and digest mismatches.

Run the same path locally from the exact feature head:

```sh
cargo dev-scenario-validate-feature \
  --feature ai/temper#806 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-806 \
  --pr <landing-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The command executes the implicit `manifest` topology: real Forgejo, a host
`forgejo-runner`, a standalone Temper binary from the checkout, and Jig fake-LLM
agents. This is the only scenario execution topology.

`cargo dev-scenario-run scenarios/<name>` is the sole manual live-run alias. It
requires the path and never defaults to `basic-delivery`.

## Provider-result anchor example

Feature `ai/temper#991` resolves to the active
`scenarios/provider-result-anchor` mapping on `agent/pr-for-feature-991`.
Resolve and run it from the exact aggregate feature head with the supplied PR
number and `git rev-parse HEAD`; do not substitute a default-branch scenario or
reuse evidence from an earlier feature head. Its live Jig fixture mints opaque
dependent values only after successful provider results, so the agent must
consume a result-derived trace and complete current-root source evidence before
the minimal repair. Durable evidence retains aggregate tool, ordering, binding,
and typed-correlation facts only.

## Result-driven guidance example

Feature `ai/temper#982` resolves to the active
`scenarios/result-driven-decision-guidance` mapping on
`agent/pr-for-feature-982`. Its live fixture mints opaque dependent values, so
later operations must consume successful provider results rather than merely
replay a successful scripted sequence. Run evidence retains aggregate tool,
binding, and typed-correlation facts only.

## CI evidence

`.forgejo/workflows/focused-feature-validation.yml` runs the focused job for
aggregate pull requests targeting `main`. It checks out
`pull_request.head.sha`, fetches the landing base, derives the feature issue
from the canonical or legacy source-branch form, and invokes the same command.
The host lane resolves its disposable Forgejo server and runner binaries through
`$HOME/.cache/bench-forgejo`; it must not depend on a first-use release download
during validation. The post-merge live-validator lane uses the same shared
fixture cache.

The retained artifact is named
`focused-feature-validation-pr-<pr>-<head-sha>` and contains, when available:

- `feature-scenario-mapping.json`, including the mapping id, scenario, exact
  head, landing base, and resolved content digest;
- `run-evidence.json`, including topology, binary, assertion, final-state,
  retained-path facts, the exact read-back dedicated-CI/role-feed/mechanical
  cadences, and any verified ordinary-failure proof provenance;
- `validation-pr-<pr>-<head>.json`, the structured
  `temper.validator.result.v2` payload;
- `focused-validation-audit.json`, joining the mapping and validator result for
  CI audit consumers;
- `checkout-audit.log`, recording the supplied landing PR head and the forced
  checkout head; and
- live logs, script assertion output, and a Markdown report; and
- `focused-validation-failure.txt` and `ci-audit.log` on early failures.

The upload step uses `always()`. A missing mapping, stale head, live execution
failure, missing evidence, unsupported required assertion, or failed required
assertion therefore fails the job without discarding diagnostics. For dedicated
CI polling scenarios, operators should require both
`[expect.effective_configuration]` (all three exact cadence values) and
`[[expect.verified_failure_proof]]` (the selected job and bounded issuer/
verification expectations). Missing configuration, proof, or repository/PR/
head/run/job/attempt coordinates is intentionally inconclusive and blocking;
it must never be treated as a passing absence.

Verified-failure evidence is diagnostic after backend authentication: retained
records contain category, exact subject/execution coordinates, producer/issuer,
verification mode, and validity timestamps, but no credential, signature,
source record, log text, or secret. Preserve `run-evidence.json` rather than
reconstructing this trust decision from rendered logs.

## Stale evidence and authority

The supplied landing PR SHA is authoritative: the workflow force-checks out
that commit, the command checks it before and after the live run, and the audit
records it as `landing_pr_head_sha`. Mapping resolution still independently
proves the feature, source branch, scenario identity, and content digest; it
does not replace the supplied SHA. The result repeats the mapping id, source
branch, head SHA, content digest, standalone binary SHA-256, required assertion
results, and verdict. Any mismatch among the supplied SHA, checkout, resolver,
or passing result fails the lane.

Any new commit changes the feature head. Prior evidence then describes an old
attempt and must be rerun; it cannot authorize landing. Focused CI is a
pre-merge bridge and useful audit signal, but the workflow-native validator gate
is the landing authority. The older `validate-pr` command remains manual report
compatibility tooling, not authority.

## Implicit-live scenarios and lower-level tests

Focused scenarios are isolated real-stack proofs. They supplement rather than
replace the repository's MemoryForge, filesystem-forge, in-process, hermetic
real-stack, and simulation tests. Those lower-level tests remain the cheapest
place for workflow logic, edge cases, and failure interleavings. They are not a
scenario CLI mode and must not be cited as the exact-head scenario evidence
required for feature landing.

Whole-corpus `cargo dev-scenario-check` and the broad ignored live e2e lane stay
separate. The former checks every manifest without execution; the latter keeps
broad regression coverage. Neither selects or replaces the one mapped feature
scenario.
