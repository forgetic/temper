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
the deterministic fakes; webhook/`ChangeHint` triggering stays `PollLoop`-only.

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
the access token**: each role needs its own user + token, and `current_user` is
whatever the token resolves to. Provisioning therefore creates a real user (with
a known password) and mints a token per role. So one provisioned login can serve
all three needs — REST token, PR assignee `UserId`, and web-UI CI login — the
role users are given `id == handle`.

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

## What remains on the "swap to real" list

- **Agents** — still the deterministic behavior-only fakes. Real LLM agents
  (Set B: `pi_agent_rust` + DeepSeek) plug into the same `Agent`/`RoleTools`
  boundary.
- **Webhook/`ChangeHint` triggering** — still `PollLoop`-only; the ADR 0009
  accelerator is future latency work, not correctness.
