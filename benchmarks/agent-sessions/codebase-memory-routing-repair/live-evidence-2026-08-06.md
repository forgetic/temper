# Live codebase-memory effectiveness evidence: 2026-08-06

Status: **not ready for deployment**.

This report records the first production-provider trial set for the controlled
`codebase-memory-routing-repair` benchmark. It intentionally makes no readiness
claim: the exact-patch, decision-relevance, and material-improvement gates were
not all met. A new trial set is required after the findings below are addressed.

## Frozen protocol and gates

The protocol and thresholds were frozen in `README.md` and `benchmark.toml`
before these trials began:

- five fresh repetitions of each enabled, disabled, and forced-unavailable
  condition;
- the same fixture, task, model, provider, host, capture policy, and repetition
  policy for every condition;
- a warm stable codebase-memory project for the included enabled repetitions;
- successful agent completion, all host commands, and the byte-exact expected
  patch in every included trial;
- at least 20% lower enabled median conventional discovery calls before decisive
  selection (or the predeclared latency/token alternative) than disabled;
- at least 50% decision relevance among successful graph calls with complete
  relevance evidence; and
- a stable categorized unavailable failure, conventional fallback, and no
  immediate graph retry after a systemic failure.

The live command used the checked-in manifest, `--repetitions 5`, the normal
Temper configuration and credential sources, and only changed the runner-owned
condition. Credentials were never copied into the benchmark or its report.

## Non-secret environment metadata

| Field | Value |
| --- | --- |
| Temper revision | `fc52fc6f3d2a91f6969e9ea61b21666e32cbffa7` |
| Provider / model | `openai-codex` / `gpt-5.6-terra` |
| Provider region annotation | `loopback` |
| Host | Linux x86-64, 8 logical CPUs |
| Trace capture | diagnostic |
| Stable-cache policy | warm before the included enabled set |
| Provider executable version | `codebase-memory-mcp 0.9.0` |

A preliminary five-run enabled batch is excluded. Its stable project was absent
for the first three repetitions and was bootstrapped while diagnosing the cold
index failure, so cache state was not matched within that batch. The excluded
raw artifacts remain local and are not evidence for the comparison.

## Included results

All medians below have five samples unless a coverage note says otherwise.

| Measure | Enabled | Disabled | Forced unavailable |
| --- | ---: | ---: | ---: |
| Completed agent runs | 5/5 | 5/5 | 5/5 |
| Exact patch and all host checks | 4/5 | 4/5 | 2/5 |
| Median model attempts | 8 | 7 | 9 |
| Median input tokens | 19,291 | 13,142 | 15,593 |
| Median tool calls | 23 | 19 | 21 |
| Median graph calls | 7 | 0 | 3 |
| Median graph successes | 7 | 0 | 0 |
| Median graph failures | 0 | 0 | 3 |
| Median immediate systemic repeats | 0 | 0 | 0 |
| Median wall time (advisory) | 102,940 ms | 93,178 ms | 142,051 ms |
| Median graph discovery time (advisory) | 74 ms | 0 ms | 1 ms |

Four enabled trials had complete relevance evidence. Across those trials, 8 of
22 classified successful graph calls were relevant (36.4%); the fifth trial's
relevance evidence was unavailable and was not converted to zero. This misses
the predeclared 50% gate. Broad architecture calls and empty code searches made
up much of the irrelevant success count, even though focused graph results did
guide selection of the implementation, caller, and focused test.

The aggregate conventional-discovery total is unavailable for both enabled and
disabled. Models used compound shell commands such as an initial status check
followed by `find`; these commands did not start with a declared discovery
prefix. The analyzer correctly retained incomplete shell-classification
coverage instead of guessing or reporting zero. Component medians were enabled
0 grep, 0 find, and 1 read versus disabled 2 grep, 1 find, and 1 read, but those
components cannot establish the predeclared total-call improvement gate.

The enabled and disabled exact-patch misses were functionally correct and passed
the Rust tests, but added explanatory text not present in `expected.patch`.
Three forced-unavailable trials likewise missed the byte-exact patch while the
agent completed. They remain failures under the frozen gate. The gate was not
relaxed after observing the result.

Every forced-unavailable trial emitted one safe `provider_protocol` category,
then two or three `circuit_open` results for graph calls already requested in
the same model batch. No immediate systemic repeat was measured, and no raw
provider text survived in the typed failure evidence. The model subsequently
used conventional tools, so the bounded-fallback behavior passed even though
only two final patches passed the exact correctness gate.

## Gate verdict

| Gate | Verdict | Evidence |
| --- | --- | --- |
| Five trials per condition | pass | 5 completed runs in each of 3 conditions. |
| Exact patch and host validation in every trial | fail | Enabled 4/5; disabled 4/5; unavailable 2/5. |
| Correctness no worse than disabled | pass, insufficient | Both included conditions were 4/5, but the all-trials gate failed. |
| Graph success after readiness | pass for warm cache | All classified enabled graph calls succeeded. |
| Decision relevance at least 50% | fail | 8/22 (36.4%) with complete evidence. |
| Material improvement at least 20% | unavailable | Compound-shell classification coverage prevents a comparable median. |
| Bounded unavailable fallback | pass | 5/5 had one typed provider failure, an open circuit, conventional fallback, and zero immediate systemic repeats. |

## Required follow-up and rerun

Before a fresh candidate set:

1. Make the benchmark classify discovery within common compound shell commands
   using a reviewable parsed-command rule, rather than expanding prefixes after
   seeing results. Preserve unknown coverage for commands that cannot be parsed.
2. Tighten default graph-use guidance toward targeted search, path tracing, and
   snippets, and away from broad architecture queries when the task already
   names a concrete defect. This is a product behavior change and needs its own
   reviewed implementation, not a post-hoc rubric change.
3. Investigate cold stable-project creation. The excluded batch observed a
   categorized `index_failure` followed by `circuit_open`; explicit bootstrap
   succeeded and the clean warm batch then had graph success.
4. Keep the byte-exact correctness gate unchanged and run a wholly new matched
   set. Do not merge this report as deployment approval unless every frozen gate
   passes in that replacement set.

## Privacy and retention review

This checked-in report contains only aggregate counts, distributions,
non-secret configuration metadata, and privacy-safe failure categories. It does
not contain credentials, run identifiers, temporary paths, raw traces, model
transcripts, tool arguments/results, source excerpts, provider stderr, or cache
databases. Raw diagnostic artifacts are retained only in the operator-owned
local state directory for short-lived review and must not be uploaded or
committed.

## Post-deployment sampling plan

Deployment remains blocked by this report. After a later trial set passes and a
candidate is deployed, sample fresh engineer, architect, and scenario-author
traces. For each role, report graph attempts and successes, decision relevance,
conventional fallback calls, readiness and graph latency coverage, immediate
systemic retries, and typed safe failure categories. Compare failure rate with
the pre-change 120 failures in 120 recent graph calls; do not compare raw trace
content or expose provider diagnostics.
