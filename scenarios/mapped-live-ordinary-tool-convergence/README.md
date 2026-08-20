# Mapped live ordinary-tool convergence

This active checked-in scenario is the sole mapping for feature
`ai/temper#1041` and plan `ai/temper#1042` on
`agent/pr-for-feature-1041`. It is a new bundle: the historical
`mapped-live-graph-consumption` mapping and controlled routing-repair benchmark
remain unchanged.

## Live contract

Real Forgejo, the host Actions runner, standalone Temper, an Anthropic-shaped
Jig fake LLM, and the temporary codebase-memory provider execute one bounded
repair. The engineer consumes one targeted root and four later-turn current-root
carry-forwards before the generic boundary returns an exact local
`exploration_closed` denial. Exact provider-call counts prove the denied call
never reaches MCP and there is no post-closure graph retry.

After closure, the deterministic ordinary sequence proves:

1. the allowlisted provider-native `Read` form reaches canonical `read`;
2. an ambiguous provider-native write is scrubbed as
   `invalid_tool_invocation` with the closed schema category, reason, and
   correction disposition, and cannot touch the fixture;
3. one provider-native `Bash` call fails non-retryably after recording one
   transient execution counter outside the product tree;
4. the identical call is redirected once with zero-duration circuit evidence,
   without reaching the underlying tool;
5. a changed counter check succeeds, proving exactly one underlying execution;
6. a corrected provider-native write reaches canonical `write` and produces
   only the exact minimal `src/lib.rs` repair; and
7. canonical shell validation and `submit_for_pr` remain available, after which
   Actions passes, the PR merges, and the source issue closes.

The checked-in focused fixture preserves aliased and unaliased retry behavior.
The runtime write is fixed by harness code, and shell validation requires one
changed file, formatting, focused tests, all crate tests, and a clean diff.

## Privacy-safe evidence

Durable evidence contains only safe tool identities, closed category/reason/
disposition fields, aggregate counts, ordering, graph binding/checkpoint facts,
and convergence outcomes. The harness emits a scrubbed MCP aggregate and a
content-free Jig request summary for this profile. Raw arguments, provider
payloads, process-local fingerprints, prompts, credentials, source excerpts,
host-gate output, and diagnostic traces remain ephemeral. Generated runtime
evidence must not be committed to this bundle.

## Validation

From the assembled feature head:

```sh
cargo dev-scenario-check
cargo dev-scenario-run scenarios/mapped-live-ordinary-tool-convergence
./.temper/pre-pr
```

The aggregate landing workflow resolves the same mapping with:

```sh
cargo dev-scenario-validate-feature \
  --feature ai/temper#1041 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1041 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```
