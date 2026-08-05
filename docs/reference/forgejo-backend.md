# Forgejo backend reference

`temper-forge-forgejo` is the Forgejo implementation of the portable
`temper_forge::Forge` trait.

Rust type: `temper_forge_forgejo::ForgejoForge<C>`. Production uses
`ReqwestHttpClient`; tests inject a mock `HttpClient`. The backend has no local
persistent store: Forgejo is authoritative. The only local state is a per-process
version cache used for best-effort conditional writes.

Read this page for Forgejo-specific surprises. For the portable API contract,
read [`forge-interface.md`](forge-interface.md).

## Configuration and tests

`ForgejoConfig` is built only from explicit values — this crate is a library, not
a process boundary, and never reads the environment. The wiring layer translates
a resolved temper config into one via `temper-engine-service`'s `forgejo_config`
adapter, supplying:

| Field | Purpose |
| --- | --- |
| `base_url` | required Forgejo base URL; trailing slashes are stripped |
| `token` | required REST token, sent as `Authorization: token <token>` |
| `default_owner` / `default_name` (`with_default_repo`) | optional `owner/repo` default |

REST requests are sent under `/api/v1`, JSON typed, paginated with
`limit`/`page`, and capped internally. Credentials are redacted from `Debug` and
never included in errors.

Default tests are offline request-shape/contract tests. Ignored live tests start
a throwaway local Forgejo plus host-mode `forgejo-runner`; they do not read
external Forgejo credentials. See
[`run-daemon-e2e.md`](../how-to/run-daemon-e2e.md)
for commands and [the Forgejo e2e fixture reference](forgejo-e2e-fixture.md) for
the fixture/cache model.

## Identifier encodings

IDs are backend-owned opaque strings; workflow code must not parse them.

| Portable id | Forgejo encoding |
| --- | --- |
| Repository | `forgejo:{owner}/{repo}` |
| Issue | `forgejo:{owner}/{repo}:issue:{number}` |
| Pull request | `forgejo:{owner}/{repo}:pull:{number}` |
| Comment | `forgejo:{owner}/{repo}:comment:{id}` |
| Review | `forgejo:{owner}/{repo}:review:{id}` |
| Label | `forgejo:{owner}/{repo}:label:{id}` |
| CI job | `forgejo:{owner}/{repo}:actions:{provider_run_id}:{job_id}:{attempt}:{task_id}` |
| User | the raw Forgejo login |

## Coverage and error shape

`ForgejoForge<C>` implements every `Forge` method. Known partial/best-effort
areas are:

- repository creation for non-self owners assumes the token may create repos in
  that org;
- `ReviewDecision::Pending` submission is rejected with `InvalidRequest`;
- merge payload fields are best-effort Gitea/Forgejo shape;
- native dependency add/remove payloads are Forgejo/Gitea-specific;
- optimistic concurrency is not provider-atomic;
- CI requires Forgejo 16.0.1 and uses token-authenticated run/job JSON APIs only;
- Forgejo-only surfaces such as teams, branch protection policy, and raw CI logs
  stay outside the portable trait.

Central HTTP mapping: `404 -> NotFound` (lookup helpers may turn this into
`Ok(None)`), `409`/`412 -> Conflict`, `400`/`422 -> InvalidRequest`, and auth,
transport, `5xx`, or unexpected statuses -> `Backend`. Operations with stronger
semantics override this locally, e.g. duplicate repository create ->
`AlreadyExists`, merge rejections -> `Conflict`, and dependency write `404`
after target verification -> `InvalidRequest`.

## Forgejo-specific quirks

### Issues and pull requests share the issue namespace

Forgejo models pull requests as issues. Consequences:

- issue reads must exclude rows with a `pull_request` marker;
- PR comments, labels, assignees, and dependency links use `/issues/{number}`;
- issue and PR numbers are one repository-scoped namespace on Forgejo;
- `Forge::item_number_namespace()` therefore reports `Shared`, allowing fresh
  same-pass PR candidates to resolve dependency target state without a redundant
  `/issues/{number}` collision probe;
- labelled PR scans use `/issues?type=pulls&labels=...` as the provider label
  index, not `/pulls`.

`body_contains`, author, and assignee filters are applied client-side after the
narrowest safe provider query because exact body-substring search is not part of
the supported Forgejo API contract. Ordinary `IssueQuery.labels` and
`PullRequestQuery.labels` remain portable all-of filters: although Forgejo may
interpret the single comma-separated provider filter as OR, the backend applies
a final local all-label check.

Consolidated candidate reads preserve provider-side any-label behavior without
letting sibling repositories consume a bounded page. Forgejo's
repository-scoped `/repos/{owner}/{repo}/issues` endpoint applies multiple label
names inconsistently across supported versions, while owner-scoped search can
fill a page with rows from another repository. Temper therefore uses one
single-label stream per normalized any-of label on the exact repository
endpoint and merges/deduplicates those streams locally. Unfiltered buckets use
one exact-repository stream. The backend accepts at most 32 normalized label
streams per bucket.

Every stream sends `sort=updated&direction=asc`. Provider page offsets are
retained only while traversing one equal-timestamp tie; advancing the inclusive
`since` timestamp resets that stream to page one. A bounded first page freezes a
provider `before` boundary; continuations retain that boundary and send `since`
for the last committed timestamp. A small opaque backend cursor records provider
page movement so equal timestamps do not restart at page one. Issue buckets send
`type=issues`; PR buckets send `type=pulls`. Open buckets send `state=open`.
Terminal issue buckets send `state=closed`, while terminal PR discovery also
sends just `state=closed` and locally separates portable `Closed` and `Merged`
rows. Terminal workflow planning always supplies labels, and the backend never
substitutes an unlabelled issue or pull-history read for labelled discovery.

Unpaged open discovery remains exhaustive and level-triggered. A terminal or
explicitly paged bucket decodes at most 1,001 provider rows and sends at most 64
provider list requests, regardless of repository history. Continuation is
returned only after the complete bounded window succeeds; transport, status, or
decode failure advances nothing. Rows are ordered oldest update first with
number and typed id tie-breaks, then deterministically deduplicated.

Summary list calls (`details.dependencies=false`) skip dependency N+1s;
labelled PR summary rows may also omit branch refs, head/base SHAs, requested
reviewers, and merge records. Unambiguous candidate summaries are materialized
from the issue index without exact PR reads. A closed PR-as-issue row that lacks
a sufficient merge marker is the exception: `/pulls/{number}` is read before
closed/merged filtering. Exact PR summary reads similarly perform only
`/pulls/{number}` and return empty dependencies; exact full reads additionally
use `/issues/{number}/dependencies`.

The checked-in 17-label `reference-delivery` workflow remains bounded by the
fixed per-pass provider ceilings above. Exact request count depends on populated
normalized label streams, not on total terminal-history cardinality. Overflow
commits a typed continuation and returns control to the wake coordinator; it
never extends the current list traversal beyond 64 requests or 1,001 decoded
rows. A default 100-row PR page can additionally make at most 100 exact summary
reads when every retained row lacks an unambiguous merge marker, so the full
terminal bucket remains at or below a fixed 164 provider requests. With the portable
100-row retained page, `N` matching terminal rows need at most `ceil(N/100)` successful cold generations. Summary exact reads remain
separately bounded by rows in that provider-retained page.

Continuation and retained-target state live in the runner, not this stateless
backend. The default runner owner bounds 64 repositories, eight buckets and 256
exact targets per repository. Process restart is cold. Workflow-fingerprint or
bucket changes, non-advancing/provider anomalies, and local workflow mutations
invalidate sweep authority; failed HTTP/status/decode pages preserve the last
committed cursor. Targeted webhooks retain a bounded exact target but do not
short-circuit the next periodic sweep. Explicit deep audit uses ordinary
whole-history lists and is intentionally outside all periodic row/request and
latency guarantees.

Terminal requests always include workflow-derived labels. An ambiguous closed
PR row may add a summary exact read only after it enters the current bounded
provider page; ambiguous history beyond the continuation window adds no
`/pulls/{number}` traffic. Cold dependency-gated reconciliation may additionally
perform one full exact read per uncached retained source; the long-lived
mechanical cache removes those reads on an unchanged warm pass and forcibly
refreshes them within 15 minutes.

### Labels and assignees are set-like but Forgejo wants label ids

Forgejo issue-label endpoints take numeric label ids, not names. When a label
mutation is requested the backend lists repository labels once, resolves names to
ids, applies `set_labels`, then removals, then additions. Unknown label names are
skipped; workflows are expected to upsert labels before assigning them.

Assignees are rewritten as `current - remove + add` through the issue patch
endpoint.

### Reviews preserve history

Forgejo may mark an old verdict dismissed/stale when the same reviewer submits a
new one. The backend still returns dismissed/stale verdict events so review
history matches the reference backends; portable aggregation already takes the
latest non-comment verdict per reviewer. Review timestamps can tie at one-second
precision, so equal timestamps are not meaningful ordering evidence.

`submit_pull_request_review` uses the one-call submit endpoint for `APPROVED`,
`REQUEST_CHANGES`, and `COMMENT`. Pending reviews are deliberately unsupported
because the historical two-step flow can drop the body.

### Merge results are coarse

`merge_pull_request` posts `{ Do, MergeTitleField?, MergeMessageField? }` with
`Do = merge|squash|rebase`, then re-fetches the PR for merge metadata. Forgejo
does not echo the merge method on later reads, so ordinary reads default
`MergeRecord::method` to `MergeCommit`; the merge call itself reports the method
that was requested. Provider rejections such as branch protection, already
merged, or content conflict all collapse to portable `Conflict`.

### Dependency links use Gitea's issue dependency endpoint

Both issue and PR dependency methods use
`/repos/{owner}/{repo}/issues/{number}/dependencies` with the source item number.
Reads return the items the source is blocked by, sorted and deduplicated.

Add/remove bodies must include the target repository coordinate:
`{ "index": <target>, "owner": <owner>, "repo": <repo> }`. Omitting owner/repo
can resolve against an empty repository and fail.

A dependency read `404` is treated as an empty list for compatibility with
providers lacking the endpoint. Add/remove first verify the target exists;
after that, a dependency endpoint `404` is `InvalidRequest`, not silent success.
Mutation return values may not include enriched dependency vectors; reload with
an explicit full-detail exact get or a full-detail list when dependency state
matters. Metadata-only fan-out and recovery use summary exact gets and therefore
never call this endpoint merely to update workflow bodies.

### Fan-out mutation request budget

Forgejo issue creation resolves label names before the POST, caches the
repository name/id map across sibling creates, and materializes the POST response
directly with empty dependencies. A successful label upsert invalidates that
repository cache. A staged child therefore keeps atomic final labels without a
post-create issue/dependency refetch.

`update_issue_from_snapshot` derives current labels and assignees from its
validated snapshot. Conditional writes perform one CAS preflight; unconditional
writes perform none. Intent-owned staged-child wiring and new-protocol
activation are body-only unconditional writes because the staging guard keeps
them undispatchable and excludes concurrent workflow ownership. Label changes
are sent while `metadata.staged` is still true, and the body PATCH that can clear
staging is last. Its response becomes the committed representation, with no
post-write exact or dependency read. Body-only wiring/activation updates issue
no label-list request. The executor loads a summary issue when the transition
has no dependency signal need and carries that validated source snapshot into
intent persistence instead of re-reading the parent.

For known-first same-repository fan-out with `N` children and `D` children that
have one or more dependencies, the core write ceiling is `4 + 2N + D`.
Dependency *edge* count does not add writes because each dependent child's
complete sorted edge set is written once. The ten-child acyclic maximum
(`D <= 9`, even though the maximum DAG has 45 edges) is 33 core writes.
Provider regression tests cap the corresponding Forgejo traffic at 24 GETs, 36
non-GETs, and 60 total requests; method/path traces are retained by
`temper_testing::counting_http` on failure. One injected crash plus replay is
separately capped by the portable suite at 30 Forge reads and 68 Forge writes,
with correlation recovery using one open/closed query pair per affected
repository rather than per child.

Activation ordering is part of the budget protocol, not an optimization:
children are created with final labels and `metadata.staged=true`, all child
wiring and parent aggregation complete, and only then does a final body PATCH
clear staging. Label repair for a legacy child happens while it is still staged.
No targeted, broad, startup, or poll path may dispatch a staged child.

The ignored local baseline uses the cached
`temper_testing::forgejo_server` fixture and the real `EngineHttpClient` behind
a counting client:

```sh
cargo test -p temper-testing --test forgejo_fanout_budget \
  local_forgejo_ten_child_fanout_meets_budget_and_crash_converges \
  -- --ignored --exact --nocapture
```

The first run needs network access to populate the pinned Forgejo binary cache
(or set `TEMPER_FORGEJO_BINARY` to an already downloaded binary). Subsequent
runs restore `.cache/forgejo/` into a fresh per-test server. The test
materializes the ten-child 45-edge maximum DAG, enforces the 60-request fresh
ceiling, injects an uncertain committed write and verifies convergence, prints
both wall times, and requires each local phase to finish within 15 seconds.

### HTTP correlation and webhook acknowledgement

Every Forgejo HTTP completion is a debug event with `method`, normalized `path`
(numeric resource ids become `{id}`), `operation`, `status`, and numeric
`duration_ms`. Requests made by an admitted wake
inherit `wake.run_id`; result application requests inherit `apply.id`. Count
operations by either span field rather than parsing the human message.

After HMAC verification and payload classification, the engine queues the HTTP
`202 Accepted` response before any debounce timer, Forge read, scan, or
mechanical work. A proven lease-heartbeat-only edit is still acknowledged `202`
and recorded as `wake.outcome=suppressed`. Invalid signatures remain `401` and
malformed payloads `400`; ambiguous valid payloads are acknowledged and broad
fall back.

For troubleshooting with `TEMPER_LOG_FORMAT=json`:

```sh
# Provider operations and statuses for one apply.
journalctl -u temper -o cat | jq -s \
  '[.[] | select((.span."apply.id" // ."apply.id")=="job-42" and .operation != null)] |
   group_by(.operation) | map({operation:.[0].operation, count:length, statuses:map(.status)})'

# Fan-out budget summaries that exceeded the documented provider ceiling.
journalctl -u temper -o cat | jq -c \
  'select(.measurement=="fan_out.completed" and ."provider.request_total">60)'

# Slow mechanical phases and the provider-request delta for their admitted wake.
journalctl -u temper -o cat | jq -c \
  'select(.measurement=="mechanical.phase" and .duration_ms>1000) |
   {run:(.span."wake.run_id" // ."wake.run_id"),phase:."mechanical.phase",
    scope:."mechanical.scope",duration_ms,requests:."provider.request_total"}'
```

A successful admitted run ends with `wake.phase=finish` and
`wake.outcome=completed`. Repeated `gate.evaluated` debug records only show that
a PR's state was read. To confirm a merge execution, query
`measurement=mechanical.landing_attempt` and require a `started` record followed
by a terminal outcome and `duration_ms` for the same repo, PR, queue, and
transition. When phase records report `provider.requests_available=false`, use
the correlated HTTP operation count because that backend cannot expose a
cumulative provider counter.

### Optimistic concurrency is per backend instance

Forgejo has no confirmed conditional-write contract. The backend records a
validator on reads (`ETag` when available, otherwise weak `updated_at`) in a
per-process cache and compares it before conditional `update_issue` /
`update_pull_request` writes. Versions are meaningful only when the read and
write use the same `ForgejoForge` instance.

With no validator, `CasMode::Strict` rejects the conditional write as
`InvalidRequest`; `CasMode::BestEffort` performs a weak read-before-write. This
is not atomic and `updated_at` has one-second granularity, so lease race safety
is best-effort on this backend.

### CI uses Forgejo 16 per-run jobs APIs

Forgejo 16.0.1 is the minimum supported release for every Temper Forgejo
integration, not only CI. The backend lists workflow
runs through `/repos/{owner}/{repo}/actions/runs`, strictly matches them to the
requested pull request and/or commit, and expands every match through:

```text
GET /repos/{owner}/{repo}/actions/runs/{provider_run_id}/jobs
```

`provider_run_id` is the run database `id`, not a repository-local display
number. Every job must provide `id`, `run_id`, `attempt`, `task_id`, `name`, and
`status`; those provider coordinates form the portable opaque job id. The
backend selects the largest provider-reported attempt per stable job id. It
never infers attempts from names or response order.

A non-empty `CiJobQuery.commit_sha` requires matching provider SHA evidence from
the run or pull-request event payload. PR numbers, refs, branches, and the
separately fetched PR head cannot widen that ownership check, and query values
are never copied into returned jobs. A matching run with no jobs sets
`CiJobListing::matching_ci_present`, keeping the gate pending while a runner is
assigned.

Missing, unauthorized, unsupported, malformed, cross-run, duplicate, or
zero-identity job responses fail closed as provider unavailability. There is no
repository-wide tasks, HTML, login, or live-view fallback. Status filtering and
sorting remain deterministic, and `get_ci_job` re-reads the encoded provider run
and exact provider job/attempt/task identity.

Forgejo 16 job rows expose status but no separate detailed conclusion/reason, and
host-mode `forgejo-runner` does not add a separately authenticated completion
category to that API. A bare `failure` therefore remains terminal `unknown` and
recovery-required; it is not reinterpreted as ordinary source/test failure.
Explicit success, cancellation, interruption, timeout, runner loss, startup
failure, action-required, neutral, and skipped statuses retain their portable
categories. Provider evidence is control-sanitized and bounded before it is
exposed.

#### Configured protected-workflow failure statements

Because neither the Forgejo v16 API nor the host runner provides a suitable
native carrier, the backend supports exactly one stronger-evidence mechanism:
a configured generic JSON endpoint. It is disabled unless
`[forge.ci_failure_evidence]` supplies all of:

```toml
[forge.ci_failure_evidence]
endpoint = "https://ci-evidence.example/v1/forgejo-failures"
issuer = "runner-host"
protected_producers = ["protected-ci"]
bearer_token = "ci-evidence-read-token"
hmac_key = "ci-evidence-hmac-key"
```

`bearer_token` and `hmac_key` are names in the selected credentials/secret
source, never literal secrets. The evidence endpoint is independent of the
Forgejo REST identity. A remote endpoint must use HTTPS; `http://localhost` and
`http://127.0.0.1` are accepted only for the same-host runner topology. The
backend never reads evidence configuration or credentials from ambient
environment variables and never authenticates evidence with a scenario, repo,
workflow, or Forgejo role identity.

The protected workflow serializes one compact JSON statement, computes
HMAC-SHA256 over those exact UTF-8 bytes, and publishes the statement plus
signature to the configured evidence service. The integrity key must be
available only to an allowlisted workflow whose definition and secret access
are protected from pull-request changes. Untrusted PR code must not receive the
key. The acquisition bearer credential is separate and read-only. A response to
`GET <endpoint>?repository_id=<opaque-id>&run_id=<provider-id>` has this closed
shape:

```json
{
  "schema_version": 1,
  "records": [{
    "statement": "{...compact statement JSON...}",
    "hmac_sha256": "sha256=<64 lowercase or uppercase hex digits>"
  }]
}
```

The statement schema is also closed. It contains `schema_version = 1`, one of
`source|build|test`, typed repository and optional PR IDs, exact 40- or 64-hex
head SHA, run/job/attempt/task IDs, producer and issuer IDs, and creation/expiry
timestamps. The backend verifies response bounds and schema, HMAC integrity,
configured issuer and producer authorization, the closed/open freshness window,
and every authoritative job coordinate. Forgejo's task identity is mandatory.
Acquisition occurs only after strict run/head matching and validation of all
per-run job rows. One proof can strengthen only its exact completed bare-failure
attempt; success and non-ordinary terminal categories are never overridden.

Malformed, oversized, unauthenticated, unauthorized, stale, future, expired,
duplicate-coordinate, contradictory, or mismatched evidence is ignored. Any
malformed, integrity-invalid, unauthorized, duplicate, or contradictory record
invalidates that complete acquisition batch, preventing a valid-looking sibling
from masking it. Missing endpoints, non-2xx responses, and transport failures
also leave the Forgejo result `unknown`; ordinary CI list/get operations remain
available. Warnings expose only one of the bounded diagnostic codes
`unavailable`, `oversized`, `malformed`, `unsupported_schema`,
`invalid_integrity`, `unauthorized_issuer`, `unauthorized_producer`,
`invalid_proof`, `duplicate_coordinate`, or `uncorrelated`. Response bodies,
statement values, credentials, and signatures are never included.

`list_ci_jobs_with_presence`, `list_ci_jobs`, and `get_ci_job` all use this same
post-correlation path. They retain identical typed conclusion and
`verified_failure` provenance, opaque IDs, latest-attempt behavior, sorting, and
matching-run presence. With this source absent or unusable—including historical
run 591—the status-only result remains `unknown` and recovery-required.

Exact-attempt retry remains `unsupported` on Forgejo. The backend validates that
the portable repository and PR IDs name the same repository, then fails closed
without guessing an endpoint or mutating a commit/ref. Operators should use a
configured read-only interruption diagnostic when available; otherwise Temper
parks the PR with exact head/run/attempt evidence for manual retriggering.

## Persistent-service upgrade boundary

The API-only binary must not be deployed against an older Forgejo service. Prove
the Bench-owned 16.0.1 fixture and merge the API-only feature first; an operator
then backs up, rehearses, migrates, and proves the persistent service while the
previous compatible Temper deployment remains running. Only after the jobs
endpoint is proven may the operator remove the obsolete `ci_user` key from the
deployed configuration and restart into the API-only binary. Repository tests
and scripts never perform that persistent migration. Follow the complete
[Forgejo 16 migration runbook](../how-to/migrate-forgejo-16-api-ci.md).
