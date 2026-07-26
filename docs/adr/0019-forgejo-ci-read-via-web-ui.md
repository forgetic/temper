# ADR 0019: Read Forgejo CI status through the password-authenticated web UI

## Status

Superseded by the Forgejo 16 API-only CI contract.

## Context

Forgejo 7 lacked structured Actions run/job APIs, so Temper previously logged
into the Forgejo web UI, scraped repository Actions HTML, and read live-view JSON
with cookies and version-specific CSRF handling. That compatibility adapter also
required a separate `ci_user` password in deployment configuration.

Forgejo 16.0.1 provides the provider-run jobs endpoint:

```text
GET /api/v1/repos/{owner}/{repo}/actions/runs/{run_id}/jobs
```

## Superseding decision

Forgejo 16.0.1 is the minimum supported Forgejo release. Temper now discovers
runs and reads each matched run's jobs exclusively through token-authenticated
JSON APIs. Provider run, job, attempt, and task identifiers form the opaque job
identity. Missing, unauthorized, unsupported, or malformed jobs responses fail
closed; there is no HTML, login, cookie, CSRF, live-view, or repository-wide
tasks fallback.

The old web-UI source, credentials, cache, configuration, worker inputs, fixtures,
and tests were deleted. See the current
[Forgejo backend reference](../reference/forgejo-backend.md) for the API contract.
