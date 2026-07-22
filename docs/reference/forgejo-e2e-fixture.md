# Forgejo e2e fixture reference

This page records the implementation contracts for the ignored Forgejo e2e
fixtures. For commands, read
[run-daemon-e2e.md](../how-to/run-daemon-e2e.md).
For design rationale, read
[forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md).

## Crate boundary

Generic Forgejo fixture internals live in the sibling `ai/bench` repository's
`bench-forgejo` crate: process lifecycle, pinned binary download/cache handling,
state-cache mechanics, and host-mode runner support. Temper-specific
provisioning remains in `temper-testing::forgejo_server` and related Temper
crates; that module re-exports the generic fixture types under Temper's existing
public path and layers on reference-delivery org, role, label, workflow, and PR
preparation helpers.

## Process and cache model

Each test execution copies a cached Forgejo state tree to `/tmp`, starts a fresh
server against the copy, and kills runtime processes on drop. Cache publishers
use clean shutdown and atomic publish steps so SQLite/git state is reusable.
Shared binary and state caches are protected by process-safe locks; libtest
default parallelism is correctness-safe.

Startup resolves binaries in this order: explicit environment override, cached
`.cache/forgejo/` file, then pinned download with SHA-256 verification. State
snapshots live under `.cache/forgejo/states/<hash>/`; the hash includes fixture
cache-format version, Forgejo binary version/override, and the JSON state
description. The metadata sidecar stores throwaway raw tokens, so keep `.cache/`
local and gitignored.

## Pinned binaries

The pinned Forgejo server and `forgejo-runner` asset constants are owned by
`bench-forgejo`. Run bench's fixture command (for example, `bench forgejo
download`) or the bench crate's own tests (for example, `cargo test -p
bench-forgejo ...` from the bench checkout) when you need to pre-stage or verify
generic fixture downloads outside Temper.

Both assets are `linux-amd64`; the server is SQLite and statically linked.
Partial downloads are never published. Offline machines should point binary
overrides at pre-downloaded files.

## Environment knobs

`BENCH_FORGEJO_*` is the canonical namespace for generic fixture binary/cache
settings. `bench-forgejo` still honors the old `TEMPER_FORGEJO_*` fixture
variables as deprecated legacy aliases so existing Temper scripts keep working.

| Canonical variable | Legacy alias | Effect |
| --- | --- | --- |
| `BENCH_FORGEJO_BINARY` | `TEMPER_FORGEJO_BINARY` | Absolute path to a pre-downloaded server binary; skips cache lookup and checksum. |
| `BENCH_FORGEJO_VERSION` | `TEMPER_FORGEJO_VERSION` | Override the pinned server version in the default URL. |
| `BENCH_FORGEJO_URL` | `TEMPER_FORGEJO_URL` | Override the server download URL when paired with the checksum override. |
| `BENCH_FORGEJO_SHA256` | `TEMPER_FORGEJO_SHA256` | Override the expected server checksum. |
| `BENCH_FORGEJO_RUNNER_BINARY` | `TEMPER_FORGEJO_RUNNER_BINARY` | Absolute path to a pre-downloaded runner binary; skips cache lookup and checksum. |
| `BENCH_FORGEJO_RUNNER_VERSION` | `TEMPER_FORGEJO_RUNNER_VERSION` | Override the pinned runner version in the default URL. |
| `BENCH_FORGEJO_RUNNER_URL` | `TEMPER_FORGEJO_RUNNER_URL` | Override the runner URL when paired with the runner checksum override. |
| `BENCH_FORGEJO_RUNNER_SHA256` | `TEMPER_FORGEJO_RUNNER_SHA256` | Override the expected runner checksum. |
| `BENCH_FORGEJO_GOMAXPROCS` | `TEMPER_FORGEJO_GOMAXPROCS` | `GOMAXPROCS` for spawned server/runner processes; default `2`, empty opts out. |

Remove `.cache/forgejo/states/` to force state reprovisioning while keeping the
binary cache.

## Provisioning sequence

`start_cached_provisioned_server()` declares reference-delivery state, restores
or publishes the matching cache, and returns a `Provisioned` bundle with admin
token, repository, and per-role identities. On cache miss it:

1. creates a non-reserved admin (`e2eadmin`) via the Forgejo CLI and mints an
   `all`-scoped token with `generate-access-token --raw`;
2. creates the owner org;
3. for each role binding, creates a real user with a known password, adds it to
   the org Owners team, and mints a token through that user's own basic auth;
4. creates the repository with `auto_init:true` so `main` exists;
5. upserts compiled workflow labels through the async `ForgejoForge` backend;
6. enables Actions and commits `.forgejo/workflows/ci.yml`.

Tokens and passwords are redacted from debug output and travel to workers by
environment, never argv.

## CI pass/fail mechanism

The committed workflow runs on `host` and reads the commit message for
`GITHUB_SHA` through Forgejo's commit API. A PR head without `[ci-pass]` fails;
the engineer's fix commit includes `[ci-pass]`, producing a passing run on a new
head SHA. This avoids `actions/checkout` and lets fail-then-pass scenarios assert
two verdicts on two SHAs of the same head branch.

Forgejo 7.0.x does not emit Actions-completion repository webhooks. The daemon
fixture therefore sets `ci_poll_cadence_secs = 1` while retaining deliberately
long 600-second `poll_cadence_secs` and `mechanical_cadence_secs` values. The
short dedicated monitor emits exact PR CI hints: terminal red evaluates
`pr_ci_failed` for the engineer, and terminal green runs targeted mechanical
landing. Convergence before either broad fallback proves the dedicated path.

`ci_poll_cadence_secs` bounds webhook-less red-repair and green-landing
detection; `poll_cadence_secs` remains the full correctness/liveness backstop.
A mechanical cadence alone does not discover red engineer work. The CI
aggregate is latest-per-name for the current head, so `ci_failed` requires every
latest job to be terminal; a failed job mixed with queued or running work stays
pending.

## PR head preparation

Real Forgejo rejects `create_pull_request` when the head branch does not exist.
`prepare_pull_request_head` is a Forgejo-only worker step that runs before
`open_pull_request`: it creates the head branch from base and commits a trivial
unique file under `.temper-pr-prep/` so the branch has a non-empty diff.
Recreating the branch or prep file is tolerated, making the step idempotent.

## Diagnostics and quirks

Scenario diagnostics identify the dedicated `ci_poll` trigger and print each
PR head plus its CI jobs. Timeout panics also include daemon and worker log tails,
runner status/log tail, and the stalled assertion. The red→green capstone counts
`pr_ci_failed` worker assignments and requires exactly one.

The server config must allow loopback webhook targets:
`[webhook] ALLOWED_HOST_LIST = 127.0.0.1,localhost`. If hooks register but no
trigger request arrives, check that before debugging signatures or legacy wake
sockets.

Binary resolution and readiness polling use the fixture crate's blocking HTTP
client; backend exercise happens in async tests (on the asupersync engine
runtime) after the server is live.
