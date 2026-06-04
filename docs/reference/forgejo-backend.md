# Forgejo backend reference

The `temper-forge-forgejo` crate adapts `temper_forge::Forge` to Forgejo's
HTTP API. It is a **best-effort, offline-tested** backend: the provider is
reached through a mockable HTTP seam, every contract test runs without a
network, and provider semantics that cannot be verified live (notably
conditional writes and merge payload shapes) are documented as best-effort.

Rust type: `temper_forge_forgejo::ForgejoForge<C>`, where `C` is the HTTP
client. Production uses `ReqwestHttpClient`; tests inject a recording mock.

## Configuration and transport

`ForgejoConfig` carries the base URL, the access token, an optional default
repository, the list page size, and the conditional-write mode. Requests are
built by prefixing the path with `/api/v1` and always sending
`Authorization: token <token>`, `Accept: application/json`, and
`Content-Type: application/json`, mirroring the reference TypeScript
integration. List endpoints paginate with `limit`/`page` and stop on the first
short page (bounded by an internal page cap of `MAX_LIST_PAGES`).

`ForgejoConfig` fields and their builders:

| Field | Builder | Default | Meaning |
| --- | --- | --- | --- |
| `base_url` | `new` | — | Forgejo host, trailing slashes stripped |
| `token` | `new` | — | personal access token; sent as `token <token>`, never logged |
| `default_owner` / `default_name` | `with_default_repo` | `None` | optional default repository |
| `page_limit` | `with_page_limit` | `DEFAULT_PAGE_LIMIT` (50) | page size for list requests |
| `cas_mode` | `with_cas_mode` | `CasMode::BestEffort` | conditional-write strategy |
| `web_ui` | `with_web_ui_credentials` | `None` | optional web-UI username+password for the CI read fallback (ADR 0019); redacted in `Debug`, never logged |

### Environment variables

`ForgejoConfig::from_env` reads the same names as the reference TypeScript
integration:

| Variable | Required | Meaning |
| --- | --- | --- |
| `FORGEJO_URL` | yes | base URL, e.g. `https://git.example.com` |
| `FORGEJO_ACCESS_TOKEN` | yes | personal access token |
| `FORGEJO_DEFAULT_REPO` | no | default repository as `owner/repo` |
| `FORGEJO_USERNAME` | no | web-UI login user for the CI read fallback (ADR 0019) |
| `FORGEJO_PASSWORD` | no | web-UI login password for the CI read fallback (ADR 0019) |

Blank required values are treated as missing (`ConfigError::MissingEnv`); a
`FORGEJO_DEFAULT_REPO` that is not exactly `owner/repo` is
`ConfigError::Invalid`. `FORGEJO_USERNAME` and `FORGEJO_PASSWORD` take effect
only when **both** are present and non-blank; either alone is ignored. The
token authenticates every REST operation; the web-UI password is needed only for
CI reads on a server that does not serve the Actions REST endpoints.

The optional live smoke test (`tests/live.rs`) is `#[ignore]`d, so a plain
`cargo test` never touches the network. When run with `--ignored`, it starts a
throwaway local Forgejo from a JSON-declared cached state plus a host-mode
`forgejo-runner` from the shared `.cache/forgejo/` binary cache; it does not read
external Forgejo credentials and does not need `TEMPER_FORGEJO_LIVE` or
`TEMPER_FORGEJO_LIVE_MUTATE`. Populate the binary cache with
`cargo test -p temper-forgejo-fixture --test cache -- --ignored`; state snapshots
are created on demand under `.cache/forgejo/states/`.

## Identifier scheme

Identifiers are backend-private encodings; workflow code must treat them as
opaque and never parse them.

- repository: `forgejo:{owner}/{repo}`
- issue: `forgejo:{owner}/{repo}:issue:{number}`
- pull request: `forgejo:{owner}/{repo}:pull:{number}`
- comment: `forgejo:{owner}/{repo}:comment:{id}`
- review: `forgejo:{owner}/{repo}:review:{id}`
- label: `forgejo:{owner}/{repo}:label:{id}`
- CI job: `forgejo:{owner}/{repo}:actions:{run}:{job_index}:{task_id}`
- user: the Forgejo login, unprefixed (so reviewer-request logins map directly)

## Trait implementation

`ForgejoForge<C>` implements the **full** `temper_forge::Forge` trait (see
`src/forge_impl.rs`), so it is a drop-in backend for the workflow runner:
`temper_workflow::Executor::new(&workflow, &forge)` accepts it in both the
concrete `ForgejoForge<C>` and the erased `&dyn Forge` forms. The trait impl is
a thin one-line delegation to the inherent method of the same name and
signature; the inherent methods remain the single source of truth and the
offline contract tests exercise them directly. `C: HttpClient` carries the
`Send + Sync` the trait requires.

The best-effort caveats documented below still apply when the backend is driven
through the trait — conditional writes use the per-process version cache rather
than a provider-enforced precondition, and merge/dependency payload shapes are
unverified against a live instance.

## Supported operations

`ForgejoForge<C>` implements **every** `Forge` method — there is no `todo!`,
`unimplemented!`, or silent stub in any of them. Each method either maps a real
Forgejo endpoint or, where the provider contract is unverified, returns a
**portable** `ForgeError` category and is documented as best-effort below.

The **Support** column reads:

- **Full** — backed by a Forgejo endpoint with no unverified assumptions.
- **Best-effort** — works against a real instance, but relies on a payload
  shape or semantic that is not yet confirmed live; the failure mode is a
  portable `ForgeError`, never a silently wrong result. See the linked section.
- **Partial** — a portable sub-case is deliberately rejected with a portable
  error (documented), while the rest is full.

| Domain | Operations | Support |
| --- | --- | --- |
| Identity | `current_user`, `get_user` | Full |
| Repositories | `get_repository`, `get_repository_by_path`, `list_repositories` | Full |
| Repositories | `create_repository` | Best-effort for non-self owners (see [Repositories](#repositories)) |
| Labels | `list_labels`, `upsert_label` | Full |
| Issues | `list_issues`, `get_issue`, `get_issue_by_number`, `create_issue` | Full |
| Issues | `update_issue` | Full; conditional updates best-effort (see [Optimistic concurrency](#optimistic-concurrency-best-effort)) |
| Issue comments | `list_issue_comments`, `add_issue_comment` | Full |
| Pull requests | `list_pull_requests`, `get_pull_request`, `get_pull_request_by_number`, `create_pull_request` | Full |
| Pull requests | `update_pull_request` | Full; conditional updates best-effort (see [Optimistic concurrency](#optimistic-concurrency-best-effort)) |
| PR comments | `list_pull_request_comments`, `add_pull_request_comment` | Full |
| Reviewers | `request_pull_request_reviewers` | Full |
| Reviews | `list_pull_request_reviews` | Full |
| Reviews | `submit_pull_request_review` | Partial — `Pending` rejected with `InvalidRequest` (see [Reviews](#reviews)) |
| Merge | `merge_pull_request` | Best-effort payload shape (see [Merge](#merge)) |
| Dependencies | `add_issue_dependency`, `remove_issue_dependency`, `add_pull_request_dependency`, `remove_pull_request_dependency` | Best-effort endpoint shape (see [Dependency links](#dependency-links)) |
| CI | `list_ci_jobs`, `get_ci_job` | Full over REST; **web-UI fallback** when REST is absent (Forgejo 7.0.x), best-effort and credential-gated (see [Continuous integration](#continuous-integration-actions)) |

## Identity

`current_user` calls `GET /user` and maps the response to a portable `User`.
The Forgejo `login` is both the portable `UserId` and the human-facing
`handle`, so reviewer-request logins map directly. Empty `full_name`/`email`
strings (Forgejo's "unset" form) become `None`.

`get_user` calls `GET /users/{login}` and maps `404` to `Ok(None)` via the
shared optional-read helper.

## Repositories

`get_repository_by_path` calls `GET /repos/{owner}/{name}`; `404` maps to
`None`. `get_repository` parses the opaque `RepositoryId` into an
owner/name pair and delegates to the path lookup, so the two paths share one
mapper.

`list_repositories` calls `GET /user/repos` through the shared pagination
helper. Forgejo's listing order is not contractual, so the requested
`RepositorySortField` (`Path`, `CreatedAt`, `UpdatedAt`) is applied
client-side after mapping, with an owner-then-name then id tie-break for
determinism; the default (no sort) is owner-then-name.

`create_repository` chooses the endpoint by owner: `POST /user/repos` when
the input owner equals the client's own handle (read once via
`current_user`), else `POST /org/{owner}/repos`. The request body carries
`name`, `default_branch`, and `description` (omitted when empty). A `409`
maps to `AlreadyExists`; `422` maps to `InvalidRequest` through the shared
status mapper. The created record is re-fetched by path so the returned
value goes through the read mapping (the create response is otherwise
sparse and not guaranteed to match the read shape); a missing read maps to
`Backend`.

**Limitations.** Repository creation under an arbitrary owner assumes the
caller has rights to that org's `POST /org/{owner}/repos`; the backend does
not introspect or pre-validate org membership. Forgejo's permissions errors
flow through the shared status mapper unchanged.

## Labels

`list_labels` calls `GET /repos/{owner}/{repo}/labels` through the shared
pagination helper and sorts by name then id, exposing each as a prefixed
opaque `LabelId` (`forgejo:{owner}/{repo}:label:{id}`).

`upsert_label` matches by name: it lists the repository's labels, and if a
label with the input name exists it sends `PATCH
/repos/{owner}/{repo}/labels/{id}`; otherwise it sends `POST
/repos/{owner}/{repo}/labels`. The request body carries `name`, and
`color`/`description` when present. The provider's returned label is mapped
and returned directly (no re-fetch).

## Issues

Forgejo serves issues and pull requests through the **same** issue endpoints, so
every issue read path excludes pull requests: a row carrying a non-null
`pull_request` marker is never returned as an `Issue`.

`list_issues` calls `GET /repos/{owner}/{repo}/issues?state=...&type=issues`,
adding `labels=<comma-separated>` when the query carries labels. The portable
state filter maps `Open → open`, `Closed → closed`, `None → all`. Normal runner
scan queries pass an explicit `open` state or labelled `closed` state; `all` is
reserved for callers that ask for the portable default. Belt and suspenders
against the provider ignoring `type=issues`, PR-as-issue rows are also dropped
client-side. Forgejo 7.0.x has no reliable provider-side exact body-substring
search, so `body_contains` is applied client-side after the state/label provider
query; `Some("")` is the same as no body filter. No `q`/`body` provider query
parameter is sent for this fallback, so the state and label parameters remain the
narrowest provider-side filter. Author and assignee are filtered client-side
after mapping too. When `details.dependencies=true` (the default), matching
issues are enriched with their dependency links; summary list queries set
`details.dependencies=false` and skip the dependency N+1, returning empty
dependency vectors. Results are then sorted by the requested sort field, then by
number, then by id for determinism.

`get_issue`/`get_issue_by_number` call `GET /issues/{number}`; a `404` **or** a
PR-as-issue row maps to `Ok(None)`. The match is enriched with dependency links.

`create_issue` posts `{ title, body, assignees }` to `POST /issues`, then applies
labels through the shared issue label helper (skipped when empty), and re-fetches
the issue so the returned value reflects the applied metadata.

`update_issue` re-reads the current issue (a PR-as-issue row maps to `NotFound`),
performs the optional conditional-write check (see Optimistic concurrency),
patches `title`/`body`/`state` through `PATCH /issues/{number}` when any is set,
applies label/assignee changes through the shared helpers (preserving the
`set_labels` → removals → additions order), and re-fetches. A missing issue maps
to `NotFound`. Label and assignee sequencing is identical to pull requests (see
below), since both run on the issue endpoints.

## Issue comments

`list_issue_comments` and `add_issue_comment` use `GET`/`POST
/issues/{number}/comments` through the shared item-comment helpers, the same code
path pull-request comments use. Comments map and sort identically (by creation
time, then id).

## Pull requests

`list_pull_requests` uses two provider paths:

- without labels, it calls `GET /repos/{owner}/{repo}/pulls?state=...`; and
- with labels, it first calls
  `GET /repos/{owner}/{repo}/issues?type=pulls&state=...&labels=...` to discover
  candidate pull-request numbers, then fetches `GET /pulls/{number}` only for
  those candidates.

The portable state filter maps `Open → open`, both `Closed` and `Merged →
closed`, and `None → all`; `Merged` is then re-checked client-side after the PR
detail fetch. The labelled path deliberately does not fall back to
`/pulls?state=all`, so provider-shape failures are explicit backend errors rather
than silent broad scans. Forgejo 7.0.x has no reliable provider-side exact
body-substring search, so `body_contains` is applied client-side after the
existing state/label provider query; `Some("")` is the same as no body filter.
No `q`/`body` provider query parameter is sent for this fallback. Labelled
correlation lookups therefore keep the shape
`/issues?type=pulls&state=<open|closed>&labels=...` followed by exact
`/pulls/{number}` reads, never `/pulls?state=all`. Author and assignee are
filtered client-side after mapping too. When `details.dependencies=true` (the
default), matching pull requests are enriched with dependency links; summary list
queries set `details.dependencies=false` and skip that dependency N+1. Results
sort by the requested sort field, then by number, then by id for determinism.

`get_pull_request`/`get_pull_request_by_number` call `GET /pulls/{number}`; a
`404` maps to `Ok(None)`.

`create_pull_request` posts `{ title, head, base, body }` to `POST /pulls`,
then applies labels and assignees through the issue endpoints (see below) and
re-fetches the pull request so the returned value reflects the applied
metadata. Empty label/assignee sets skip the corresponding requests.

`update_pull_request` re-reads the current pull request, performs the optional
conditional-write check (below), patches `title`/`body`/`state` through
`PATCH /pulls/{number}` when any is set, applies label/assignee changes, and
re-fetches. A missing pull request maps to `NotFound`.

### Label and assignee sequencing (shared with issues)

Pull requests are issues on Forgejo, so label and assignee updates use the
issue endpoints and the same helper issues use, keeping one sequencing
implementation:

- labels: Forgejo's issue-label endpoints key on the **numeric label id**, not
  the name — a name array is rejected with `422 cannot unmarshal … into int64`.
  When any of set/add/remove is non-empty the backend issues **one**
  `GET /repos/{owner}/{repo}/labels` read to resolve names to ids, then:
  `set_labels` replaces the full set with `PUT /issues/{number}/labels` carrying
  ids; removals are deleted by id with `DELETE /issues/{number}/labels/{id}` (a
  missing label is a no-op); additions are appended by id with `POST
  /issues/{number}/labels`. A name with no matching repository label is skipped
  (the workflow upserts its labels before applying them), so the empty-input case
  issues no label read at all.
- assignees: the new set is computed as `current − remove + add` (sorted,
  deduplicated) and written with `PATCH /issues/{number}`; a no-op update skips
  the request.

## Pull-request comments

Forgejo PR comments are issue comments. `list_pull_request_comments` and
`add_pull_request_comment` use `GET`/`POST /issues/{number}/comments` and map
exactly like issue comments. A missing item maps to `NotFound`.

## Requested reviewers

`request_pull_request_reviewers` posts `{ reviewers: [login...],
team_reviewers: [] }` to `POST /pulls/{number}/requested_reviewers`, then
re-fetches the pull request. User ids are logins, so they map directly. The
call is idempotent: Forgejo rejects re-requesting an already-requested
reviewer, so on a non-success response the backend re-fetches and returns the
current pull request when the desired reviewers are already present. A `404`
maps to `NotFound`.

## Reviews

`list_pull_request_reviews` calls `GET /pulls/{number}/reviews` and maps
provider states to portable decisions, accepting both submit-event and stored
state spellings: `APPROVED`/`approve → Approved`,
`REQUEST_CHANGES`/`changes_requested → ChangesRequested`,
`COMMENT`/`commented → Commented`, `PENDING → Pending`. Only review-request
events (`REQUEST_REVIEW`) and unknown states are excluded. **Dismissed and stale
verdicts are kept**: Forgejo auto-dismisses a reviewer's prior verdict when they
resubmit (e.g. an approval after a changes-requested review), and dropping it
would erase that changes-requested event from history and diverge from the
reference backends, which have no dismissal concept and return every verdict. The
portable aggregate (`PullRequestReviewStatus`) already resolves superseding by
taking the latest verdict per reviewer, so keeping dismissed/stale verdicts does
not affect the gate. Reviews sort by submission time, then id. Forgejo timestamp
precision can collapse adjacent approval and merge events into the same second;
callers should treat equal timestamps as inconclusive ordering rather than a
strict inversion.

`submit_pull_request_review` submits in **one call**:
`POST /pulls/{number}/reviews` with `{ event, body }` where `event` is
`APPROVED`, `REQUEST_CHANGES`, or `COMMENT`. The author is the backend client's
current user. `ReviewDecision::Pending` returns `InvalidRequest`: the historical
two-step pending flow drops the body for `APPROVED`, so it is deliberately not
used. If the provider's response echo is too sparse to map, the backend returns
a review carrying the decision it submitted.

## Merge

`merge_pull_request` posts to `POST /pulls/{number}/merge` with a best-effort
Gitea/Forgejo payload: `{ Do, MergeTitleField?, MergeMessageField? }`, where
`Do` maps `MergeCommit → merge`, `Squash → squash`, `Rebase → rebase`. These
field names are not verified against a live instance and may need refinement.
The merge `POST` returns no usable body, so the backend re-fetches the pull
request for the merge commit SHA, merger, and timestamp; the returned
`MergeRecord` reports the method that was requested. A success with no merge
record maps to `Backend`. `404` maps to `NotFound`; `405`/`409`/`412`/`422`
(already merged, not mergeable, failed precondition) map to `Conflict`.

## Dependency links

Issue and pull-request dependency links use Forgejo's (Gitea's) issue
dependency endpoints under
`/repos/{owner}/{repo}/issues/{number}/dependencies`. Pull requests share the
issue-number namespace on Forgejo, so the **pull-request** dependency methods
use the **same** endpoint with the pull-request number as the source — a
provider-specific adaptation. The endpoint shapes are isolated in
`dependencies.rs` so live refinement only edits one module.

- read: `GET /issues/{number}/dependencies` returns the items the source is
  blocked by; the backend maps each to its repository-scoped `ItemNumber` and
  returns them sorted and deduplicated. `Issue::dependencies` and
  `PullRequest::dependencies` are populated this way during `get` reads, list
  reads whose `details.dependencies` flag is true, and by the dependency-link
  methods' returned source. A `404` on the read (no dependencies, or an
  unsupported provider endpoint) yields an **empty** list — a safe, documented
  behavior. Summary list calls set `details.dependencies=false` and skip this
  enrichment entirely; dependency-gated workflow paths reload exact artifacts and
  dependency targets when they need the links.
- add: `POST /issues/{number}/dependencies` with a
  `{ "index": <target-number>, "owner": <owner>, "repo": <name> }` body (Gitea's
  `IssueMeta`). Forgejo 7.0.12 resolves the target by `(owner, repo, index)`, not
  `index` alone: omitting `owner`/`repo` resolves against an empty repository and
  returns `404 IsErrRepoNotExist` (verified live). The target shares the source's
  repository, so both come from the source coordinates.
- remove: `DELETE /issues/{number}/dependencies` with the same body shape.

Semantics match the portable contract:

- The source must exist; a missing source returns `NotFound`.
- Add resolves the target through the issue endpoint (which serves both issues
  and pull requests); a missing target returns `NotFound`.
- Add is idempotent: when the target is already a dependency the source is
  returned unchanged without a write. Remove of an absent link is a successful
  no-op once the source exists and does not require the target to exist.
- Add/remove never silently claim success on an unsupported endpoint: a `404`
  from the add/remove request (after the target was verified to exist) maps to
  `InvalidRequest`, in contrast to the read path's empty-list behavior.

The returned source artifact is re-fetched after a changed link, which advances
its `Version` through the validator cache (Forgejo bumps the artifact's
`updated_at` on a dependency change). The mutation paths
(`create`/`update`/`merge`/reviewer requests) deliberately do **not** re-read
dependencies, so their returned artifacts may report empty dependencies; read
the item through `get` or a full-detail list for an enriched dependency view.

## Optimistic concurrency (best-effort)

Forgejo exposes no confirmed conditional-write contract, so compare-and-swap is
best-effort. The backend derives a portable `Version` from a per-artifact
validator — an `ETag` header when present, otherwise the weak `updated_at`
timestamp — captured on every read through a shared `VersionCache`. A
`Version` is only meaningful when the read that issued it and the conditional
write that consumes it go through the same backend instance, which is how the
workflow layer's `LeaseManager` uses it.

When `update_issue` or `update_pull_request` is called with `expected_version`,
the backend re-reads the artifact and re-resolves its validator before mutating;
a changed validator returns `Conflict` and writes nothing (the read happens, but
no PATCH/PUT/DELETE/POST is emitted). When no validator is available,
`CasMode::Strict` refuses the conditional write (`InvalidRequest`) while
`CasMode::BestEffort` proceeds with a documented weak read-before-write. A
successful write re-fetches the artifact and re-observes its validator, so the
returned `Version` reflects the post-write state. Both updates use one stable
cache key per artifact (the formatted issue/pull-request id).

Residual races: read-modify-write is not atomic, and `updated_at` has
one-second granularity. Until live validation confirms provider-supported
conditional requests, lease-race safety on this backend is best-effort and
mode-dependent.

## Continuous integration (Actions)

The backend adapts Forgejo Actions **runs** and **tasks** to the portable
`CiJob` model through two inherent methods on `ForgejoForge`:

- `list_ci_jobs(repo_id, query)` lists runs (`GET /repos/{repo}/actions/runs?limit=200`),
  matches them to the `CiJobQuery` target, expands matched runs to tasks
  (`GET /repos/{repo}/actions/tasks?limit=200`), groups tasks into attempts, and
  maps the latest attempt's tasks to jobs.
- `get_ci_job(id)` takes no repository parameter: it decodes the repository
  coordinate out of the `CiJobId`, finds the run, expands its latest attempt, and
  returns the matching job (or `None`).

Both Actions endpoints return their array wrapped in a `workflow_runs` field; the
backend tolerantly decodes that wrapper (and a bare array or `null`). Timestamps
may arrive as RFC3339 strings (`*_at`) or unix-epoch integers (`created`/
`updated`), so they decode through a flexible serde helper into
`chrono::DateTime<Utc>`. Runner scans call `list_ci_jobs` only after
queue/transition signal-needs analysis proves that the candidate can inspect CI,
so non-CI role scans do not enter the REST or web-UI CI paths. Setting
`TEMPER_FORGEJO_CI_DIAGNOSTICS=1` logs each web-UI CI fallback read without
secrets for live/e2e diagnostics.

### Run matching

A `CiJobQuery { pull_request_id, commit_sha, status, sort }` resolves to a match
target. When `pull_request_id` is set, the backend first fetches the pull request
(`GET /repos/{repo}/pulls/{number}`) to learn its head SHA and head ref. A run
matches when any of the following hold (checked in order):

1. **PR ref**: `prettyref` or `head_branch` equals `#<pr>`.
2. **Head SHA**: the run head/commit SHA matches the query commit or PR head SHA
   (full equality or a shared prefix of at least 7 characters).
3. **Event payload number**: the parsed `event_payload` pull-request number
   matches.
4. **Event payload head SHA**: the parsed `event_payload` PR head SHA matches.
5. **PR head branch**: the run `head_branch` equals the PR head ref — only for
   pull-request events (`event` starting with `pull_request`).

Matched runs are sorted newest first by creation time, then update time, then
repo-stable run index (`index_in_repo`, then `run_number`, then `id`).

### Attempts and jobs

A run's tasks are filtered by `run_number` (matched against the run's repo-stable
index), sorted by monotonic task id, and grouped into attempts: a repeated task
name starts a new attempt. Only the latest attempt's tasks become jobs, enumerated
by index. Each `CiJob` carries the backend-owned encoded id
(`forgejo:{owner}/{repo}:actions:{run}:{job_index}:{task_id}`, see the Identifier
encoding table), the owning `repo_id`, the resolved pull request (from the query
or derived from the run), a commit SHA (task/run SHA, then PR head SHA, then query
commit), the job UI URL when constructible, and timestamps from provider fields
(`*_at` then bare fields) with run-level fallbacks; `created_at` falls back to the
unix epoch and `completed_at` is set when the job is `Completed`.

Status mapping: `success`/`failure`/`cancelled`/`skipped`/`timeout` (and
`neutral`) map to `Completed` with the matching conclusion; `running`/
`in_progress` map to `Running`; `waiting`/`queued`/`requested`/`blocked`/`pending`
(and anything unknown) map to `Queued`.

### Web-UI read fallback (ADR 0019)

Forgejo 7.0.x does **not** serve the Actions runs/tasks REST endpoints (they
404). When the REST runs endpoint is absent — or returns successfully but lists
no matching run for the target — and **web-UI credentials are configured**
(`ForgejoConfig::web_ui`, see Configuration), the backend reads CI status through
the **password-authenticated web UI**, mirroring the production `forgejo-tools.ts`
pattern. This path lives in `src/ci_ui.rs` (+ `src/ci_ui_parse.rs`), the only
modules that know the web-UI shapes.

The fallback:

1. **Logs in** via CSRF: `GET /user/login` to capture the `_csrf` hidden input
   and initial cookies, then a form-encoded `POST /user/login`
   (`user_name`/`password`/`remember=on`/`_csrf`). Success is a redirect off
   `/user/login`; a `200`, a redirect back to `/user/login`, or a `401`/`403`
   means the session failed/expired, and the client re-logs in once on a bounce.
2. **Discovers runs** by scraping `…/actions/runs/{id}` links from
   `GET /{owner}/{repo}/actions` (cookie auth).
3. **Reads status** from the live-view JSON
   `POST /{owner}/{repo}/actions/runs/{run}/jobs/{job}` with the cookie jar, an
   `X-Csrf-Token: <_csrf cookie>` header, and a `{"logCursors":[]}` body. The
   response's `state.run.jobs[].status` map to `CiJob` through the **same** status
   mapper the REST path uses. A run is kept when its commit short-SHA matches the
   target head SHA **or** its commit branch equals the target head ref (or
   unconditionally when the target carries no filter). The **branch** match is
   load-bearing for a fail→pass pull request: the failing and fixed verdicts live
   on different SHAs of the same head branch, so a SHA-only filter would drop the
   failing verdict. **Cancelled** runs (superseded by a newer push) are dropped —
   they carry no verdict — so the kept stream matches the reference CI producer's
   clean pass/fail shape.

These requests bypass the API helper: **no `/api/v1` prefix, cookie auth instead
of the token, and form-encoded bodies**, issued through the raw `HttpClient`
seam. The reqwest client used by the backend disables auto-redirect following
(`redirect::Policy::none()`) so this path observes the raw `3xx`: a successful
login is a `303` to `/`, which a redirect-following client would silently chase
to a `200` homepage and read as a failed login. The path is **best-effort and
version-sensitive**: it tolerates missing fields (unknown/absent status →
`Queued`), derives each job's `created_at`/`updated_at` from the run id (the UI
exposes no per-job timestamp; run ids are monotonic, so this gives a stable
older-run-before-newer ordering that `CiStatus::from_jobs` and the
`ci_fails_then_passes` ordering rely on), and reuses the run id as the encoded
`task_id` (the UI has no stable task id). The credentials are redacted in `Debug`
and never appear in errors or logs.

Without web-UI credentials and with no REST endpoint, the backend keeps the hard
`ForgeError::Backend` below — a missing verdict is never fabricated as pass/fail.

### Errors and limits

If the Actions endpoints are unavailable (`403`/`404`) and the web-UI fallback is
not usable (no credentials), the backend returns a `ForgeError::Backend` rather
than silently reporting CI as passed or failed, so the runner's merge gate stays
closed when CI cannot be read. Job results are sorted by the requested
`CiJobSort` (a `field` of name/created-at/updated-at with an asc/desc
`direction`), falling back to name then job id, mirroring the reference backends.
The same sort and the query's status filter apply to web-UI-read jobs too.

**CI log fetching remains outside the portable backend.** `Forge::list_ci_jobs`
needs only structured run/task status; fetching build logs (which the TypeScript
tooling and the web-UI live view also expose) is intentionally not part of the
`Forge` interface.

## Error mapping

Transport failures (DNS, TLS, connection resets) become `ForgeError::Backend`.
Non-success HTTP statuses are classified centrally (`src/error.rs`):

| Status | `ForgeError` |
| --- | --- |
| `404` | `NotFound` (lookups that tolerate absence convert it to `Ok(None)` first) |
| `409`, `412` | `Conflict` (already-exists / failed precondition / stale conditional write) |
| `400`, `422` | `InvalidRequest` |
| `401`, `403`, `5xx`, other | `Backend` |

A handful of operations override the default before delegating to the shared
mapper: `create_repository` maps `409 → AlreadyExists`; `merge_pull_request`
maps `405`/`409`/`412`/`422 → Conflict`; dependency add/remove map a post-verify
`404 → InvalidRequest`. Error messages append a trimmed response-body snippet
(capped at 200 characters) for diagnosis. **The access token is never included
in an error message or log** — only the bearer header carries it, and that is
built per request and never formatted into errors.

## Unsupported and provider-specific behavior

- **Pending reviews.** `submit_pull_request_review` has no safe one-call pending
  submit (the historical two-step flow drops the body for an `APPROVED` event),
  so `ReviewDecision::Pending` returns `InvalidRequest`. This mirrors the
  reference TypeScript tooling, which rejects the same case.
- **Pull requests are issues.** Forgejo serves issues and pull requests through
  the same endpoints. Comments, labels, assignees, and dependency links for a
  pull request all use the `/issues/{number}` namespace; the issue read paths
  exclude PR-as-issue rows so a pull request is never returned as an `Issue`.
- **Dependency endpoint shared by both kinds.** Pull-request dependency links
  reuse the issue dependency endpoint with the pull-request number as the
  source, since the two share a number namespace.
- **Merge method is not echoed.** Forgejo's pull-request JSON does not expose the
  method used to merge, so a merged pull request read back maps its
  `MergeRecord::method` to `MergeCommit` as a documented default;
  `merge_pull_request` reports the method that was actually requested.
- **No provider-specific surface.** The backend exposes only the portable
  `Forge` trait; Forgejo-only concepts (teams, branch protection, review
  policy, raw build logs) are intentionally not surfaced.
- **Version cache is per-process.** Optimistic-concurrency `Version`s are only
  comparable within one backend instance (see below); they are not durable or
  shared across processes.
