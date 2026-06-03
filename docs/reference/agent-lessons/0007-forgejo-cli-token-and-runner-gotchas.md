# Lesson 0007: Forgejo 7.0.x CLI token + runner registration gotchas

## Tags

`forgejo`, `ci`, `testing`, `tooling`

## Trigger

Phase 1b of the Forgejo e2e effort. The host-mode `forgejo-runner` smoke test
first 403'd on every REST call, then needed care to get a runner registration
token. Both are non-obvious 7.0.x behaviors the later phases (2, 2b, 3, 3b, 4)
will hit when they provision identity and read CI.

## What went wrong

1. **`admin user create --access-token` mints a *scopeless* token.** On Forgejo
   7.0.x the resulting token has no scopes, so the first REST call fails with
   `403 "token does not have at least one of required scope(s): [write:user]"`.
   The `--access-token` flag has no scope option.
2. **`forgejo actions generate-runner-token` needs the web server running.** It
   makes an internal API call to `http://localhost:<HTTP_PORT>/api/internal/...`;
   run against a config whose server is down (or `HTTP_PORT = 0`) it fails with
   `connection refused`. Against the live `ForgejoServer` it works.

## Steering for future agents

- To mint a usable token from the CLI, do it in **two steps**: create the user,
  then `forgejo admin user generate-access-token --username <u> --scopes all
  --raw` (the `--raw` form prints only the 40-char token). Pick narrower scopes
  than `all` for production paths; `all` is fine for throwaway test admins.
- Get a runner registration token via the CLI **only against a running server**
  (`ForgejoServer::run_cli(&["actions", "generate-runner-token"])`), or via
  `GET /api/v1/admin/runners/registration-token` with an admin token. The
  `ForgejoRunner` fixture uses the CLI path so it needs no admin token.
- Register host-mode runners with `--labels host:host` and write a workflow that
  declares `runs-on: host`; no Docker is involved.

## Where this is now documented

- `crates/temper-testing/tests/forgejo_runner.rs` (`create_admin_token`) and
  `crates/temper-forgejo-fixture/src/runner.rs` (`registration_token`).
- `docs/how-to/run-forgejo-multiprocess-e2e.md` (Runner smoke test section).
- `plans/forgejo-e2e/findings-phase-0b.md` (the original runner spike).
