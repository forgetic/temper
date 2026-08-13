# Controlled graph-consumption validation evidence: 2026-08-13

Status: **feature #1009 graph-consumption validation passed**.

This report records privacy-reviewed aggregate evidence for the transformed,
multi-call codebase-memory consumption work. It does not publish or summarize
individual prompts, provider results, source excerpts, tool arguments, run
identifiers, temporary paths, or diagnostic traces.

## Candidate and execution order

The final aggregate feature head was
`e8db2cb7bcdee5a1fd87854cde2435f0d7664430`. Its source tree is identical to
the corrective head used for the final pre-PR and enabled-smoke validation.

Validation followed the required order:

1. run the focused enabled smoke against the exact final source tree;
2. require the minimal expected patch, task correctness, all host validation,
   current-root source evidence, complete typed correlations, a declared
   selection, complete relevance coverage, and at least 50% relevant successful
   graph results;
3. freeze the benchmark inputs and verifier; and
4. only then run five enabled, five disabled, and five forced-unavailable live
   repetitions.

No benchmark fixture, Jig transcript, decision target, or verifier was changed
between the qualifying smoke and the matrix.

## Qualifying enabled smoke

| Gate | Result |
| --- | ---: |
| Agent task correctness | pass |
| Byte-exact expected patch | pass |
| Host validation | pass |
| Current-root source evidence | pass |
| Complete typed graph correlations | 13/13 |
| Complete relevance coverage | 13/13 |
| Successful graph calls | 13/13 |
| Relevant successful graph results | 7/13 (53.8%) |
| Declared implementation selection | pass |

The smoke changed exactly the intended routing implementation with the checked-
in minimal patch. The analyzer recognized later-turn consumption through typed
lineage without inspecting provider prose or copying provider values into the
run summary.

## Frozen 5-by-3 matrix

The following counts cover all 15 repetitions. Correctness means the measured
run produced the required product result under the unchanged host-owned gates.

| Measure | Enabled | Disabled | Forced unavailable |
| --- | ---: | ---: | ---: |
| Correct repetitions | 5/5 | 3/5 | 3/5 |
| Successful graph calls | 80/80 | 0 | 0 |
| Relevant successful graph calls | 36/80 (45.0%) | not applicable | not applicable |
| Classified index failures | 0 | 0 | 8 |
| Immediate graph retries after systemic failure | 0 | not applicable | 0 in every repetition |
| Conventional fallback exercised | not required | yes | yes |

Every enabled repetition produced the byte-exact expected patch and passed host
validation. The enabled condition's correctness was 100%, compared with 60%
for each control condition: a 40 percentage-point aggregate effectiveness
difference under the frozen task and correctness gates.

The matrix's 36/80 aggregate relevance rate is retained as observed rather than
being rounded up, filtered, or used to redefine the gate. The pre-matrix smoke
is the required at-least-50% exact-final-tree acceptance run; the subsequent
matrix exposes live model variance and preserves its complete sample counts.

Forced-unavailable repetitions had no graph successes and no immediate repeat
after a systemic failure. They fell back to conventional discovery without an
eager provider retry.

## Privacy review

The privacy scan covered every durable output tree from all 15 matrix
repetitions. It found neither the fixture secret sentinel nor the secret-shaped
authorization prefix. This checked-in report contains only:

- the candidate revision;
- condition-level sample and aggregate counts;
- pass/fail gate outcomes; and
- privacy-safe failure categories and retry counts.

Diagnostic exports remain operator-local and are intentionally not committed.
The durable evidence contains no provider payload, model transcript, source
content, tool target, target digest, credential, or local artifact location.

## Interpretation

This evidence validates feature #1009's goal: successful targeted graph results
can be consumed across later transformed calls, reach the smallest correct
mutation, and retain bounded unavailable fallback. It also demonstrates why
transport success and relevance remain separate metrics: all 80 enabled matrix
graph calls succeeded, while 36 were classified as relevant.

The result does not turn deterministic harness timing into model-performance
evidence and does not replace the separately predeclared discovery-cost
deployment criterion. The unsuccessful August 6 trial set remains useful as a
historical failed baseline, but it is not evidence against this exact final
source tree.
