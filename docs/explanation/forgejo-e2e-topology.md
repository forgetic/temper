# Forgejo multi-process e2e: topology and real CI

This explains the durable design of the Forgejo multi-process end-to-end
rehearsal — the **real-backend twin** of the filesystem
[multi-process rehearsal](multiprocess-e2e-roadmap.md). It is the design record
that outlives `plans/` (which is gitignored). The how-to to run it is
[run-forgejo-multiprocess-e2e.md](../how-to/run-forgejo-multiprocess-e2e.md);
the CI-read decision is [ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md).

## What changed from the filesystem rehearsal

The filesystem rehearsal proves the *topology* — workers coordinate solely
through the Forge and survive real process boundaries — on fakes. The Forgejo
twin keeps the **exact** scenario seed/assert closures and worker set, but makes
two of the "swap to real" pieces real: the **backend** and **CI**. Agents stay
the deterministic fakes in the main suite; separate gated regressions cover real
webhook/`ChangeHint` triggering, including a two-repo fixed worker set.

## Process topology

For each scenario the test boots an isolated world:

```text
  ┌──────────────────────────────────────────────────────────┐
  │ Forgejo server (SQLite, ephemeral port, kill-on-drop)      │
  │   REST /api/v1  ·  web UI /user/login + /{o}/{r}/actions   │
  └──────────────────────────────────────────────────────────┘
        ▲ REST (per-role token)        ▲ web-UI (password)   ▲ runs CI
        │                              │                     │
  ┌─────┴─────┐  ┌──────────┐  ┌───────┴──────┐      ┌───────┴────────┐
  │ role      │  │ role     │  │ mechanical   │      │ forgejo-runner │
  │ worker ×N │  │ worker   │  │ worker       │      │ (host mode)    │
  └───────────┘  └──────────┘  └──────────────┘      └────────────────┘
```

- **Server**: a throwaway Forgejo (`harness_testing::forgejo_server`), Actions
  enabled, fresh SQLite, ephemeral port, killed and removed on drop.
- **Runner**: a real host-mode `forgejo-runner` (`--labels host:host`, **no
  containers**) registered to the server. It is the genuine CI producer — there
  is no fake `--kind ci` worker on Forgejo.
- **Role + mechanical workers**: the `harness-testing-worker` binary, one OS
  process per role-with-an-agent plus one mechanical reconciler, launched
  `--backend forgejo --clock wall`. They coordinate **only** through the server.

## Identity is per-token

Filesystem identity is a free `as_user(handle)` relabel. Forgejo identity **is
the access token**: each role needs one user + token, and `current_user` is
whatever the token resolves to. In multi-repo deployments that same role token
must have Forge access to every repo in the worker's scan set. Provisioning
therefore creates a real user (with a known password), adds it to the owner org,
and mints a token per role. So one provisioned login can serve all three needs —
REST token, PR assignee `UserId`, and web-UI CI login — the role users are given
`id == handle`.

## Provisioning is server-agnostic and operator-runnable

The provisioning sequence (org → per-role user/token → `auto_init` repo → label
upsert → CI workflow commit) is split so it does not depend on the throwaway
server. `provision_world(base_url, admin_token, owner, name, roles,
default_branch)` is the portable REST/Forge portion; `provision(&server)` is the
throwaway-server wrapper that bootstraps an admin (CLI) then calls it. Role
logins come from the passed-in binding list (`runner_config().role_bindings`),
never hardcoded. `seed_intake_issue(base_url, token, owner, name)` adds one
realistic intake issue whose labels are **derived from the compiled workflow**
(the entry issue artifact a queue filters on), idempotently. The production
binary exposes this as `harness-provision-forgejo`: it takes
`--base-url/--owner/--name/--out`, reads the admin token from
`HARNESS_FORGEJO_ADMIN_TOKEN` (never argv), and writes the per-role
`{user, token, password}` to a `0600` POSIX-sourceable secrets file
(`HARNESS_FORGEJO_{USER,TOKEN,PASSWORD}_<ROLE>=…`), printing nothing secret. The
operator demo calls it once per configured repo so labels, CI, webhooks, and one
seed issue are present in every repo before the single shared worker pool starts.

## Real CI: producing and reading

**Producing.** The provisioned repo commits `.forgejo/workflows/ci.yml`
(`runs-on: host`). The host-mode runner executes it for real. Because the host
runner has no `actions/checkout` offline, the CI-pass gate keys on a
**commit-message marker** (`[ci-pass]`) the engineer's fix commit carries, not a
checked-out sentinel file. A PR head without the marker fails; the fix commit on
a new head SHA passes — two SHAs, two verdicts, which is exactly what the
`ci_fails_then_passes` scenario asserts.

**Reading.** Forgejo 7.0.12 does not serve the Actions run/task REST endpoints,
so `list_ci_jobs` is REST-first with a **password/web-UI fallback** (ADR 0019):
CSRF login, cookie jar, run discovery from `/{owner}/{repo}/actions`, and
per-job status from the live-view JSON. The read matches runs by head **branch**
(not just the current head SHA), drops superseded cancelled runs, and orders
jobs by run id.

## Backend hardening this forced

Driving real concurrent workers through a real server surfaced gaps that landed
in `harness-forge-forgejo` (all behind the unchanged `Forge` signatures):

- **Bounded `5xx` write retry** — concurrent workers contend on SQLite.
- **No-auto-redirect client** — so the web-UI `303` login redirect is *observed*
  rather than transparently chased to a `200`.
- **Dependency add/remove payload carries `owner`/`repo`** — Gitea resolves the
  dependency target by `(owner, repo, index)`.
- **`list_pull_request_reviews` keeps dismissed/stale verdicts** — history is
  preserved; the aggregate still takes the latest per reviewer.

## Why it stays `#[ignore]`d + env-gated

It boots real OS processes (server, runner, N workers), executes CI **on the
host**, and detects convergence by wall-clock polling — non-deterministic,
network-bound, and host-mutating. Like the `harness-forge-forgejo` live tests it
is `#[ignore]`d and gated behind `HARNESS_FORGEJO_E2E=1`, so the default
`cargo test` stays hermetic and deterministic. The in-process scenarios remain
the default coverage for workflow logic; this covers the real-backend topology.

## Triggering status

The four-scenario Forgejo multi-process suite remains poll-driven so it can
compare directly with the filesystem rehearsal. Separate gated long-poll
regressions cover the real webhook accelerator: `forgejo_webhook_wakeup` for one
repo, and `forgejo_multi_repo_webhook` for one fixed worker set scanning two
repos. Real Forgejo posts to the production trigger, the trigger sends
authenticated wake datagrams, fake-agent Forgejo workers consume them, and the
happy path must converge before the `120000` ms poll backstop can fire. Polling
still stays the correctness backstop; webhook payloads are hints only and every
wake runs the same fresh Forge scan.
