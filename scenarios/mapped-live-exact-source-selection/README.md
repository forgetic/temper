# Mapped live decision-evidence convergence

This checked-in scenario is the dedicated mapping for feature `ai/temper#1210`
and plan `ai/temper#1211` on `agent/pr-for-feature-1210`. It updates the
`mapped-live-exact-source-selection` scenario in place and retains
`introduced_by = "#1144"`. The prior feature `ai/temper#1151`, plan
`ai/temper#1152`, and source branch `agent/pr-for-feature-1151` remain
historical audit metadata; the focused-test and decision-gap scenarios also
remain unchanged.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal repair
only after complete retained decision evidence.
The enabled path first discovers separate routing and focused-test roots in one
immutable batch, then reads `repo/src/route.rs` before complete source evidence.
That early exact read has no retroactive selection or mutation authority. They
begin with every required evidence kind absent. One immutable recovery batch
combines the admitted active-root implementation source with a cross-root
caller read, an irrelevant broad search, a malformed selector, and a failed
duplicate implementation attempt. Those four failed calls remain local, share
the same pre-batch diagnostic, spend no allowance, and earn no evidence credit. The implementation
source alone reaches the provider and leaves `trace`, `caller`, and
`focused_test` while preserving four recovery slots.

A later batch consumes the provider-derived active-root trace while a duplicate
implementation source is denied from the same immutable snapshot. The trace
leaves caller and focused-test evidence missing with allowance three. Separate
later turns consume the provider-derived caller selector on the routing root and
the focused-test selector on the independent test root. Those results complete
the retained root forest. After completion, broad, irrelevant, and selectorless
graph activity is also denied locally. Provider counts prove that cross-root,
irrelevant, malformed, denied, failed, duplicate, and post-completion activity
did not reach MCP or replace the complete typed chain.

The direct route patch is then denied with the closed `policy_denial` /
`policy_precondition` outcome because the only route read happened too early.
The denial has no workspace effect. Exactly one later ordinary read of
`repo/src/route.rs` supplies the required `selection` mode and authorizes the
matching one-file patch. Focused host validation, the submission gate, real
Actions success, merge, and issue closure complete the run.

Trusted provider unavailability still fails once and releases productive
conventional fallback without an immediate retry through the frozen routing
harness. Disabled mode remains the unchanged honest conventional-discovery
control and synthesizes no graph evidence. The mapped enabled scenario does not
special-case the benchmark transcript or weaken either control.

## Privacy boundary

Checked-in declarations and retained aggregate evidence contain only safe tool
counts, typed lineage kinds and stages, closed recovery and policy categories,
ordering, the declared exact selection target, current-root binding facts, and
gate outcomes. Runtime prompts, provider selectors and payloads, source text,
roots, credentials, mutation arguments, host output, local paths, and generated
traces remain ephemeral. Generated runtime evidence must not be committed.

## Validation

From the exact assembled #1210 feature head, run the focused product and
scenario checks before the live run:

```sh
cargo test -p temper-agent-core every_successful_incomplete_kind_matrix
cargo test -p temper-agent-core rejected_failed_cross_root_broad_and_irrelevant
cargo test -p temper-agent --test jig_decision_chain_bypasses
cargo test -p temper-testing exact_source_selection
cargo dev-scenario-check
./.temper/pre-pr
cargo dev-scenario-run scenarios/mapped-live-exact-source-selection
cargo dev-scenario-validate-feature \
  --feature ai/temper#1210 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1210 \
  --pr <aggregate-pr> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

Keep production landing held until exact-head feature validation succeeds.
External acceptance must use the unchanged routing benchmark and verifier in
this order: one wholly fresh enabled smoke, five fresh enabled repetitions,
five fresh disabled repetitions, five fresh forced-unavailable repetitions,
and exactly one verifier invocation. Freeze each root before starting the next
condition. Do not reuse #1203 artifacts, selectively rerun, reorder conditions,
or continue after a failed condition.

The final report must use only privacy-safe aggregate evidence and report the
unchanged `enabled_decision_evidence`, byte-exact patch, host validation,
relevance, improvement, unavailable retry, disabled control, and privacy gates.
It must not publish individual model paths or diagnostic artifacts.
