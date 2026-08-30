# Mapped live post-source exact read

This checked-in scenario is the dedicated mapping for feature `ai/temper#1151`
and plan `ai/temper#1152` on `agent/pr-for-feature-1151`. It evolves the
scenario in place while retaining `introduced_by = "#1144"` as the provenance
of the original #1139/#1140 mapping. The historical
`mapped-live-focused-test-source-relevance` mapping for #1130 remains unchanged.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, Jig
engineer, and deterministic current-root provider perform one minimal repair.
After targeted root discovery, the engineer issues one same-turn read-only batch
containing `repo/src/route.rs`, its caller, and its focused test. The route read
is intentionally too early to create source-selection or mutation authority.

Later turns consume complete V1-correlated and V1-lineaged implementation,
caller, and focused-test sources. Two same-root generic competitors in one
parallel batch, one bounded non-progress competitor, one forest traversal, and
one semantic fallback preserve the mapped topology while the fixture produces
10/10 successful graph results and nine relevant results. A mismatched typed
confirmation remains local and fail-closed.

After the typed source chain completes, the engineer attempts the exact route
patch directly. Temper denies it locally with the closed `policy_denial` /
`policy_precondition` outcome and fixed actionable guidance to perform the
ordinary exact read. The denial does not execute or change the workspace. One
successful ordinary read of `repo/src/route.rs` then confirms that the route is
unchanged and authorizes the matching one-file patch. Focused host validation,
the submission gate, real Actions success, merge, and source closure complete
the run.

The mapped analyzer must retain exactly one `selection` / `read` row for the
declared `repo/src/route.rs` target. The early read and denied patch receive no
selection or mutation authority, and the eventual successful patch cannot
replace the later exact-read row.

## Corrected candidate context

The validation record retains the smoke that exposed this ordering gap only as
non-reusable historical context:

- held source: `6c27457897c0a08a255b427ccc797781060cc1f7`;
- agent SHA-256: `422d6748d8fb4683cea6a9afab5980f438ea8bcde1f852ec8a8c6dcdb38fdaab`;
- provider/model: `openai-codex` / `gpt-5.6-sol`;
- host correctness: 3/3;
- graph results: 9/9 successful and 8/9 relevant, with complete typed
  correlation and lineage.

That smoke stopped before matrix repetitions and must not be reused for the
corrected candidate.

## Privacy boundary

Checked-in declarations and retained aggregate evidence contain only safe tool
counts, closed checkpoint and policy categories, correlation and lineage
completeness, source-evidence kind, ordering, the declared selection target,
consumption mode, current-root binding facts, and gate outcomes. Runtime
prompts, provider selectors and output, source content, roots, credentials,
mutation arguments, host output, and generated traces remain ephemeral.
Generated runtime evidence must not be committed.

## Validation

From the exact assembled #1151 feature head, run:

```sh
cargo test -p temper-agent-core decision_anchor_exact_read
cargo test -p temper-agent --test jig_codebase_memory_agent
cargo test -p temper-benchmark-cli --test interleaved_selection_live_shape
cargo test -p temper-testing exact_source_selection
cargo dev-benchmark-harness
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-exact-source-selection
./.temper/pre-pr
cargo dev-scenario-validate-feature \
  --feature ai/temper#1151 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1151 \
  --pr <aggregate-pr> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

Keep production landing held. External acceptance must use the exact final
aggregate head and immutable artifacts in this order: one fresh enabled smoke,
five fresh enabled runs, five fresh disabled runs, five fresh
forced-unavailable runs, and one verifier invocation. Do not reuse, reorder, or
selectively rerun any smoke or matrix result. The routing-repair effectiveness
criterion and acceptance thresholds remain frozen.
