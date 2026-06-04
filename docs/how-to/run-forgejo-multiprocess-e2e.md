# Run the Forgejo end-to-end fixture

> **Status: complete.** The full five-scenario multi-process test runs against a
> real Forgejo server **plus a real host-mode `forgejo-runner` producing genuine
> CI**. This is the **same** rehearsal as the filesystem
> [run-multiprocess-e2e.md](run-multiprocess-e2e.md), with the **backend and CI
> swapped to real**. Temper's fixture workers use deterministic fake agents;
> real LLM decisions live in Smith's process-responder e2e. The suite carries the
> same webhook trigger shape as `examples/reference-delivery`; separate long-poll
> wakeup regressions cover focused single- and multi-repo wake behavior. The design rationale
> (topology, real CI, per-token identity, webhook wakeups) lives in
> [forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md) and
> [ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md).

This fixture runs a **real Forgejo** server locally so the reference-delivery
workflow can be exercised against the `temper-forge-forgejo` backend instead of
the in-memory/filesystem backends. Everything here is `#[ignore]`d, so a plain
`cargo test` never opens a socket. No opt-in environment variable is required,
but the pinned Forgejo binaries must already be cached under `.cache/forgejo/`
or provided through `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY`.

Tests declare their required initial Forgejo state as JSON. The first run for a
new state hash boots a fresh Forgejo, provisions that state, shuts it down
cleanly, and saves the data tree plus fixture metadata under
`.cache/forgejo/states/`. Each test execution then copies that cached tree to
`/tmp` and starts a new Forgejo process against the copy. Runtime Forgejo and
`forgejo-runner` processes are still killed on drop for fast teardown; only the
one-time cache publisher uses clean shutdown so the SQLite/git tree is safe to
reuse.

## One-line command (the full test)

```sh
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
```

This starts one Forgejo server from the declared cached state **and** one
host-mode `forgejo-runner`, then converges all five reference-delivery scenarios
in predeclared per-scenario repos across real OS processes. It spawns real
processes and executes CI **on this host**, so run it only where that is
acceptable. The first run for a cache miss still pays provisioning cost; warmed
runs copy the state from `.cache` to `/tmp`. The per-phase smoke tests below
build up to it. See
[The multi-process test](#the-multi-process-test-phase-5) for detail.
If the binary cache is missing, first run:

```sh
cargo test -p temper-forgejo-fixture --test cache -- --ignored
```

## Real LLM process proof

The command above runs deterministic **fake** agents. Temper no longer contains
real in-process LLM fixtures or `--agents real`; provider/model behavior is
Smith-owned. To validate real Forgejo + real LLM + Temper's process adapter, run
Smith's ignored process-boundary e2e from the Smith checkout. It drives
Temper's `WorkflowRoleDecisionProcessAgent` against a throwaway Forgejo and
opens a PR through `RoleTools`:

```sh
cd ~/src/rust/smith
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p smith-temper-agent-cli --test forgejo_workflow_role_e2e -- \
  --ignored --test-threads=1
```

Run Smith's documented provider/auth preflight before enabling that gate.

### CPU note (sustained-CPU incident + mitigation)

The throwaway **forgejo web server** (a Go program), not the workers, can drive
the host to **2+ cores of *sustained* CPU for minutes** under this multi-process
workload. Brief git/CI spikes are normal; the steady runaway is the concern.
Temper caps `GOMAXPROCS` on the spawned server and runner (default `2`, override
with `TEMPER_FORGEJO_GOMAXPROCS`, or set it empty to opt out) so the Go runtime
cannot exceed ~2 cores — measured peak dropped from ~222% to ~112% with every
scenario still converging. Note `taskset -cp` alone is insufficient: it re-pins
only the main thread, while Go keeps goroutines on every core. If a run is
**force-killed** (SIGKILL), the Rust `Drop` guards do not run — clean up orphans
with `pkill -f forgejo` / `pkill -f temper-testing-worker` and
`rm -rf /tmp/temper-forgejo-*`.

## Smoke test (Phase 1)

```sh
cargo test -p temper-testing --test forgejo_server -- --ignored
```

This declares an empty state, restores it to `/tmp`, polls `/api/v1/version` to
readiness, and kills the process plus removes the runtime data dir on drop.

## Runner smoke test (Phase 1b)

```sh
cargo test -p temper-testing --test forgejo_runner -- --ignored
```

CI is **real**: this boots the server plus a host-mode `forgejo-runner`
(`--labels host:host`, **no containers**), provisions a repo whose
`.forgejo/workflows/ci.yml` deliberately fails (`run: exit 1`), and polls the
head commit's status API until the real runner reports `state: "failure"`. Both
the server and the runner daemon are killed and their temp dirs removed on drop.

The server config enables Actions (`[actions] ENABLED = true`). Provisioning here
uses an admin token minted via the server CLI (`admin user create` then
`admin user generate-access-token --scopes all --raw` — a plain `--access-token`
yields a scopeless token that 403s on 7.0.x). Reading CI via the password/web-UI
live-view JSON is Phase 3b; commit status is the cheap confirmation here.

The runner spawns real OS processes and executes jobs **on this host**, so run it
only where that is acceptable.

## Provisioning + identity (Phase 2)

```sh
cargo test -p temper-testing --test forgejo_provision -- --ignored
```

`temper_testing::forgejo_server::start_cached_provisioned_server()` declares the
reference-delivery state, restores a per-test Forgejo from the matching cache,
and returns a `Provisioned { admin_token, owner, name, repository, roles }`. On a
cache miss the initializer runs the same provisioning sequence against a fresh
server before publishing the cache. It is the real-backend analogue of the
filesystem `provision` step; because Forgejo identity **is the token** (not a
free `as_user` relabel), it creates a real user and token per workflow role. In
order it:

1. **Admin bootstrap** — creates a non-reserved admin (`e2eadmin`; `admin` is
   reserved) via the server CLI and mints an `all`-scoped admin token
   (`generate-access-token --raw`; a plain `--access-token` is scopeless on
   7.0.x).
2. **Org** — `POST /api/v1/orgs` for the repo owner (`acme`), idempotent.
3. **Per-role identity** — for each `runner_config()` role binding (architect,
   engineer, reviewer, owner, human): create the user with a **known password**,
   add it to the org **Owners** team (org write), and mint a token via the user's
   **own basic-auth** (admin-on-behalf token creation 404s on 7.0.x). The map is
   keyed by `RoleId`; each entry carries `{ user, token, password }`. The
   password is the Phase 3b web-UI CI-read credential.
4. **Repository** — `POST /api/v1/orgs/{owner}/repos` with **`auto_init:true`** so
   `main` exists for PRs.
5. **Labels** — upserts every label the compiled workflow declares **through the
   async `ForgejoForge` backend** (mirrors the filesystem `upsert_labels`). One
   neutral grey color is supplied because 7.0.12 requires a non-empty color.
6. **CI** — `PATCH has_actions:true`, then commits `.forgejo/workflows/ci.yml`
   (base64 contents API) so the push schedules an Actions run.

Tokens and passwords are **never logged**; `RoleIdentity`'s `Debug` redacts them
and provisioning errors carry only a status + body snippet.

### The CI fail→pass mechanism

The committed workflow is host-mode (`runs-on: host`) and its `build` job reads
the message of `GITHUB_SHA` through Forgejo's commit API. A PR head **without**
the `[ci-pass]` marker fails the job; the engineer's **fix commit** carries the
marker, so the re-run on the new head SHA passes. Because a Forgejo commit status
is keyed by SHA, the fail and the pass live on two different head SHAs — exactly
the two verdicts the `ci_fails_then_passes` scenario asserts. Steps are trivial
(no toolchains and no checkout). Creating the head branch + fix commit before
opening the PR is Phase 2b.

Provisioning is async (`#[tokio::test]`) because the label upsert goes through
the async backend. The server is booted on a blocking thread (`spawn_blocking`)
so its blocking-reqwest readiness poll does not trip Tokio's "drop a runtime in
an async context" guard.

## PR-prep: making a head branch real before opening a PR (Phase 2b)

```sh
cargo test -p temper-testing --test forgejo_pr_prep -- --ignored
```

On the filesystem/memory backends the fake engineer opens a PR with a head
branch (`fake/pr-for-code-{N}`) that is never a real git ref — nothing checks. On
**real Forgejo**, `create_pull_request` against a non-existent head fails with
`404 "The target couldn't be found."`. So a PR cannot be opened until its head
branch exists with a commit that differs from base.

`temper_testing::forgejo_server::prepare_pull_request_head(base_url, token,
owner, name, &input)` closes that gap. It is a **Forgejo-only** step a worker runs
**before** `open_pull_request`, not a change to the backend-agnostic `Forge`
trait. Given the same `CreatePullRequest` the agent already built, it:

1. creates the head branch (`input.source.branch`) off the base branch
   (`input.target.branch`) — `POST …/branches`; and
2. commits one trivial file unique to the head (`.temper-pr-prep/<head>.txt`) so
   the head diverges from base with a non-empty diff — `POST …/contents/{path}`.

It is **idempotent**: re-creating the branch (`409/422`) and re-committing the
prep file (`422`) are tolerated as "already prepped", so a re-attempted worker
tick does not error. The filesystem backend never calls it and keeps working
unchanged (the `tests/multiprocess.rs` rehearsal is untouched).

The ignored test proves the full contract: without prep `create_pull_request` 404s;
after prep it succeeds and the PR becomes `mergeable` (Forgejo computes
mergeability asynchronously, so the test polls); and re-running prep is a no-op.

## The multi-process test (Phase 5)

```sh
cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
```

This is the Forgejo twin of `tests/multiprocess.rs`: the **same**
one-process-per-part rehearsal, but against a real Forgejo + a real host-mode
`forgejo-runner`. The ignored test is one serial scenario suite: it declares one
state containing the shared admin, per-role identity/token set, all five primary
scenario repos, and the cross-repo target repo. Each test run starts a new
Forgejo process from a `/tmp` copy of that cached tree, starts a new runner, then
registers dynamic webhooks for the repos against one shared production-shaped
`/forgejo/webhook` trigger. Each scenario spawns a fresh
`temper-testing-worker` fleet `--backend forgejo --clock wall` once per
role-with-an-agent plus one mechanical worker (**no** `--kind ci` — the real
runner is the CI producer). Workers use authenticated Unix wake sockets and a
`120000` ms non-CI poll backstop; all non-CI handoffs must converge by webhook
wake before that backstop. Because the pinned Forgejo 7.0.x fixture does not
emit Actions-completion repo webhooks, only CI-reading role workers use a 1s
status-poll fallback for CI verdict transitions: owner in every scenario, plus
engineer in `ci_fails_then_passes` so it can observe the failed run and push the
recovery commit. The driver polls the scenario's **exact** assert closure to
observe convergence, then stops via its unique `--stop-file` sentinel and asserts
each child exited 0.

Scenario isolation is by repository, stop file, wake socket, and log directory;
only the Forgejo server, runner, trigger, admin, and role users are shared within
the one test process. The cached tree itself is never mutated directly: each run
uses a fresh `/tmp` copy, so a panic drops the active worker fleet and then the
runner/server handles without corrupting future starts.

Secrets travel by env only: each role worker gets its token via
`TEMPER_FORGEJO_TOKEN`, plus `TEMPER_FORGEJO_USERNAME`/`PASSWORD` for the
web-UI CI read, never on argv.

The only per-topology differences from the filesystem test are the worker flags
(`--backend forgejo`, `--base-url`, `--clock wall`, wake socket flags, and the
Forgejo-only `--ci-sentinel present|deferred` telling the engineer whether the PR
head passes CI from the start or fails first and is fixed by a follow-up commit).
The seed/assert closures are reused verbatim, so both topologies are checked
against the same end state. CI is gated on a **commit-message marker**
(`[ci-pass]`) the engineer's commit carries — not a checked-out file — because
the host-mode runner has no `actions/checkout` offline.

### Expected timing and scan diagnostics

On a warmed local checkout, expect one shared setup line (state copy + server
startup, <1s runner startup) and one timing line per scenario. A cache miss also
prints the one-time provisioning cost before the warmed path is available. Recent
pre-cache runs finished in roughly high-80s to mid-90s total, with worker
convergence and real CI dominating; warmed runs should reduce setup time. Each
successful scenario also prints a worker scan summary (`ticks`,
summed `scanned_repositories`, `ci_read_log_lines`, and last scanned paths); the
suite enables `TEMPER_FORGEJO_CI_DIAGNOSTICS=1` so web-UI CI fallback reads are
counted.
If times regress, first check that wake ticks are present, poll-trigger ticks are
limited to the narrow CI status fallback roles, and multi-repo wake scans remain
narrowed except for the mechanical worker's intentional broad recovery scan. The
shared five-scenario fixture sets `TEMPER_WAKE_DEBOUNCE_MS=50` for its worker
children; the production default remains 500ms for more conservative
webhook-burst coalescing. Timeout
panics include the same summaries, worker log tails, runner log tail, and
per-repo CI diagnostics.

## Long-poll webhook wakeup regressions

Single repo:

```sh
cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored --test-threads=1
```

Multi repo, one fixed worker set:

```sh
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
```

These wakeup regressions use the same throwaway Forgejo + real `forgejo-runner`,
register the production `/forgejo/webhook` trigger, and launch fake-agent
Forgejo workers with authenticated Unix wake sockets and `--poll-ms 120000`.
The workers default to a conservative audit cadence of `--audit-ms 600000` (`0`
disables audit ticks; for mechanical workers this is the explicit deep-audit
path), so the immediate webhook assertions exercise hint-narrowed wake scans
rather than the poll/audit backstop. The throwaway server config must allow
loopback webhook targets (`[webhook]`
`ALLOWED_HOST_LIST = 127.0.0.1,localhost`); if hooks register but no trigger
request arrives, check that setting before debugging signatures or wake sockets.
The multi-repo variants declare a cached two-repo state, register webhooks for
both repos, pass both `--repo` values to one role/mechanical worker set, and require
either one issue in each repo or one cross-repo fan-out intake to converge in
less than the poll interval.

On timeout the panic prints the trigger URL (trigger logs are on test stderr),
repo-specific stalled assertion text, worker log paths/tails, runner log tail,
and per-repo CI diagnostics. Worker tick logs include `scanned_repositories` and
`scanned_repository_paths`; the multi-repo webhook regression uses these counters
to prove source-repo role-worker wakes can narrow to one configured repo even
when provider startup noise also produces broad mixed-repo batches. Mechanical
wake scans intentionally visit all configured repositories so cross-repo recovery
can observe dependency-source repositories, but each per-repo reconciliation is
bounded rather than a deep audit. Run these serially for the same CPU/isolation
reasons as the multi-process suite.

## Running it in CI

This repository has **no in-repo CI config**, so there is no committed job to
edit; instead, configure your CI system with a **dedicated job**, separate from
the default hermetic suite. The default job runs plain `cargo test` (the Forgejo
tests are `#[ignore]`d, so they never fire there). The dedicated job:

1. **Provides the two pinned binaries.** Either run the cache helper first
   (needs network to Codeberg / code.forgejo.org) or pre-stage them and point
   `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY` at the absolute
   paths (the offline/sandboxed path — see [Environment knobs](#environment-knobs)).
   Cache `.cache/forgejo/` across runs to avoid re-downloading.

   ```sh
   cargo test -p temper-forgejo-fixture --test cache -- --ignored
   ```
2. **Runs on a host that allows host-mode jobs.** The `forgejo-runner` registers
   with `--labels host:host` and executes workflow steps **directly on the CI
   host** (no containers), so the job must run on a runner that permits spawning
   child processes and binding an ephemeral localhost port — not a locked-down
   container-only executor.
3. **Invokes exactly:**

   ```sh
   cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
   ```

   `--test-threads=1` is still recommended because the suite owns a real
   server, runner, and multiple worker processes while it runs.

The job should not block the default pipeline gate (it is slower and
host-dependent); treat it as a periodic / on-demand real-backend check. The
runner smoke test (`--test forgejo_runner`) is a cheaper pre-flight that confirms
the host can boot a server + runner before the full suite.

## The pinned binaries

The ignored cache helper downloads the pinned Forgejo **and** `forgejo-runner`
binaries into `.cache/forgejo/` (gitignored) and verifies each SHA-256 before
use. The regular ignored Forgejo tests require the cache (or explicit binary
overrides) and fail with a cache-population hint when it is absent.

| Binary | Version | SHA-256 | Source |
| --- | --- | --- | --- |
| Forgejo server | `7.0.12` | `ecd25535250aeb8073fdef1a0c9e92f288de1c0cdde24c95a3b61ead6bc9cf7c` | `https://codeberg.org/forgejo/forgejo/releases/download/v7.0.12/forgejo-7.0.12-linux-amd64` |
| `forgejo-runner` | `3.5.1` | `e2f36aa8149a0e883b5713398aa185c88a827fc0527d5cd2e2b05b88c9ba0b36` | `https://code.forgejo.org/forgejo/runner/releases/download/v3.5.1/forgejo-runner-3.5.1-linux-amd64` |

Both are `linux-amd64` (the server is SQLite, statically linked).

### Environment knobs

The server uses the `TEMPER_FORGEJO_*` namespace; the runner mirrors it under
`TEMPER_FORGEJO_RUNNER_*`.

| Variable | Effect |
| --- | --- |
| `TEMPER_FORGEJO_BINARY` | absolute path to a pre-downloaded **server** binary; skips the cache lookup and checksum (operator vouches for it) |
| `TEMPER_FORGEJO_VERSION` | override the pinned server version in the default download URL |
| `TEMPER_FORGEJO_URL` | override the server download URL (checked only when paired with `TEMPER_FORGEJO_SHA256`) |
| `TEMPER_FORGEJO_SHA256` | override the expected server checksum |
| `TEMPER_FORGEJO_RUNNER_BINARY` | absolute path to a pre-downloaded **runner** binary; skips the cache lookup and checksum |
| `TEMPER_FORGEJO_RUNNER_VERSION` | override the pinned runner version in the default download URL |
| `TEMPER_FORGEJO_RUNNER_URL` | override the runner download URL (checked only when paired with `TEMPER_FORGEJO_RUNNER_SHA256`) |
| `TEMPER_FORGEJO_RUNNER_SHA256` | override the expected runner checksum |

State snapshots live under `.cache/forgejo/states/<hash>/`. The hash includes a
fixture cache-format version, the Forgejo binary version/override, and the test's
JSON state description. If a state cache becomes stale or you want to force
re-provisioning, remove `.cache/forgejo/states/`; the binary cache in
`.cache/forgejo/forgejo-*` and `.cache/forgejo/forgejo-runner-*` can remain.
The metadata sidecar stores raw tokens for the throwaway cached state, so keep
`.cache/` local and gitignored.

A mismatched checksum fails loudly; the partial download is never published to
the cache path. A sandboxed/offline machine should point the two `*_BINARY`
overrides at pre-downloaded binaries.

## Why blocking HTTP in Temper

The cache helper downloads binaries and the fixture polls readiness with a
**blocking** `reqwest` client. The async `temper-forge-forgejo` backend needs a
Tokio reactor, so driving it against the live server happens in the e2e tests
(under an async runtime), not in the lifecycle code itself.
