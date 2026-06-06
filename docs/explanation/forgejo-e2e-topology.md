# Forgejo multi-process e2e: topology and real CI

This explains the durable design of the Forgejo multi-process end-to-end
rehearsal — the **real-backend twin** of the filesystem
[multi-process rehearsal](../how-to/run-multiprocess-e2e.md). It is the design
record that outlives `plans/` (which is gitignored). The how-to to run it is
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
- **Trigger**: one host-local `/forgejo/webhook` receiver per scenario
  (`crates/temper-trigger-forgejo`) is registered against that scenario's
  repositories and sends authenticated Unix-datagram wakes to its workers.
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

## Provisioning boundary

Provisioning is split at the same boundary production operators need. The
reference-delivery binary delegates to `crates/temper-forgejo-provision`, while
fixture support keeps reusable server setup local to `temper-testing`:
`provision_role_identities` creates org + per-role users/tokens once, and
`provision_repository` creates the `auto_init` repo, labels, and CI workflow per
repository. `provision_world(...)` keeps the single-repo convenience path;
`provision(&server)` is the throwaway test wrapper that bootstraps an admin first.

Role logins come from `runner_config().role_bindings`, never a hardcoded list.
`seed_intake_issue(...)` creates the realistic entry issue idempotently, deriving
its queue labels from the compiled workflow. The production
`temper-provision-forgejo` binary reads the admin token from
`TEMPER_FORGEJO_ADMIN_TOKEN` and writes per-role user/token/password secrets to a
`0600` POSIX-sourceable file without printing secrets.

### Provisioning onto an existing, content-bearing repo

The defaults above are built for **throwaway** repos on a dedicated org: they
create the repo, commit a marker CI workflow (`.forgejo/workflows/ci.yml`) plus a
sentinel onto `main`, and add every identity to the org **Owners** team. Two
flags relax that for a **real, pre-existing** target — e.g. running basic-delivery
against `ai/smith` on the shared `ai` org (which also hosts `temper`). Both
default to today's behavior, so throwaway flows are unchanged.

- `--existing-repo` provisions onto a repo that **must already exist**. It checks
  the repo up front (`GET /repos/{owner}/{name}`) and errors clearly if absent
  instead of silently creating a bare repo. It **skips** the marker CI commit and
  the sentinel commit, so the repo's own `.forgejo/workflows/ci.yml` and history
  are never touched; labels, the webhook, and `enable_actions` (all idempotent)
  still apply.
- `--access org-owners|repo-collaborator` selects how identities are granted
  access (default `org-owners`, today's behavior). `repo-collaborator` never
  touches the Owners team; instead it grants each role user **and** the `bot` a
  repo-scoped `write` collaborator permission on the target repo. `write` is
  enough for the bot to merge approved, green PRs and read Actions status over the
  web UI ([ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md)); `admin` is
  intentionally avoided until a concrete need appears.

The intended Smith caller pairs both with `--seed-intake no` (intake issues are
filed separately by the `agent` user):

```sh
TEMPER_FORGEJO_ADMIN_TOKEN=<agent admin token> temper-provision-forgejo \
  --base-url http://127.0.0.1:3000 --owner ai --name smith \
  --existing-repo --access repo-collaborator \
  --workflow ~/.config/smith/workflow.json \
  --webhook-url http://127.0.0.1:<trigger-port>/forgejo/webhook \
  --webhook-secret-file ~/.config/smith/secrets/webhook-secret \
  --seed-intake no \
  --out ~/.config/smith/secrets/roles.env
```

## Real CI: producing and reading

**Producing.** The provisioned repo commits `.forgejo/workflows/ci.yml`
(`runs-on: host`). Because the host runner has no offline `actions/checkout`, the
CI-pass gate keys on a **commit-message marker** (`[ci-pass]`) rather than a
checked-out file. A failing head SHA and later passing fix SHA give the
`ci_fails_then_passes` scenario two real verdicts.

**Reading.** Forgejo 7.0.12 lacks Actions run/task REST endpoints, so
`list_ci_jobs` is REST-first with a **password/web-UI fallback** (ADR 0019): CSRF
login, cookie jar, run discovery from `/{owner}/{repo}/actions`, and per-job
status from live-view JSON. Reads match by head **branch**, drop superseded
cancelled runs, and order jobs by run id.

## Backend hardening this forced

Driving real concurrent workers through a real server surfaced gaps that landed
in `temper-forge-forgejo` behind unchanged `Forge` signatures:

- **Bounded `5xx` write retry** for SQLite contention.
- **No-auto-redirect client** so web-UI `303` login redirects are observable.
- **Dependency add/remove payload carries `owner`/`repo`** because Gitea resolves
  dependency targets by `(owner, repo, index)`.
- **`list_pull_request_reviews` keeps dismissed/stale verdicts**; history is
  preserved and the aggregate still takes the latest per reviewer.

## Scaling shape

The production scan fixes are part of the e2e topology, not test-only shortcuts.
Role workers derive candidate queries from subscribed queues, request summary
issue/PR rows, prune unlabelled closed history, and read CI/review/dependency
signals only after a cheap queue match needs them. Webhook wakeups narrow
immediate role ticks to hinted configured repositories; poll and audit remain
broad backstops.

Forgejo 7.0.x does not surface Actions completion as a repo webhook, so
CI-reading roles use a short 1s status-poll fallback only for CI verdict
transitions: the owner in every scenario, and the engineer in the CI fail→pass
scenario. Scenario-specific cached state removes repeated provisioning. Libtest
default parallelism is correctness-safe; throttle test threads only for host
CPU/I/O capacity.

## Why it stays `#[ignore]`d

It boots real OS processes, executes CI **on the host**, and detects convergence
by wall-clock polling. Like the `temper-forge-forgejo` live test it is
`#[ignore]`d, so default `cargo test` stays hermetic and deterministic.

Ignored startup may download pinned binaries into `.cache/forgejo/` when explicit
overrides and cached files are absent. Binary and state caches are process-safe,
and each test gets unique runtime paths. The in-process scenarios remain the
first-line workflow coverage; this suite covers the real-backend topology.

## Triggering status

Each split scenario is webhook-driven: real Forgejo posts to the production
trigger, the trigger sends authenticated wake datagrams, and fake-agent Forgejo
workers consume them while their normal poll backstop is `120000` ms. The focused
ignored regressions still cover wake acceleration in isolation:
`forgejo_webhook_wakeup` for one repo and `forgejo_multi_repo_webhook` for one
fixed worker set scanning two repos.

Polling remains the correctness backstop; webhook payloads are hints only and
every wake runs a fresh Forge scan. On Forgejo 7.0.x, the suite keeps the short
status-poll fallback limited to CI-reading role workers because CI-completion
repo hooks are not observable.
