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

`ForgejoConfig::from_env` reads:

| Variable | Purpose |
| --- | --- |
| `FORGEJO_URL` | required Forgejo base URL; trailing slashes are stripped |
| `FORGEJO_ACCESS_TOKEN` | required REST token, sent as `Authorization: token <token>` |
| `FORGEJO_DEFAULT_REPO` | optional `owner/repo` default |
| `FORGEJO_USERNAME` / `FORGEJO_PASSWORD` | optional web-UI login used only for the CI fallback |

REST requests are sent under `/api/v1`, JSON typed, paginated with
`limit`/`page`, and capped internally. Credentials are redacted from `Debug` and
never included in errors.

Default tests are offline request-shape/contract tests. Ignored live tests start
a throwaway local Forgejo plus host-mode `forgejo-runner`; they do not read
external Forgejo credentials. See
[`run-forgejo-multiprocess-e2e.md`](../how-to/run-forgejo-multiprocess-e2e.md)
for the fixture/cache model.

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
- labelled PR scans use `/issues?type=pulls&labels=...` as the provider label
  index, not `/pulls`.

`body_contains`, author, and assignee filters are applied client-side after the
narrowest safe provider query. Forgejo 7.0.x has no reliable exact body-substring
search. Summary list calls (`details.dependencies=false`) skip dependency
N+1s; labelled PR summary rows may also omit branch refs, head/base SHAs,
requested reviewers, and merge records. Use exact `get_*` or full-detail lists
when those fields matter.

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
exact `get_*` or a full-detail list when dependency state matters.

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
`/actions/runs` plus `/actions/tasks`. Runs are matched to a query by PR ref,
head SHA, event payload PR data, and PR head branch; tasks are grouped into the
latest attempt and mapped to portable `CiJob`s.

Forgejo 7.0.x lacks those REST run/task endpoints. If web-UI credentials are
configured, the backend logs in with CSRF, scrapes run ids from the Actions page,
and reads live-view JSON from `POST /{owner}/{repo}/actions/runs/{run}/jobs/{job}`.
This path bypasses `/api/v1`, uses cookies instead of the token, observes raw
redirects, and is isolated in `ci_ui.rs` / `ci_ui_parse.rs` because the HTML/JSON
shape is version-sensitive.

The fallback never fabricates a verdict: missing/unreadable CI is a `Backend`
error unless REST or the web UI can read it. Branch matching is intentional so a
fail-then-pass PR keeps both verdicts even though they are on different SHAs of
the same head branch; cancelled superseded runs are dropped. See
[ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md).
