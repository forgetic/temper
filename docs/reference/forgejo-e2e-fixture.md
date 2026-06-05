# Forgejo e2e fixture reference

This page records the implementation contracts for the ignored Forgejo e2e
fixtures. For commands, read
[run-forgejo-multiprocess-e2e.md](../how-to/run-forgejo-multiprocess-e2e.md).
For design rationale, read
[forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md).

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

| Binary | Version | SHA-256 | Source |
| --- | --- | --- | --- |
| Forgejo server | `7.0.12` | `ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c` | `https://codeberg.org/forgejo/forgejo/releases/download/v7.0.12/forgejo-7.0.12-linux-amd64` |
| `forgejo-runner` | `3.5.1` | `e2f36aa8149a0e883b5713398aa185c88a827fc0527d5cd2e2b05b88c9ba0b36` | `https://code.forgejo.org/forgejo/runner/releases/download/v3.5.1/forgejo-runner-3.5.1-linux-amd64` |

Both assets are `linux-amd64`; the server is SQLite and statically linked.
Partial downloads are never published. Offline machines should point binary
overrides at pre-downloaded files.

## Environment knobs

| Variable | Effect |
| --- | --- |
| `TEMPER_FORGEJO_BINARY` | Absolute path to a pre-downloaded server binary; skips cache lookup and checksum. |
| `TEMPER_FORGEJO_VERSION` | Override the pinned server version in the default URL. |
| `TEMPER_FORGEJO_URL` | Override the server download URL when paired with `TEMPER_FORGEJO_SHA256`. |
| `TEMPER_FORGEJO_SHA256` | Override the expected server checksum. |
| `TEMPER_FORGEJO_RUNNER_BINARY` | Absolute path to a pre-downloaded runner binary; skips cache lookup and checksum. |
| `TEMPER_FORGEJO_RUNNER_VERSION` | Override the pinned runner version in the default URL. |
| `TEMPER_FORGEJO_RUNNER_URL` | Override the runner URL when paired with `TEMPER_FORGEJO_RUNNER_SHA256`. |
| `TEMPER_FORGEJO_RUNNER_SHA256` | Override the expected runner checksum. |
| `TEMPER_FORGEJO_GOMAXPROCS` | `GOMAXPROCS` for spawned server/runner processes; default `2`, empty opts out. |

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

Forgejo 7.0.x does not emit Actions-completion repo webhooks. CI-reading role
workers therefore use a 1s status-poll fallback for CI verdict transitions: the
owner in every scenario, and the engineer in the fail-then-pass scenario.

## PR head preparation

Real Forgejo rejects `create_pull_request` when the head branch does not exist.
`prepare_pull_request_head` is a Forgejo-only worker step that runs before
`open_pull_request`: it creates the head branch from base and commits a trivial
unique file under `.temper-pr-prep/` so the branch has a non-empty diff.
Recreating the branch or prep file is tolerated, making the step idempotent.

## Diagnostics and quirks

Scenario fixtures set `TEMPER_FORGEJO_CI_DIAGNOSTICS=1` so web-UI CI fallback
reads are counted. Worker summaries include tick counts, scanned repository
counts/paths, and CI read log lines. Timeout panics print worker logs, runner log
tail, trigger URL, stalled assertion text, and per-repo CI diagnostics.

The server config must allow loopback webhook targets:
`[webhook] ALLOWED_HOST_LIST = 127.0.0.1,localhost`. If hooks register but no
trigger request arrives, check that before debugging signatures or wake sockets.

Binary resolution and readiness polling use blocking `reqwest`; backend exercise
happens in async tests after the server is live.
