# Run focused feature validation

Use focused validation for an aggregate feature pull request targeting `main`.
The lane resolves the feature mapping at the checked-out PR head and runs exactly
one active scenario. It does not pick `basic-delivery` or run the whole corpus.

## Mapping contract

The source branch must use `feature/<issue>-<slug>`. The mapped scenario must
have matching `[validation]` metadata, be active, and be new or deliberately
updated relative to the landing base. Resolution rejects zero or multiple
matches, dirty scenario content, unchanged content, and digest mismatches.

Run the same path locally from the exact feature head:

```sh
cargo dev-scenario-validate-feature \
  --feature ai/temper#778 \
  --landing-base origin/main \
  --source-branch feature/778-exact-head-validation \
  --pr <landing-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The command always selects the live `manifest` runner. It uses real Forgejo,
real host `forgejo-runner`, a standalone Temper binary from the checkout, and
Jig fake-LLM agents. There is no validation-grade hermetic substitute.

Use `cargo dev-scenario-run scenarios/<name>` only for an explicit manual live
run. It now requires the path and never defaults to `basic-delivery`.

## CI evidence

`.forgejo/workflows/ci.yml` runs the focused job only for `feature/*` pull
requests targeting `main`. It checks out `pull_request.head.sha`, fetches the
landing base, derives the feature issue from the source branch, and invokes the
same command.

The retained artifact is named
`focused-feature-validation-pr-<pr>-<head-sha>` and contains, when available:

- `feature-scenario-mapping.json`, including the mapping id, scenario, exact
  head, landing base, and resolved content digest;
- `run-evidence.json`, including topology, binary, assertion, final-state, and
  retained-path facts;
- `validation-pr-<pr>-<head>.json`, the structured
  `temper.validator.result.v2` payload;
- `focused-validation-audit.json`, joining the mapping and validator result for
  CI audit consumers;
- live logs, script assertion output, and a Markdown report; and
- `focused-validation-failure.txt` and `ci-audit.log` on early failures.

The upload step uses `always()`. A missing mapping, stale head, live execution
failure, missing evidence, unsupported required assertion, or failed required
assertion therefore fails the job without discarding diagnostics.

## Stale evidence and authority

The command requires the supplied PR head to equal checked-out `HEAD`. The
result repeats the mapping id, source branch, head SHA, content digest,
standalone binary SHA-256, required assertion results, and verdict. A mismatch
between the resolver and a passing result fails the lane.

Any new commit changes the feature head. Prior evidence then describes an old
attempt and must be rerun; it cannot authorize landing. Focused CI is a
pre-merge bridge and useful audit signal, but the workflow-native validator gate
is the landing authority. The older `validate-pr` command remains manual report
compatibility tooling, not authority.

## Live scenarios versus hermetic tests

Focused scenarios are isolated live real-stack proofs. They supplement rather
than replace the repository's MemoryForge, filesystem-forge, in-process, and
simulation tests. Those `hermetic` tests remain the cheapest place for workflow
logic, edge cases, and failure interleavings. They must not be cited as the live
exact-head scenario evidence required for feature landing.

Whole-corpus `cargo dev-scenario-check` and the broad ignored live e2e lane stay
separate. The former checks every manifest without execution; the latter keeps
broad regression coverage. Neither selects or replaces the one mapped feature
scenario.
