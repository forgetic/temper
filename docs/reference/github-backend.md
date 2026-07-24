# GitHub backend

The `temper-forge-github` crate implements the portable
[Forge interface](forge-interface.md) against GitHub's REST API
(`api.github.com`, or a GitHub Enterprise `/api/v3` root). It mirrors the
structure of the [Forgejo backend](forgejo-backend.md): a mockable `HttpClient`
seam, lenient provider DTOs, pure DTO→model mapping, and offline contract tests
that replay canned responses — no test touches the network.

## Configuration

`GitHubConfig` is built programmatically from explicit values — this crate is a
library, not a process boundary, and never reads the environment. The wiring
layer supplies:

| Field | Required | Meaning |
| --- | --- | --- |
| `token` | yes | Personal access token, sent as `Authorization: Bearer …` |
| `base_url` (`with_base_url`) | no | API root; defaults to `https://api.github.com`. GitHub Enterprise uses `https://host/api/v3` |
| `default_owner` / `default_name` (`with_default_repo`) | no | `owner/repo` default repository |

Every request also pins `Accept: application/vnd.github+json`,
`X-GitHub-Api-Version: 2022-11-28`, and a `User-Agent` (GitHub rejects requests
without one). Pagination uses `per_page`/`page`.

## Identifier shapes

Ids are opaque to workflow code; only the backend parses them:

- repository: `github:{owner}/{repo}`
- issue: `github:{owner}/{repo}:issue:{number}`
- pull request: `github:{owner}/{repo}:pull:{number}`
- comment / review / label: `github:{owner}/{repo}:{kind}:{provider_id}`
- CI job: `github:{owner}/{repo}:job:{job_id}` (Actions job ids are
  repository-stable, so no run coordinate is needed)
- user: the GitHub login, unprefixed

## Provider adaptations

- **PR-as-issue rows.** GitHub serves pull requests through the issue
  endpoints and offers no `type` filter, so issue reads drop every row with a
  `pull_request` marker.
- **Merged vs closed.** The `/pulls` list omits the `merged` boolean; the
  mapping falls back to `merged_at` presence. Detail reads use `merged`.
- **Labels by name.** Issue-label endpoints key on label names (and
  auto-create unknown ones), so no name→id resolution read is needed.
- **Reviews.** Submit events are `APPROVE`/`REQUEST_CHANGES`/`COMMENT`;
  `Pending` is rejected (no one-call submit). GitHub's `DISMISSED` state
  replaces the original verdict, so dismissed reviews are dropped from review
  lists (unlike Forgejo, which keeps the flagged verdict).
- **Default branch on create.** GitHub ignores `default_branch` in the create
  payload; when the requested branch differs from the provider's, the backend
  issues a follow-up `PATCH /repos/{owner}/{repo}` (which requires the branch
  to exist) and surfaces failures rather than silently ignoring the request.
- **CI.** `list_ci_jobs` narrows Actions runs provider-side with the
  `head_sha` query parameter (resolved from the pull request's head when a
  PR id is given), then expands each run's latest-attempt jobs.
  `list_ci_jobs_with_presence` preserves a matching run as present even before
  GitHub materializes jobs, and `get_ci_job` reads `/actions/jobs/{job_id}`
  directly. Actions conclusions keep startup failure, action-required,
  stale/interrupted, timeout, cancellation, neutral, skipped, ordinary failure,
  runner loss (when explicitly supplied), and unknown terminalization distinct.
  A machine-readable reason can refine a broad failure into an explicit terminal
  category; arbitrary reason prose is preserved but never reclassified. The
  adapter also retains bounded, control-sanitized raw conclusion/reason evidence
  and opaque run/attempt ids; it never turns an unknown completed result into
  ordinary failure. `retry_ci_attempt` supports only GitHub's documented
  `POST /repos/{owner}/{repo}/actions/runs/{run_id}/rerun` endpoint. It first
  re-reads the exact PR head, run attempt, and latest jobs and compares the
  complete portable fingerprint. A higher `run_attempt` is `already_observed`;
  changed coordinates are rejected. The POST is issued once with no body.
  Endpoint `404`/`410` (including older Enterprise versions) is typed
  `unsupported`, explicit non-success client responses are rejected, and
  transport/`5xx` completion is `uncertain` for later read reconciliation.
  Temper never blindly repeats that uncertain boundary: a newer pending attempt
  reconciles it, while unchanged evidence proceeds to the configured read-only
  diagnostic or actionable park after the bounded grace. There is no commit/ref
  fallback.
- **Optimistic concurrency.** Best-effort, identical to the Forgejo backend:
  a portable `Version` is derived from the response `ETag` (or the weak
  `updated_at` fallback) per artifact, and conditional writes re-read and
  compare before mutating. `CasMode::Strict` refuses writes without a
  captured validator.
- **Errors.** `404`/`410 Gone` → `NotFound`, `409`/`412` → `Conflict`,
  `400`/`422` → `InvalidRequest`, everything else (including `403`
  rate-limiting) → `Backend`. Transient `5xx`s are retried a bounded number
  of times. Repository creation maps GitHub's `422 … name already exists` to
  `AlreadyExists`.

## First-pass limitations

- **No native dependency links.** GitHub's stable REST surface has no issue
  dependency endpoint. Reads report empty `dependencies`; `add_*`/`remove_*`
  dependency operations return `InvalidRequest` instead of silently
  succeeding. A later pass can adopt the sub-issues API once stable.
- **Offline tests only.** The crate currently has hermetic contract tests
  (mock HTTP client with canned responses) plus unit tests; live smoke tests
  against a real GitHub remain to be added in a follow-up pass.
- The backend is not yet wired into the worker/daemon backend selection
  (`temper-testing` worker args expose only filesystem and Forgejo).
