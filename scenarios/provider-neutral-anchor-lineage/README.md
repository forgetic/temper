# Provider-neutral anchor lineage

This active, checked-in scenario is the sole mapping for `ai/temper#1000` and
plan `ai/temper#1001` on `agent/pr-for-feature-1000`. It adds this mapping
without rewriting the historical `#991` provider-result-anchor mapping.

## Contract

A real Forgejo instance, host Actions runner, standalone Temper process, and
Jig fake engineer run one minimal repair. The temporary provider fixture first
returns a provider-shaped qualified selection. In a later model turn, the
engineer uses its approved transformed typed representation for the dependent
trace, then consumes current-root implementation, caller/model, and focused
behavioral-test evidence before mutation.

The deterministic bundle denies unrelated targets and producer-turn use, as
well as malformed or cross-root lineage and incomplete evidence. It also keeps
bounded recovery exhaustion mutation-free. An unavailable or systemic fallback
creates no anchor and retains conventional discovery without granting mutation
authority. The accepted path runs focused host validation and requires the
implementation PR, Actions CI, and source issue to converge.

The deterministic fixture contract keeps the successful transformed chain
separate from denial regressions: unrelated, producer-turn, malformed,
cross-root, incomplete, recovery-exhausted, and unavailable/systemic paths
cannot become mutation authority. The native state-machine regressions exercise
those denied paths, while this mapped bundle exercises the one real-stack valid
minimal-repair path.

## Privacy-safe evidence

Checked-in declarations and durable aggregate evidence retain only tool
identities, ordering, current-root binding, and correlation/lineage type facts.
Provider or model content, transient selections, targets, digests, source
paths, traces, credentials, and runtime fixture logs stay ephemeral. Assertions
and errors use generic categories rather than reproducing those values.

## Landing use

After the aggregate feature head is assembled, run its exact-head focused
validation from that checkout. Supply the aggregate PR number and retain the
output directory as the focused-validation artifact; do not substitute a
default-branch scenario or evidence from an earlier head.

```sh
cargo dev-scenario-check
cargo dev-scenario-validate-feature \
  --feature ai/temper#1000 \
  --landing-base origin/main \
  --source-branch agent/pr-for-feature-1000 \
  --pr <aggregate-pr-number> \
  --sha "$(git rev-parse HEAD)" \
  --output-dir target/focused-validation
```

The manual live-run alias remains useful for exercising only this scenario:

```sh
cargo dev-scenario-run scenarios/provider-neutral-anchor-lineage
```
