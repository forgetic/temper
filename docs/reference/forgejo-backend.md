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
| `web_ui` (`with_web_ui_credentials`) | optional web-UI login used only for the CI fallback |
| `ci_diagnostics` (`with_ci_diagnostics`) | when set, web-UI CI fallback reads are logged to stderr |

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
| CI job | `forgejo:{owner}/{repo}:actions:{run}:{job_index}:{task_id}` |
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
- CI is REST-first with a version-sensitive web-UI fallback for Forgejo 7.0.x;
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
narrowest safe provider query. Forgejo 7.0.x has no reliable exact body-substring
search. Ordinary `IssueQuery.labels` and `PullRequestQuery.labels` remain portable
all-of filters: although Forgejo versions may interpret the single
comma-separated provider filter as OR, the backend applies a final local
all-label check.

Consolidated candidate reads use Forgejo's provider-side any-label search, with
one request shape per lifecycle bucket. Forgejo 15's repository-scoped
`/repos/{owner}/{repo}/issues` endpoint actually applies multiple label names as
all-of even though its API description says any-of. Consequently, a candidate
bucket with multiple interest labels uses the owner-scoped
`/repos/issues/search?owner=...&labels=...` index and locally rejects rows whose
embedded repository identity is not the requested repository. Single-label and
unfiltered buckets stay on the repository endpoint. Rows are always locally
retained only when they carry at least one requested label. Issue buckets send
`type=issues`; PR buckets send `type=pulls`. Open buckets send `state=open`.
Terminal issue buckets send `state=closed`, while terminal PR discovery also
sends just one `state=closed` request and locally separates portable `Closed`
and `Merged` rows. Terminal workflow planning always supplies labels, and the
backend never substitutes an unlabelled issue or pull-history read for labelled
discovery. Every bucket follows the shared `limit`/`page` pagination loop,
deduplicates rows repeated across pages, and returns deterministic number/ID
order. Thus a one-page bucket costs one provider list request regardless of
interest-label count.

Summary list calls (`details.dependencies=false`) skip dependency N+1s;
labelled PR summary rows may also omit branch refs, head/base SHAs, requested
reviewers, and merge records. Unambiguous candidate summaries are materialized
from the issue index without exact PR reads. A closed PR-as-issue row that lacks
a sufficient merge marker is the exception: `/pulls/{number}` is read before
closed/merged filtering. Exact PR summary reads similarly perform only
`/pulls/{number}` and return empty dependencies; exact full reads additionally
use `/issues/{number}/dependencies`.

The checked-in 17-label `reference-delivery` workflow locks these one-page
provider ceilings:

| Consumer | Candidate-list requests |
| --- | ---: |
| broad role discovery | <= 4 populated issue/PR lifecycle buckets |
| bounded reconciliation | <= 4 populated issue/PR lifecycle buckets |
| automated discovery | <= 2 populated open issue/PR buckets |
| second unchanged mechanical pass | 6 for the reference workflow; 0 exact artifact and 0 dependency requests |

Terminal requests always include workflow-derived labels. The only candidate
row allowed to add a summary exact read is a closed PR row whose issue-index
merge marker is ambiguous. Cold dependency-gated reconciliation may additionally
perform one full exact read per uncached source; the long-lived mechanical cache
removes those reads on an unchanged warm pass and forcibly refreshes them within
15 minutes.

The ignored local benchmark runs that cold/warm pair against the cached Forgejo
fixture and the real HTTP client:

```sh
cargo test -p temper-testing --test idle_request_budgets \
  local_forgejo_two_pass_idle_broad_benchmark \
  -- --ignored --exact --nocapture
```

It prints each broad-pass duration and normalized warm-pass method/path counts,
then enforces the warm shape above. The first invocation may populate the pinned
Forgejo binary cache (or use `TEMPER_FORGEJO_BINARY`); default CI never starts a
server. Additional pages multiply requests per bucket. They do not reintroduce
one request per workflow label.

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
`{ "index": <target>, "owner": <owner>, "repo": <repo> }`. On Forgejo 7.0.12,
omitting owner/repo resolves against an empty repository and fails.

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

### CI is REST-first, web-UI fallback on Forgejo 7.0.x

Newer Forgejo/Gitea Actions REST endpoints are used when available:
`/actions/runs` plus `/actions/tasks`. For PR-only queries, runs are matched by
PR ref, provider head SHA, event payload PR data, or PR head branch so historical
same-branch diagnostics remain visible. Tasks are grouped into the latest
attempt and mapped to portable `CiJob`s. REST task conclusion/reason fields and
the run plus latest-attempt identities are retained when present.
Machine-readable task/run reasons can refine a broad failure into an explicit
infrastructure category; arbitrary reason prose is retained for diagnostics but
never guessed into a category. An explicit task `conclusion: failure` is the
trustworthy evidence required for ordinary source/test `failure`. Forgejo also
uses a status-only `failure` when execution is terminalized after runner loss,
including the captured run #591/task 3385 shape, so bare `status: failure`
without a conclusion or specific machine reason maps to terminal `unknown` and
requires recovery rather than writable repair. Success, cancellation,
interruption, timeout, runner loss, startup failure, action-required, neutral,
and skipped retain their explicit categories. A task known to be terminal
through a completed task or run but carrying an unrecognized result likewise
maps to terminal `unknown`; the parent run's failure does not manufacture an
ordinary job failure. Logs and output cadence are not classification evidence.
Printable raw evidence is control-sanitized and bounded to 256 UTF-8 bytes.

A non-empty `CiJobQuery.commit_sha` changes this to strict commit ownership,
including when `pull_request_id` is also present. The run must expose a matching
provider SHA in its run fields or pull-request event payload; PR numbers, refs,
branches, and the separately fetched PR head cannot widen the query. Exact SHAs
and full/abbreviated pairs of at least seven characters match. Missing provider
SHA evidence is conservative: the run/job is omitted, and the query SHA is never
copied into a job to manufacture ownership. A matching current run contributes
queued or running tasks with its provider commit SHA. A registered run with no
tasks contributes no jobs but sets `CiJobListing::matching_ci_present`, so the
gate remains pending without the missing-CI monitor mistaking runner queueing
for an absent run.

Forgejo 7.0.x lacks those REST run/task endpoints. If web-UI credentials are
configured, the backend logs in with the version-appropriate cookie/CSRF
handshake, scrapes at most 20 newest-first run ids from
`/{owner}/{repo}/actions`, and reads live-view JSON from
`POST /{owner}/{repo}/actions/runs/{run}/jobs/{job}/attempt/1` with
`{"logCursors":[]}`. Forgejo 15.0.3 uses that attempt-qualified route and
password cookies without CSRF; a `404` falls back to Forgejo 7's unqualified
`…/jobs/{job}` route, whose cookie jar includes `_csrf` and whose request sends
`X-Csrf-Token`. This path bypasses `/api/v1`, never sends the REST token, and is
isolated in `ci_ui` / `ci_ui_parse` because the HTML/JSON shapes are
version-sensitive. The fallback retains each run id and explicit attempt
coordinate along with any job/run conclusion and reason fields present in the
live payload. It applies the same conservative terminal-category mapping and
bounded evidence sanitization as REST.

A live-view `500` triggers one fresh login and one retry of the same route with
rebuilt cookies and CSRF headers. Persistent non-authentication HTTP failures
become typed per-run unreadable outcomes; login, Actions-page discovery,
transport, and persistent authentication failures remain hard. An exact
`get_ci_job` has no alternate ordered evidence, so an unreadable result remains
a detailed, secret-free `Backend` error after that retry.

List aggregation continues across the full bounded window but treats the first
unreadable run as a newest-first trust boundary. Matching jobs already read on
the newer side remain usable (including a newer success followed only by broken
older history). Matching evidence on the older side is omitted because the
unreadable run could supersede it. If the boundary appears before the first
readable target match, or every recent run is unreadable, the result is
`Ok([])`: the gate stays pending rather than accepting potentially stale green
CI. Empty degraded reads are non-terminal in the existing CI cache and are
therefore fetched again.

Each degraded list read emits one bounded warning with a safe representative
repository/run/job/status/retry count, total unreadable and omitted diagnostic
counts, and `continued` or `pending` outcome in structured fields and the
operator message. Cookies, CSRF values, credentials, and response bodies are
never retained in that diagnostic.

The fallback otherwise preserves strict explicit commit ownership: PR branch
and pseudo-ref widening is used only when no commit filter was supplied,
preserving PR-only fail-then-pass history across different heads. Query status
filtering and sorting remain in the common list path, and cancelled superseded
runs are dropped. Empty and queued/running reads are never eligible for terminal
cache reuse; cached ownership uses the same safe SHA comparison. See
[ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md).

Exact-attempt retry is deliberately `unsupported` on Forgejo. Actions rerun
routes and attempt semantics vary across supported releases, and the 7.0.x
web-UI fallback is a version-sensitive read surface rather than a verified
mutation contract. The backend validates that the portable repository and PR
IDs name the same Forgejo repository, then fails closed without HTTP. It never
guesses a REST/UI endpoint and never writes a commit or ref to trigger CI.
Operators should configure the workflow's `pull_request_read_only` interruption
diagnostic when available; otherwise Temper parks the PR with the exact
head/run/attempt, provider evidence, URLs, timestamps, and unsupported retry
outcome required for a safe manual retrigger.
