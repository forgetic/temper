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
the **backend**, **CI**, and **webhook wake path** real. Agents stay the
deterministic fakes in the main suite; separate gated regressions cover focused
single- and multi-repo wake narrowing.

## Process topology

The split multi-process suite is five ignored tests. Each test owns the live
world for one scenario:

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

- **Server**: one throwaway Forgejo per scenario
  (`temper_testing::forgejo_server`), Actions enabled, fresh SQLite, ephemeral
  port, killed and removed on drop.
- **Runner**: one real host-mode `forgejo-runner` per scenario (`--labels
  host:host`, **no containers**) registered to that server. It is the genuine CI
  producer — there is no fake `--kind ci` worker on Forgejo.
- **Repositories**: the scenario-specific cached state contains only the needed
  repo names. Cross-repo fan-out gets `service-cross-repo-source` and
  `service-cross-repo-target`.
- **Trigger**: one host-local `/forgejo/webhook` receiver per scenario is
  registered against that scenario's repositories and sends authenticated
  Unix-datagram wakes to its workers.
- **Role + mechanical workers**: per scenario, the `temper-testing-worker`
  binary, one OS process per role-with-an-agent plus one mechanical reconciler,
  launched `--backend forgejo --clock wall` with a unique stop file, wake socket,
  and log dir. They coordinate **only** through their scenario server.

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

The provisioning sequence is split at the same boundary production operators
need: org + per-role user/token (`provision_role_identities`) can run once, then
`auto_init` repo + labels + CI workflow (`provision_repository`) can run for each
repo. `provision_world(base_url, admin_token, owner, name, roles, default_branch)`
keeps the old single-repo convenience path, and `provision(&server)` is the
throwaway-server wrapper that bootstraps an admin (CLI) then calls it. Role
logins come from the passed-in binding list (`runner_config().role_bindings`),
never hardcoded. `seed_intake_issue(base_url, token, owner, name)` adds one
realistic intake issue whose labels are **derived from the compiled workflow**
(the entry issue artifact a queue filters on), idempotently. The production
binary exposes this as `temper-provision-forgejo`: it takes
`--base-url/--owner/--name/--out`, reads the admin token from
`TEMPER_FORGEJO_ADMIN_TOKEN` (never argv), and writes the per-role
`{user, token, password}` to a `0600` POSIX-sourceable secrets file
(`TEMPER_FORGEJO_{USER,TOKEN,PASSWORD}_<ROLE>=…`), printing nothing secret.

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
in `temper-forge-forgejo` (all behind the unchanged `Forge` signatures):

- **Bounded `5xx` write retry** — concurrent workers contend on SQLite.
- **No-auto-redirect client** — so the web-UI `303` login redirect is *observed*
  rather than transparently chased to a `200`.
- **Dependency add/remove payload carries `owner`/`repo`** — Gitea resolves the
  dependency target by `(owner, repo, index)`.
- **`list_pull_request_reviews` keeps dismissed/stale verdicts** — history is
  preserved; the aggregate still takes the latest per reviewer.

## Scaling shape

The production scan fixes are part of the e2e topology now, not test-only
shortcuts. Role workers derive candidate queries from subscribed queues, request
summary issue/PR rows, prune unlabelled closed history, and read CI/review/
dependency signals only after a cheap queue match needs them. Webhook wakeups
narrow immediate role ticks to hinted configured repositories; poll and audit
remain broad backstops. The pinned Forgejo 7.0.x fixture does not surface Actions
completion as a repo webhook, so CI-reading roles use a short 1s status-poll
fallback only for CI verdict transitions: the owner in every scenario, and the
engineer in the CI fail→pass scenario that must observe the failed run before
pushing the recovery commit. Scenario-specific cached state removes repeated
provisioning while keeping cleanup stack-owned per test. Libtest can run several
scenario worlds at once by default; add a thread limit only when host CPU or I/O
capacity is the bottleneck. The remaining runtime is mostly real CI convergence.

## Why it stays `#[ignore]`d

It boots real OS processes (server, runner, N workers), executes CI **on the
host**, and detects convergence by wall-clock polling — non-deterministic,
network-bound, and host-mutating. Like the `temper-forge-forgejo` live test it is
`#[ignore]`d, so the default `cargo test` stays hermetic and deterministic. On a
networked machine, ignored Forgejo startup downloads the pinned binaries into the
shared `.cache/forgejo/` cache when explicit binary overrides and cached files
are absent. Those binary and state caches are process-safe, and each test gets
unique runtime copies/paths, so libtest default parallelism is a correctness-safe
mode. Operators may still throttle test threads when CPU or I/O capacity is the
limiting factor. The in-process scenarios remain the first-line coverage for
workflow logic; this covers the real-backend topology.

## Triggering status

Each split Forgejo multi-process scenario is webhook-driven: real Forgejo posts
to the production trigger, the trigger sends authenticated wake datagrams, and
fake-agent Forgejo workers consume them while their normal poll backstop is
`120000` ms. The focused ignored regressions still cover the wake accelerator in
isolation: `forgejo_webhook_wakeup` for one repo, and
`forgejo_multi_repo_webhook` for one fixed worker set scanning two repos. Polling
still stays the correctness backstop; webhook payloads are hints only and every
wake runs the same fresh Forge scan. On Forgejo 7.0.x, CI-completion wakeups are
not observable through repo hooks, so the suite keeps the short status-poll
fallback limited to the CI-reading role workers that need those verdicts.
