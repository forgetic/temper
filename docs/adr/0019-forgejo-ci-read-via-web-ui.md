# ADR 0019: Read Forgejo CI status through the password-authenticated web UI

## Status

Accepted.

## Context

The workflow engine reads CI through `Forge::list_ci_jobs`,
`list_ci_jobs_with_presence`, and `get_ci_job`. The Forgejo backend implements
those against the Actions REST endpoints
(`GET /repos/{owner}/{repo}/actions/runs` + `/actions/tasks`).

**Forgejo 7.0.12 — the version this e2e effort targets — does not serve those
REST endpoints.** Every variant returns `404 page not found`, and the OpenAPI
spec lists only `actions/secrets` and `actions/runners/registration-token` under
`/actions`. The run/task **list** REST endpoints were added in a later
Gitea/Forgejo line. A real `forgejo-runner` (Phase 1b) produces genuine CI runs,
but on 7.0.12 there is no REST surface to read their per-job status from.

The spike (`findings-phase-0b.md` §"Phase 0c") confirmed the workaround the
production pi tool already uses (`forgejo-tools.ts`): the
**password-authenticated web UI** exposes both run discovery and a structured
**live-view JSON** that carries per-run and per-job status. The same technique
works on 7.0.12 where the REST endpoints 404.

The pinned live fixture now runs Forgejo **15.0.3**. That release serves the
Actions REST endpoints, but the web-UI adapter remains the compatibility path
when REST discovery is unavailable. Its live-view route is not identical to
7.0.12: the old unqualified POST resolves to attempt zero and returns `500`,
while the attempt-qualified route and its HTML page are healthy. The ignored
contract test forces only REST run discovery to `404` so this otherwise dormant
adapter is verified against a real runner-produced job.

## Decision

When the Actions REST endpoints are unavailable, the Forgejo backend reads CI
status through the **password/cookie web UI**, isolated in `src/ci_ui.rs` (plus
`src/ci_ui_parse.rs` for the version-sensitive HTML/header parsing). This mirrors
`forgejo-tools.ts` and is the only code that knows the web-UI shapes.

### Endpoint contract (verified on Forgejo 7.0.12 and 15.0.3)

- **Login.** On 7.0.12, `GET /user/login` supplies both the `_csrf` cookie and a
  hidden `<input name="_csrf">`; the form POST includes that field. On 15.0.3,
  the GET supplies neither, so `POST /user/login` sends only form-encoded
  `user_name`, `password`, and `remember=on`. Success redirects **off**
  `/user/login`; 15.0.3 returns `303 Location: /` and establishes `persistent`
  and `session` cookies (plus the `lang` cookie established during login).
  Unlike 7.0.12's `i_like_gitea`, `gitea_incredible`, and `_csrf` jar, 15.0.3
  sends neither an `_csrf` cookie nor `X-Csrf-Token`. A `200` (form re-rendered),
  a redirect back to `/user/login`, or a `401`/`403` is a failed/expired
  session; the client re-logs in once on a login bounce. The adapter keeps both
  the form CSRF field and header optional, preserving both versions.
- **Run discovery.** `GET /{owner}/{repo}/actions` (HTML, cookie auth) lists
  `…/actions/runs/{id}` links; the ids are scraped from the page.
- **Run/job status on 15.0.3.**
  `POST /{owner}/{repo}/actions/runs/{run}/jobs/{job}/attempt/1` with the cookie
  jar and, on 7.0.12, an `X-Csrf-Token: <_csrf cookie>` header, plus a
  `{"logCursors":[]}` body, returns `200` JSON. Forgejo 15.0.3 authenticates
  this request with its `persistent` and `session` cookies and has no CSRF
  header. The route's `{job}` remains the zero-based page coordinate (`0` for
  the fixture's `build` job), even though `state.run.jobs[0].id` is `1`. The
  JSON retains the 7.0.x shape:
  `state.run.status`, `state.run.jobs[].{name,status}`,
  `state.run.commit.{shortSHA,branch.name}`, and `logs`. The statuses (∈
  `success|failure|running|waiting|…`) map to portable `CiJob`
  status/conclusion the same way as REST.
- **Attempt page and compatibility boundary.** A cookie-authenticated
  `GET` of that same `/jobs/{job}/attempt/1` route returns healthy `200` HTML
  and embeds the same initial state. On 15.0.3 the old unqualified
  `POST …/jobs/{job}` returns `500` because it selects nonexistent attempt zero.
  The adapter therefore tries the qualified route first and falls back to the
  7.0.x unqualified route only when the qualified route returns `404`. The DTO
  remains shared because the successful JSON shape did not change.
- Logs (`…/jobs/{job}/logs`, `text/plain`) are diagnostics only and out of scope
  for the portable `Forge` trait.

### Auth split and configuration

The token still drives every REST operation. The web-UI read additionally needs
a **username + password** (web-UI login is password-based), carried in the
optional `ForgejoConfig::web_ui` (`WebUiCredentials`), settable via
`with_web_ui_credentials` or the `FORGEJO_USERNAME`/`FORGEJO_PASSWORD`
environment variables. The credentials are redacted in `Debug` and are never
formatted into errors or logs, matching the existing token-redaction guarantee.

### REST-first, web-UI fallback

CI listing and `get_ci_job` prefer REST so a newer server keeps the richer path:

1. Try `GET …/actions/runs`. A `403`/`404` (REST absent, as on 7.0.x, or
   otherwise unavailable) → fall back to the web UI. Forgejo 15.0.3 normally
   serves this endpoint; its live contract injects only this `404` to exercise
   the adapter without replacing any web-UI response.
2. REST works but lists **no** matching run for the target → also fall back (a
   real run may exist that REST does not surface).
3. With web-UI credentials, the fallback logs in, discovers run ids, reads each
   run's live-view JSON, matches by commit short-SHA when the target carries one,
   and maps jobs to `CiJob`.
4. **Without** web-UI credentials and with no REST endpoint, the existing hard
   `ForgeError::Backend` stands — CI status that cannot be read is never
   fabricated as passed or failed.

### Session recovery and per-run outcomes

Login and Actions-page discovery remain hard operations: if either fails, there
is no authenticated, ordered run sequence on which to base a result. Login-page
redirects and `401`/`403` responses retain the existing bounded authentication
recovery. In addition, a live-view HTTP `500` clears the cookie jar by performing
a fresh login, rebuilds the cookie and optional CSRF headers, and retries that
same route exactly once. This is deliberately not a general `5xx` retry loop.

Each live-view request then has one of three typed outcomes: `found`, `missing`,
or `unreadable`. `unreadable` contains only the repository coordinate, run and
job coordinates, final HTTP status, and retry count; response bodies, cookies,
CSRF values, and credentials are discarded. A transport or persistent
authentication failure remains a hard error rather than a per-run outcome.

### Newest-first trust boundary and pending behavior

List reads inspect at most 20 run ids in Actions-page order, newest first, and
continue to the end of that window after an unreadable run. The ordering is also
the trust rule:

- readable target jobs observed before the first unreadable run are retained;
  therefore a newer matching success remains valid when only older history is
  unreadable;
- after the first unreadable run, matching jobs from older runs are inspected
  but omitted, because the unknown newer run could supersede them;
- if the unreadable boundary precedes the first readable target match, or every
  recent run is unreadable, the list returns `Ok([])` rather than stale evidence
  or a repository-wide error.

An empty job result means pending to the workflow gate. A readable matching run
also sets `CiJobListing::matching_ci_present` even when it has not materialized
jobs yet; missing-CI recovery therefore does not confuse ordinary runner queueing
with run absence. The existing CI read cache never reuses empty or otherwise
non-terminal job results, so a degraded empty result is fetched again on the
next call. A returned terminal result for the explicit head keeps the existing
terminal cache behavior. Explicit SHA ownership, PR filtering, cancelled-run
exclusion, and query filtering/sorting are unchanged.

Each degraded list read emits exactly one warning. It represents the newest
unreadable run and includes repository, run, job, final status, retry count,
total unreadable count, the number of additional unreadable diagnostics omitted,
and an outcome of `continued` or `pending` in both structured fields and the
human message. No response body or authentication material is logged.

An exact `get_ci_job` read has no ordered sequence from which to select safer
evidence. Its persistent unreadable outcome is therefore converted back to a
detailed, secret-free `ForgeError::Backend` after the same one-shot recovery.

### Best-effort, version-sensitive

The web-UI HTML/JSON shapes are not a stable contract. The read path tolerates
missing fields (an unknown/absent status maps to `Queued`), exposes no per-job
timestamps (jobs share the unix epoch), and reuses the run id as the encoded
`task_id` because the UI exposes no stable task id. Infrastructure-wide
failures remain portable `ForgeError`s. Per-run HTTP failures follow the
newest-first trust boundary above and never guess a pass/fail verdict.

## Consequences

- CI is readable on Forgejo 7.0.x despite the missing REST endpoints, using the
  same technique the production tooling relies on; the real `forgejo-runner`
  remains the producer.
- `temper-forge` stays backend-agnostic: this is entirely inside
  `temper-forge-forgejo`, behind the portable CI listing and `get_ci_job`
  operations. The web-UI requests bypass `build_request` (no `/api/v1` prefix,
  no token header, cookie auth, form bodies) through the raw `HttpClient` seam.
- A new credential requirement: CI reads on a REST-less server need the web-UI
  password; everything else needs only the token. This is documented in
  `docs/reference/forgejo-backend.md`.
- Offline contract tests cover the login handshake (CSRF extraction, cookie
  storage, re-login on bounce), one-shot `500` reauthentication, newest-first
  unreadable-run boundaries, retryable degraded empty reads, bounded secret-free
  warnings, both live-view route shapes, the live-view JSON → `CiJob` mapping,
  and the REST-first/UI-fallback decision. An ignored local-Forgejo test uses a
  delegating client to force only REST run discovery to `404`, then locks the
  15.0.3 login, redirect, Actions page, attempt-qualified POST, cookie/CSRF,
  JSON, healthy attempt-page, job coordinate, and terminal real-runner verdict
  contracts without rendering credentials.

## Alternatives considered

- **Read the commit-status API** (`/commits/{sha}/status`), which the runner also
  populates. Rejected as the primary path: it is the *simplest* read but carries
  less structure than the live-view JSON, and the web UI is what production uses.
  It remains a possible cheap fallback if ever useful.
- **Require a newer Forgejo that serves the Actions REST endpoints.** Rejected:
  the deployment target is 7.0.12, and the REST-first/fallback design already
  keeps the richer path for newer servers without forcing an upgrade.
- **Surface raw build logs through `Forge`.** Rejected: the portable trait needs
  only structured status; logs stay a backend diagnostic, out of the interface.
