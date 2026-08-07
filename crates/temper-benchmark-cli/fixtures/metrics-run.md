# Agent session benchmark

## Run

| Field | Value |
| --- | --- |
| Summary version | 1 |
| Run ID | `run-metrics` |
| Repository | `ai/temper` |
| Artifact | `ai/temper#546` |
| Trace source | raw_events_jsonl |
| Capture | diagnostic |
| Events | 33/33 |
| Attachments | 0/0 |

## Outcome

| Metric | Value |
| --- | ---: |
| Status | cancelled |
| Reason | cancelled |
| Wall time | 1000 ms |
| Turns | 2 |
| Task correctness | failed |
| Host validation | unavailable |

## Model and tokens

| Metric | Value |
| --- | ---: |
| Distinct calls | 2 |
| Provider attempts | 3 |
| Succeeded attempts | 1 |
| Failed attempts | 1 |
| Cancelled attempts | 1 |
| Retries | 1 |
| Provider failures | 1 |
| Cumulative model time | 370 ms |
| Model time coverage | 3/3 |
| Cumulative TTFT | 50 ms |
| TTFT coverage | 1/3 |
| Input tokens | 100 |
| Output tokens | 20 |
| Cache-read tokens | 30 |
| Cache-write tokens | 4 |
| Token coverage | 1/1 |

## Tools

| Tool | Calls | Succeeded | Failed | Cancelled | Duration | Coverage |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| bash | 1 | 1 | 0 | 0 | 150 ms | 1/1 |
| edit | 2 | 1 | 1 | 0 | 18 ms | 2/2 |
| grep | 1 | 0 | 0 | 1 | 40 ms | 1/1 |
| read | 1 | 1 | 0 | 0 | 40 ms | 1/1 |
| submit_for_pr | 3 | 3 | 0 | 0 | 15 ms | 3/3 |
| write | 2 | 2 | 0 | 0 | 60 ms | 2/2 |
| **Total** | **10** | **8** | **1** | **1** | **323 ms** | **10/10** |

### Slowest calls

| Rank | Tool | Call ID | Duration |
| ---: | --- | --- | ---: |
| 1 | bash | `validate-1` | 150 ms |
| 2 | grep | `grep-1` | 40 ms |
| 3 | read | `read-1` | 40 ms |
| 4 | write | `write-1` | 40 ms |
| 5 | write | `write-2` | 20 ms |
| 6 | edit | `edit-2` | 10 ms |
| 7 | edit | `edit-failed` | 8 ms |
| 8 | submit_for_pr | `submit-accepted-1` | 5 ms |
| 9 | submit_for_pr | `submit-accepted-2` | 5 ms |
| 10 | submit_for_pr | `submit-rejected` | 5 ms |

## Graph discovery and decision relevance

_No graph calls or decision-relevance rubric observed._

## Mutation and validation structure

| Metric | Value |
| --- | ---: |
| Failed edit attempts | 1 |
| Write/edit mutations | 3 |
| Mutation turns | 1 |
| Single-mutation turns | 0 |
| Maximum mutations per turn | 3 |
| Validation boundaries | 2 |
| Post-validation mutations | 1 |
| Validation invalidations | 1 |
| Revalidations | 1 |

## Observability diagnostics

| Severity | Code | Sequence | Detail |
| --- | --- | ---: | --- |
| info | host_evidence_unavailable | — | offline traces do not contain host metadata |
| info | diff_evidence_unavailable | — | offline traces do not contain final workspace diff evidence |
| info | validation_evidence_unavailable | — | offline traces do not contain host-side validation evidence |
