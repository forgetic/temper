# Worker benchmark provenance

This records the inputs and evidence behind the
[2026-09-26 measured results](worker-codex-benchmark-results.md).
The [sanitized JSON](worker-codex-benchmark-results.json) contains complete
identifiers, per-attempt measurements, and receipt hashes.

## Frozen inputs

| Input | Identity |
| --- | --- |
| Measured source | `87e190453b25fecd4916c6dbee8ba41accea175c` |
| Full source tree | `28289c33f671982830b4b6a552971a5f9a9da722` |
| Integration merge | `c8d517623777ae75d314be16076bf5962024c0ea` |
| Task revision | `8af00baa943417fe2d6542311e02584f975debba` |
| Seed commit | `e1802a491825650f725b81fa650215cc1f05b6d1` |
| Fixture tree object | `ee67b8284321a40ea7b0cae1297a326087932aeb` |
| Task blob object | `32043d4d89985bb868b128042cfaaf833e2e9900` |
| Acceptance tree object | `da4fb7d6c95d8886d2cf72b2c1069cbc490f9469` |

The integration merge has exactly the measured source tree. The final build
used a fresh dedicated target directory, `--locked`, the dev profile, and
Rust/Cargo 1.85.0. Its receipt connects 2,684 source hashes, 51 archived harness
hashes, four binary hashes, and seven validation/publication receipt hashes.
Build time was 358.938 seconds and is excluded from contestant timing.

The host had eight logical CPUs. Each attempt used a fresh checkout and Cargo
target, with the existing kache wrapper and one Cargo build job. The compiler
cache and account-wide MCP daemon were warm/shared; graph namespaces were
fresh and unindexed. No per-arm `CBM_CACHE_DIR` override was used. Host load
samples and quiet-host process/CI checks are retained; they do not prove
continuous isolation from all external load.

The fixed executables were Codex 0.157.1, codebase-memory-mcp 0.10.8, Forgejo
16.0.1, and forgejo-runner 12.12.0. Both clients requested `gpt-6-astra` with
`xhigh` on the same account and default service tier. Subagents were disabled.
Native model-start events matched the configured model on every request.
Provider identity/effort metadata and Codex internal model counts remain
unavailable.

The effective Codex configuration fingerprint stayed fixed before and after
every arm. Only exact trust entries automatically created for this campaign's
generated checkouts were ignored; other configuration drift invalidates a run.
The normal MCP server was enabled with no tool allow/deny filters. Both arms
used the same provider executable with a transparent invocation recorder.

The campaign ran from **2026-09-26 22:15:06.237892 UTC** through
**23:27:34.092911 UTC**, with five alternating pairs and a fixed 1,800-second
per-arm timeout. There were no timeout failures or missing attempts.

## Delivery and review evidence

Each native attempt booted an isolated real Forgejo, host runner, and
standalone Temper. Each unique repository had issue 1 and PR 2. Retained
Forgejo API records prove successful CI on the actual PR head, merge, issue
closure, and a final
checkout at the merge commit. Complete identities are in the JSON data.

| Pair | PR head prefix | Merge prefix | Acceptance | Source review |
| --- | --- | --- | --- | --- |
| 1 | `1b0ceda1eca7` | `2528414686a6` | 17/17 | passed |
| 2 | `13cf7d24015b` | `531e63fd9d26` | 17/17 | passed |
| 3 | `db48249e2e7d` | `8a3e8fd162bf` | 17/17 | passed |
| 4 | `11fc9008da9f` | `b5cf6314c1f9` | 17/17 | passed |
| 5 | `f852043eb7b6` | `e19e832f6378` | 17/17 | passed |

All five Codex solutions also passed the same 17 acceptance cases and complete
source review. Three delegated reviewers examined all ten full diffs and every
changed/new file, including untracked Codex additions. They checked public
types, parser error precedence, normalization/prefix semantics, duplicate-rule
behavior, severity grouping, reporting, focused tests, examples, and docs.

All seven original test bodies, three legacy function bodies, two legacy
structs, manifests, lockfile, and gates were preserved. No oracle shortcuts were
found. Root independently verified all review hashes, delivered files, retained
gates, trial/configuration records, oracle counts, and merge evidence. Reviews
ran after timing and made no source changes or test reruns. Review-only native
graph indexes and direct-source fallback for stale Codex indexes are documented.

A separate campaign audit passed 278 checks, independently recomputing raw
timings, tool/MCP counts, failure categories, configuration, and frozen source
and binary links. Its receipt hash is included in the public data.

Private evidence root: `/var/tmp/temper-worker-codex-evidence/`.
Campaign: `final-campaign-87e190-001/`. Its `primary-evidence-index.json`
seals 263 selected evidence files and reviewed source files with SHA-256:

```text
d78807bbdab54657d62f2bc8ce563efeb8fd04028dae31fcc7221e4075e1f75b
```

The private index covers campaign/comparison records, ten trial/acceptance/
validation/review receipts, native journals and delivery records, Codex process/
tool records, MCP records, patches, and reviewed file hashes. It excludes live
databases, build outputs, and credentials. This digest binds retained evidence;
it does not make private evidence publicly accessible. The public JSON exposes
metrics and receipt hashes, without prompts, source/tool transcripts, or tokens
used for authentication. Raw evidence remains on the benchmark host.

## Implementation and validation

The changes address generally applicable worker/tool behavior observed during
exploration. There is no controlled ablation measuring each change's speedup.
The [parent issue][parent] records all 26 scoped PRs and their work items:

- Measurement: #1291, #1295, #1297, #1299, #1331.
- Graph discovery/evidence: #1294, #1302, #1307, #1309, #1310.
- New-file and companion-file admission, cohesive patches, batched edits,
  and typed mutation recovery: #1312, #1321, #1329, #1325, #1327, #1330.
- Read-only format checks, explicit Rust formatting, and schema feedback:
  #1328, #1326, #1332.
- Executable integrity, benchmark validation, terminal activity capture:
  #1317, #1324, #1335.
- Structured graph/readiness and CI-proof fixtures: #1320, #1334, #1339, #1340.

[Integration #1337][integration] brought the complete source to main. Before
the campaign, that tree passed `./.temper/pre-pr`, 3,977 Rust tests with 25
intentional skips, Clippy, 302 executable-header checks, 70 Python benchmark
contracts, all 35 scenario manifests, and all four controlled harness variants.
Five live Forgejo/runner/Temper scenarios passed 13/19/15/16/17 assertions;
each produced a merged PR and closed issue. Their receipts retain CI proof,
script hooks, and complete terminal traces.

[Ordinary CI 1884][ordinary] passed on measured head `87e19045` and base
`1376556c`, including the two live capstones and 79 web tests.
[Focused CI 1885][focused] passed all fourteen requirements: one observed
claim and thirteen live assertions. Original artifact 1734 includes exact
head/base mapping, runtime evidence, successful CI, merged PR, and closed issue.
These passing receipts apply to the measured source, not interrupted runs.

## Exclusions and limits

Twenty earlier exploratory attempts remain retained: nineteen terminal records
and one interrupted attempt without a trial record. Historical analyzers
reported nine correct and ten incorrect results; the interrupted attempt has
no correctness conclusion. All twenty are excluded from the final campaign.
Their inventory records available primary receipt hashes and exclusion reasons.

The latest diagnostic timings were 436.136 seconds for Temper and 438.496 for
Codex, with 34/32 tool calls and 17/20 MCP invocations. Concurrent build load
and diagnostic instrumentation make these unsuitable matched final pairs.
They establish neither a baseline speedup nor the acceptance result.

A power outage interrupted a prior build and CI run. Partial evidence remains
retained; fresh required checks passed before the final campaign. Earlier
cancelled or interrupted CI is not counted as passing evidence.

Rotating post-merge runs 1868, 1870, 1877, and 1883 failed in older graph
fixtures. Their retained causal-source comparisons against pre-feature
baseline `b6d4128340c0cd33597a31274c4b697fc14bdbc6` classify them as
pre-existing.
Passing the mapped scenarios does not establish that the entire historical
scenario corpus passes. These failures were retained without unrelated repairs.

[parent]: https://git.ekanayaka.io/ai/temper/issues/1285
[integration]: https://git.ekanayaka.io/ai/temper/pulls/1337
[ordinary]: https://git.ekanayaka.io/ai/temper/actions/runs/1253
[focused]: https://git.ekanayaka.io/ai/temper/actions/runs/1254
