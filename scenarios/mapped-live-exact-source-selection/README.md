# Mapped live exact source selection

This checked-in scenario is the dedicated mapping for feature `ai/temper#1139`
and plan `ai/temper#1140` on `agent/pr-for-feature-1139`. It extends the
focused-test source-relevance topology while preserving the historical
`mapped-live-focused-test-source-relevance` mapping for #1130.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal repair.
The engineer consumes typed implementation and caller sources, keeps two
same-root generic competitors in one immutable parallel batch followed by one
bounded non-progress competitor, completes the focused-test forest traversal
and semantic fallback, and consumes the exact typed focused-test source. This
produces 10/10 successful graph calls while the mapped reduction contract keeps
nine relevant results.

Only after implementation, caller, and focused-test evidence complete does one
wider conventional batch start. A successful wrong-target read is the malformed
selection variant. It is interleaved with file discovery, generic search, and
the successful exact route read. The mapped analyzer must retain exactly one
`selection` / `read` row for `repo/src/route.rs`; the wrong target and earlier
generic or forest evidence cannot replace or duplicate it.

The ephemeral provider validator checks exact inventory, stable binding, source
purpose, and checkpoint order. Manifest assertions require the wrong-target and
exact reads in the same post-evidence batch before one exact patch, host
submission, Actions, merge, and source closure.

## Privacy boundary

Checked-in declarations and retained aggregate evidence contain only safe tool
counts, closed checkpoint categories, correlation and lineage completeness,
decision kind, ordering, declared target, consumption mode, current-root binding
facts, local-denial category, and gate outcomes. Runtime prompts, provider
selectors and output, source content, roots, credentials, arguments, host
output, and generated traces remain ephemeral. Generated runtime evidence must
not be committed.

## Validation

From the exact assembled #1139 feature head, run:

```sh
cargo test -p temper-benchmark-cli --test interleaved_selection_live_shape
cargo test -p temper-testing exact_source_selection
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-exact-source-selection
cargo dev-benchmark-harness
./.temper/pre-pr
cargo dev-scenario-validate-feature \
  --feature ai/temper#1139 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1139 \
  --pr <scenario-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

After mapped exact-head validation, require a wholly fresh enabled smoke with
terminal success, 10/10 graph calls, nine relevant results, complete typed
correlation and lineage, all three source-evidence kinds, and exactly one
privacy-safe `selection` / `read` row for `repo/src/route.rs`. The routing-repair
benchmark fixture, manifest, provider, task, expected patch, verifier, privacy
policy, thresholds, evidence requirements, smoke/matrix protocol, and production
configuration remain frozen. Keep PR #1138 held and do not begin the 5×3 matrix
until that smoke passes.
