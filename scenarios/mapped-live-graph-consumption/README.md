# Mapped live graph consumption

This active checked-in scenario is the sole mapping for feature `ai/temper#1009`
and plan `ai/temper#1010` on `agent/pr-for-feature-1009`. It adds the new
mapping without changing the historical feature `#991` or `#1000` bundles.

## Live contract

A real Forgejo instance, host Actions runner, standalone Temper process, and Jig
engineer execute one minimal repair. The temporary provider returns the approved
multi-part transcript shape: nested results, paired short and qualified symbols,
caller lists, related-source references, and source metadata. Values are minted
at runtime and never appear in the checked-in Jig.

Across separate model turns the engineer must consume exactly:

1. one targeted `search_graph` root;
2. one transformed `search_code` refinement;
3. one `trace_path` caller/model result;
4. one current-root implementation `get_code_snippet`; and
5. one current-root focused-test `get_code_snippet`.

Every successful call has a complete typed V1 correlation and lineage record.
The first is a root and the next four are carry-forwards bound to that root.
Both source reads precede one expected unavailable descendant. Its trusted,
privacy-safe failure releases bounded conventional discovery; the Jig performs
one fallback read and does not retry a graph tool before the minimal repair,
focused host validation, Actions pass, PR merge, and source-issue closure.

Focused native regressions deny unrelated and producer-turn mutation authority
and retain the complementary negative contract for malformed, ambiguous,
cross-root, truncated, failed, and unavailable results. In particular, an
expected unavailable descendant releases bounded conventional fallback while
an unrelated outage does not bypass an active anchor.

## Privacy-safe evidence

Checked-in declarations and aggregate run evidence retain only safe tool names,
counts, ordering, current-root binding, typed correlation/lineage stage facts,
and closed producer/carry/source checkpoint names. Provider values, model
selectors, arguments, source text, target paths, digests, prompts, credentials,
local logs, and diagnostic traces remain ephemeral. Generated runtime evidence
must not be committed to this scenario bundle.

## Validation

Resolve and run this mapping from the exact assembled feature head:

```sh
cargo dev-scenario-check
cargo dev-scenario-validate-feature \
  --feature ai/temper#1009 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1009 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The manual live alias remains available for isolated scenario execution:

```sh
cargo dev-scenario-run scenarios/mapped-live-graph-consumption
```
