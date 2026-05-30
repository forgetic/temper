# ADR 0019: Read Forgejo CI status through the password-authenticated web UI

## Status

Accepted.

## Context

The workflow engine reads CI through `Forge::list_ci_jobs`/`get_ci_job`. The
Forgejo backend implements those against the Actions REST endpoints
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

## Decision

When the Actions REST endpoints are unavailable, the Forgejo backend reads CI
status through the **password/cookie web UI**, isolated in `src/ci_ui.rs` (plus
`src/ci_ui_parse.rs` for the version-sensitive HTML/header parsing). This mirrors
`forgejo-tools.ts` and is the only code that knows the web-UI shapes.

### Endpoint contract (verified on Forgejo 7.0.12)

- **Login.** `GET /user/login` → extract the hidden `<input name="_csrf">`.
  `POST /user/login` form-encoded (`user_name`, `password`, `remember=on`,
  `_csrf`) with the cookie jar from the GET → on success redirects **off**
  `/user/login`; the jar then holds `i_like_gitea`, `gitea_incredible`, `_csrf`.
  A `200` (form re-rendered), a redirect back to `/user/login`, or a `401`/`403`
  is a failed/expired session; the client re-logs in once on a login bounce.
- **Run discovery.** `GET /{owner}/{repo}/actions` (HTML, cookie auth) lists
  `…/actions/runs/{id}` links; the ids are scraped from the page.
- **Run/job status.** `POST /{owner}/{repo}/actions/runs/{run}/jobs/{job}` with
  the cookie jar, an `X-Csrf-Token: <_csrf cookie>` header, and a
  `{"logCursors":[]}` body returns JSON whose `state.run.status` and
  `state.run.jobs[].status` (∈ `success|failure|running|waiting|…`) map to the
  portable `CiJob` status/conclusion the same way the REST mapper does.
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

`list_ci_jobs`/`get_ci_job` prefer REST so a newer server keeps the richer path:

1. Try `GET …/actions/runs`. A `403`/`404` (REST absent, as on 7.0.x) → fall
   back to the web UI.
2. REST works but lists **no** matching run for the target → also fall back (a
   real run may exist that REST does not surface).
3. With web-UI credentials, the fallback logs in, discovers run ids, reads each
   run's live-view JSON, matches by commit short-SHA when the target carries one,
   and maps jobs to `CiJob`.
4. **Without** web-UI credentials and with no REST endpoint, the existing hard
   `ForgeError::Backend` stands — CI status that cannot be read is never
   fabricated as passed or failed.

### Best-effort, version-sensitive

The web-UI HTML/JSON shapes are not a stable contract. The read path tolerates
missing fields (an unknown/absent status maps to `Queued`), exposes no per-job
timestamps (jobs share the unix epoch), and reuses the run id as the encoded
`task_id` because the UI exposes no stable task id. Any hard failure surfaces a
portable `ForgeError`; the path never guesses a pass/fail verdict.

## Consequences

- CI is readable on Forgejo 7.0.x despite the missing REST endpoints, using the
  same technique the production tooling relies on; the real `forgejo-runner`
  remains the producer.
- `harness-forge` stays backend-agnostic: this is entirely inside
  `harness-forge-forgejo`, behind the unchanged `list_ci_jobs`/`get_ci_job`
  signatures. The web-UI requests bypass `build_request` (no `/api/v1` prefix, no
  token header, cookie auth, form bodies) through the raw `HttpClient` seam.
- A new credential requirement: CI reads on a REST-less server need the web-UI
  password; everything else needs only the token. This is documented in
  `docs/reference/forgejo-backend.md`.
- Offline contract tests cover the login handshake (CSRF extraction, cookie
  storage, re-login on bounce), the live-view JSON → `CiJob` mapping, and the
  REST-first/UI-fallback decision. An `#[ignore]`d, `HARNESS_FORGEJO_E2E=1`-gated
  e2e test reads a real runner-produced `Failure` verdict through the web UI.

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
