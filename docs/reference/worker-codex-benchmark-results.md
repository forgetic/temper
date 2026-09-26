# Temper worker versus Codex: measured results

The five-pair campaign on 2026-09-26 met the agreed median coding-time target:
**371.943 seconds for Temper versus 375.701 seconds for Codex**, a ratio of
0.989997 (about 1.0% lower). Both contestants passed all five attempts and all
ten source reviews. Every Temper issue produced a passing PR that merged.

Temper's mean time and tool count were higher, and its timing variability was
much greater. This narrow median result on one task does not establish general
performance superiority, statistical significance, or tool-count parity.

## Every scheduled pair

Times include every coding invocation, retry, and recovery for an attempt.
No failed or slow attempt was dropped, replaced, or restarted.

| Pair | Order | Temper s | Codex s | T/C ratio | Both correct/reviewed |
| --- | --- | ---: | ---: | ---: | --- |
| 1 | T, C | 350.864 | 385.693 | 0.910 | yes |
| 2 | C, T | 800.900 | 375.701 | 2.132 | yes |
| 3 | T, C | 376.371 | 403.205 | 0.933 | yes |
| 4 | C, T | 369.679 | 351.705 | 1.051 | yes |
| 5 | T, C | 371.943 | 374.993 | 0.992 | yes |

| Coding-time statistic | Temper seconds | Codex seconds |
| --- | ---: | ---: |
| Median | 371.943 | 375.701 |
| Mean | 453.951 | 378.260 |
| Minimum | 350.864 | 351.705 |
| Maximum | 800.900 | 403.205 |
| Sample standard deviation | 194.195 | 18.708 |

Pair 2's Temper session included three failed `edit_files` calls and a denied
shell mutation. It made 28 model requests, with 786.737 seconds in model calls
and 23,094 output tokens. Recovery remained inside its 800.900-second result.
The trace does not isolate how much model time each rejected edit caused.

## Workload and timing

The task extends a dependency-free Rust delivery-policy library across five
modules: configurable parsing, typed errors, label normalization, evaluation,
reporting, focused tests, and documentation. Both agents received the same
task and seed. Seventeen host-owned acceptance cases remained outside their
workspaces. Common format/test gates and full source reviews additionally
verified unchanged legacy APIs, seven original tests, dependencies, and gates.

Both requested GPT-6 Astra with `xhigh` reasoning, the same OpenAI account and
default service tier, and no subagents. Codex 0.157.1 ran with
`--dangerously-bypass-approvals-and-sandbox`; its normal codebase-memory-mcp
configuration was enabled and actual graph use was verified. Both used MCP
0.10.8, fresh sessions/checkouts/targets, and unique cold graph namespaces.
Arms ran serially in alternating order after CI/build activity had finished.
The toolchain, compiler cache, and MCP daemon could remain warm.

Temper coding time sums host `agent.finished.duration_ms`, covering invocation
setup, MCP preparation, model/tool work, validation, submission, cleanup, and
trace acknowledgement. Each final attempt needed one in-process session and
no CI repair. Codex time spans CLI process spawn through exit. Host acceptance
runs are excluded from both. Standalone daemon and Forgejo startup are separate
setup costs; issue-to-merge time includes delivery work beyond coding.

| Temper pair | Setup s | Issue to merge s | CI s | Outside coding s |
| --- | ---: | ---: | ---: | ---: |
| 1 | 8.508 | 361.928 | 3 | 11.064 |
| 2 | 8.429 | 809.854 | 3 | 8.954 |
| 3 | 8.519 | 386.565 | 3 | 10.194 |
| 4 | 8.531 | 379.347 | 3 | 9.668 |
| 5 | 8.785 | 381.655 | 4 | 9.712 |

Outside-coding time includes CI. Remaining queueing, polling, branch/PR
publication, and merge/convergence costs are not independently timed; their
individual fields remain unavailable in the data. Observed PR/merge boundaries
are retained without assigning the residual to a particular cause. Delivery
wall time and Codex coding time have different boundaries.

## Tools and memory

Model-selected graph attempts and actual MCP provider calls are separate
counts. Worker readiness and freshness checks also invoke MCP; a denied
wrapper attempt can finish without reaching the provider.

| Pair | Tools T/C | Graph attempts T/C | Actual MCP T/C |
| --- | --- | --- | --- |
| 1 | 35 / 35 | 8 / 21 | 17 / 21 |
| 2 | 44 / 28 | 9 / 13 | 18 / 13 |
| 3 | 39 / 32 | 9 / 20 | 18 / 20 |
| 4 | 38 / 27 | 9 / 14 | 18 / 14 |
| 5 | 38 / 28 | 7 / 15 | 13 / 15 |
| Median | 38 / 28 | 9 / 15 | 18 / 15 |

Temper used 3–4 snippet calls, one inbound trace, and 16–18 direct reads per
attempt, plus a new-file patch, batched editing, explicit Rust formatting,
and submission. Codex used 9–14 snippets, one trace, three patches, and 9–12
shell calls. Shell calls can bundle several reads or checks, so raw counts
are not equivalent units of work. Full raw-name distributions and canonical
categories are in the [sanitized data](worker-codex-benchmark-results.json).

Both indexed exactly once per attempt. Temper made 5–8 repeated identical
provider requests per attempt, largely readiness/freshness checks; Codex made
none. Identical arguments after a mutation or during readiness polling do not
alone establish redundant work. Actual MCP duration ranged from 3.220–5.517
seconds for Temper and 2.477–2.630 seconds for Codex. Durations can overlap
other work and are not an additive wall-time decomposition.

Temper recovered from six mutation/read-precondition denials, three edit
execution failures, and two coverage-call failures. Its provider records also
include five initial missing-project `index_status` errors, one per attempt.
There was no typed systemic graph fallback. Codex's fourteen recorded nonzero
tool exits were expected `git diff --no-index` exits with diff output; all
its MCP calls succeeded. These counts should not be treated as equivalent
implementation failures.

Temper made 16/28/21/20/23 model requests, with the same number of attempts
(no provider retries). Codex public events do not expose internal request
counts, retries, or model durations; those fields remain `null`. Per-attempt
total, cached, uncached, and output tokens are retained. Median total input
tokens were 551,988 for Temper and 318,154 for Codex; median uncached input
was 42,676 and 40,266 respectively. Token totals do not establish billed cost.

## Reproduction and evidence

Follow [the benchmark procedure](../how-to/benchmark-worker-vs-codex.md), using
the frozen identities and versions in the
[provenance record](worker-codex-benchmark-provenance.md). Build before timing,
keep all five pairs, and apply the same correctness and evidence requirements.
The checked-in [JSON](worker-codex-benchmark-results.json) preserves unrounded
metrics, hashes, unavailable fields, and all five native head/merge identities.

The complete feature landed through [integration PR #1337][integration] after
required local validation, exact-head ordinary CI, focused live validation,
and the final campaign. The [parent issue][parent] tracks individual changes.
The deployed Temper instance was never used for coding this work.

This is one Rust workload and five pairs. Provider scheduling, prompt-cache
behavior, and host noise remain uncontrolled influences. Requested/effective
client settings are verified; provider-reported identity and reasoning effort
were unavailable. Earlier diagnostics are retained but cannot establish a
causal before/after speedup. See provenance for exclusions and known historical
scenario failures.

[integration]: https://git.ekanayaka.io/ai/temper/pulls/1337
[parent]: https://git.ekanayaka.io/ai/temper/issues/1285
