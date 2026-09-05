# Mapped live post-correction authorization

This checked-in scenario is the dedicated mapping for feature `ai/temper#1263`
and plan `ai/temper#1264` on `agent/pr-for-feature-1263`. It updates
`mapped-live-exact-source-selection` in place while retaining
`introduced_by = "#1144"`. Feature `ai/temper#1210`, plan `ai/temper#1211`,
and source branch `agent/pr-for-feature-1210` remain historical audit metadata,
as do the earlier `ai/temper#1151` / `ai/temper#1152` mapping and
`agent/pr-for-feature-1151` source branch.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider reproduce the repetition-001
shape with separate implementation/caller and focused-test roots. The routing
root presents bounded candidates in provider order. A source-only preview and a
later explicit implementation call establish provisional `src/model.rs`
authority. Sibling-root, broad, malformed, and unrelated calls in the immutable
selection batch remain local. One unpresented unavailable exact source reaches
the deterministic provider and is rejected. All five spend no recovery
allowance and receive no evidence credit.

A later provider-derived trace of the provisional model returns the caller and
supports `src/route.rs::worker_slot` as a correction. Provider-derived caller
and separate-root focused-test sources complete the retained forest and close
graph exploration. Broad, unrelated, and selectorless graph activity remains
local after closure without discarding the correction handoff. An ordinary read
of the provisional model target is denied with the closed
`correction_inspection_required` policy reason.

The engineer then inspects both presented correction candidates in one bounded
checkpoint without an evidence purpose. In a later turn it explicitly selects
the inspected route candidate as implementation evidence. That successful
correction atomically replaces model authority and retires the correction
handoff. Reuse of the selected and unchosen references stays local; attempted
old-model and unrelated mutations are denied without workspace effects. Graph
exploration remains closed. Exactly one later ordinary read of
`repo/src/route.rs` authorizes exactly one matching one-file route patch.
Focused host validation, the submission gate, real Actions success, merge, and
issue closure complete the run.

## Privacy boundary

Checked-in declarations and retained aggregate evidence contain only safe tool
counts, checkpoint categories, V1 lineage kinds and correction flags, closed
policy reasons, ordering, the declared exact target, current-root binding facts,
and gate outcomes. Runtime prompts, provider selectors and payloads, source
text, roots, credentials, mutation arguments, host output, local paths, and
generated traces remain ephemeral. Generated runtime evidence must not be
committed.

## Validation

From the exact assembled #1263 feature head, run the focused product and
scenario checks before the live scenario:

```sh
cargo test -p temper-agent two_root_model_correction
cargo test -p temper-agent-core decision_anchor_authority_correction
cargo test -p temper-worker deterministic_policy_terminal_becomes_non_retryable_policy_activity
cargo test -p temper-testing exact_source_selection
cargo dev-benchmark-harness
cargo dev-scenario-check
./.temper/pre-pr
cargo dev-scenario-run scenarios/mapped-live-exact-source-selection
cargo dev-scenario-validate-feature \
  --feature ai/temper#1263 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1263 \
  --pr <aggregate-pr> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation-1263
```

Collect exact-head validation only from the final aggregate feature head. Do
not resume the broader #1210 acceptance matrix and do not address repetition
005.

For external live reproduction, use the unchanged routing benchmark, provider,
model, expected patch, thresholds, and budgets. Produce one wholly fresh
enabled repetition in a new output root and keep all generated evidence
untracked:

```sh
TEMPER_BENCHMARK_LIVE=1 cargo run -p temper-benchmark-cli -- run \
  --benchmark benchmarks/agent-sessions/codebase-memory-routing-repair/benchmark.toml \
  --mode live \
  --condition codebase-memory-enabled \
  --agent-bin target/debug/temper-agent \
  --config /srv/data/git/runner/.config/temper/config.toml \
  --secrets /srv/data/git/runner/.config/temper/credentials.toml \
  --pool engineers \
  --repetitions 1 \
  --output-dir target/live-reproduction-1263-enabled
```

The report may include the exact-head SHA, commands, scenario outcome, the
successful corrected-route transition, and privacy-safe aggregate checkpoint
counts. It must not publish an individual model path, selector, root, prompt,
source payload, mutation argument, host output, credential, or diagnostic
artifact.
