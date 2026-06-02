# Run the Forgejo end-to-end fixture

> **Status: complete.** The full five-scenario multi-process test runs against a
> real Forgejo server **plus a real host-mode `forgejo-runner` producing genuine
> CI**. This is the **same** rehearsal as the filesystem
> [run-multiprocess-e2e.md](run-multiprocess-e2e.md), with the **backend and CI
> swapped to real**. With `--agents real` every role runs a real LLM through
> quarantined `temper-testing` reference-delivery fixtures (ChatGPT OAuth by
> default), while production workers use generic manifest-driven agents. The
> production reference-delivery demo now carries the ADR 0009 webhook trigger;
> the separate long-poll wakeup regression covers that path. The design rationale
> (topology, real CI, per-token identity, webhook wakeups) lives in
> [forgejo-e2e-topology.md](../explanation/forgejo-e2e-topology.md) and
> [ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md).

This fixture runs a **real Forgejo** server locally so the reference-delivery
workflow can be exercised against the `temper-forge-forgejo` backend instead of
the in-memory/filesystem backends. Like the `temper-forge-forgejo` live tests,
everything here is `#[ignore]`d **and** gated behind an environment variable, so
a plain `cargo test` never downloads a binary or opens a socket.

## One-line command (the full test)

```sh
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
```

This boots a Forgejo server **and** a host-mode `forgejo-runner`, provisions, and
converges all five reference-delivery scenarios across real OS processes. It
spawns real processes and executes CI **on this host**, so run it only where that
is acceptable. Budget several minutes. The per-phase smoke tests below build up
to it. See [The multi-process test](#the-multi-process-test-phase-4) for detail.

## Real LLM agents (Phase B2)

The command above runs the deterministic **fake** agents. To run the **same**
scenarios with real agents in every role (`--agents real`), set a second gate.
The end state and the seed/assert closures are identical — only *who decides*
changes. These fixed reference-delivery adapters are test fixtures, not
production `temper-agents` prompt/adaptor surfaces.

Per the cost policy these runs default to **ChatGPT OAuth** (a flat subscription,
not pay-per-token DeepSeek), reading the shared `~/.pi/agent/auth.json`. Log in
once with `pi /login openai-codex`; **no DeepSeek key is required**:

```sh
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored \
  --test-threads=1 happy_path_converges_with_real_agents
```

To opt into Anthropic OAuth instead, log in with `pi /login anthropic` and set
`TEMPER_AGENTS_AUTH=anthropic-oauth` (optionally
`TEMPER_AGENTS_ANTHROPIC_MODEL`):

```sh
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 TEMPER_AGENTS_AUTH=anthropic-oauth \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored \
  --test-threads=1 happy_path_converges_with_real_agents
```

To opt back to DeepSeek (the bring-your-own-key fallback), set
`TEMPER_AGENTS_AUTH=deepseek` and provide the key:

```sh
TEMPER_FORGEJO_E2E=1 TEMPER_FORGEJO_AGENTS=1 TEMPER_AGENTS_AUTH=deepseek \
  TEMPER_DEEPSEEK_API_KEY_PATH="$PWD/.cache/deepseek-api-key" \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored \
  --test-threads=1 happy_path_converges_with_real_agents
```

- It is **double-gated** (both env vars) and makes real, non-deterministic LLM
  calls — never in the default suite. OAuth bearers are resolved fresh per
  decision from `~/.pi/agent/auth.json` (refreshed near expiry); any DeepSeek key
  is read at runtime. None are ever logged; the worker children receive
  credentials via env (or the absolute shared auth file), not argv.
- Run the real-agent tests **serially** (`--test-threads=1`): each boots its own
  real Forgejo, and several at once would multiply the CPU load.
- The original four scenarios converge (happy ~26–29 s, the others ~34–61 s on a 4-core
  host); see `plans/forgejo-e2e/findings-phase-b.md` for cost/latency.

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
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_server -- --ignored
```

This boots a throwaway Forgejo on an ephemeral port against a fresh SQLite data
dir, polls `/api/v1/version` to readiness, and kills the process plus removes the
data dir on drop.

## Runner smoke test (Phase 1b)

```sh
TEMPER_FORGEJO_E2E=1 \
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
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_provision -- --ignored
```

`temper_testing::forgejo_server::provision(&server)` turns a freshly-booted
server into a world the reference-delivery scenarios can run against, and returns
a `Provisioned { admin_token, owner, name, repository, roles }`. It is the
real-backend analogue of the filesystem `provision` step; because Forgejo
identity **is the token** (not a free `as_user` relabel), it creates a real user
and token per workflow role. In order it:

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
6. **CI** — commits `.forgejo/workflows/ci.yml` (base64 contents API) and
   `PATCH has_actions:true`.

Tokens and passwords are **never logged**; `RoleIdentity`'s `Debug` redacts them
and provisioning errors carry only a status + body snippet.

### The CI fail→pass mechanism

The committed workflow is host-mode (`runs-on: host`) and its `build` job runs
`test -f ci-ok`. A PR head **without** the `ci-ok` sentinel file fails the job;
the engineer's **fix commit** adds `ci-ok`, so the re-run on the new head SHA
passes. Because a Forgejo commit status is keyed by SHA, the fail and the pass
live on two different head SHAs — exactly the two verdicts the
`ci_fails_then_passes` scenario asserts. Steps are trivial (`echo`, `test -f`);
no toolchains. Creating the head branch + fix commit before opening the PR is
Phase 2b.

Provisioning is async (`#[tokio::test]`) because the label upsert goes through
the async backend. The server is booted on a blocking thread (`spawn_blocking`)
so its blocking-reqwest readiness poll does not trip Tokio's "drop a runtime in
an async context" guard.

## PR-prep: making a head branch real before opening a PR (Phase 2b)

```sh
TEMPER_FORGEJO_E2E=1 \
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

The gated test proves the full contract: without prep `create_pull_request` 404s;
after prep it succeeds and the PR becomes `mergeable` (Forgejo computes
mergeability asynchronously, so the test polls); and re-running prep is a no-op.

## The multi-process test (Phase 4)

```sh
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
```

This is the Forgejo twin of `tests/multiprocess.rs`: the **same** one-process-per-part
rehearsal, but against a real Forgejo + a real host-mode `forgejo-runner`. For
each of the five reference-delivery scenarios (happy path, changes-requested,
CI-fails-then-passes, dependency-chain, and cross-repo fan-out) it boots a server
+ runner, provisions, seeds via the scenario's **exact** seed closure, spawns the
`temper-testing-worker` binary `--backend forgejo --clock wall` once per
role-with-an-agent plus one mechanical worker (**no** `--kind ci` — the real
runner is the CI producer), polls the scenario's **exact** assert closure to
convergence, then stops via the `--stop-file` sentinel and asserts each child
exited 0. Each scenario boots its own server+runner for isolation, so run it
`--test-threads=1` (five servers at once is heavy). Budget several minutes.

Secrets travel by env only: each role worker gets its token via
`TEMPER_FORGEJO_TOKEN`, plus `TEMPER_FORGEJO_USERNAME`/`PASSWORD` for the
web-UI CI read, never on argv.

The only per-topology differences from the filesystem test are the worker flags
(`--backend forgejo`, `--base-url`, `--clock wall`, and the Forgejo-only
`--ci-sentinel present|deferred` telling the engineer whether the PR head passes
CI from the start or fails first and is fixed by a follow-up commit). The
seed/assert closures are reused verbatim, so both topologies are checked against
the same end state. CI is gated on a **commit-message marker** (`[ci-pass]`) the
engineer's commit carries — not a checked-out file — because the host-mode runner
has no `actions/checkout` offline.

## Long-poll webhook wakeup regressions

Single repo:

```sh
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_webhook_wakeup -- --ignored --test-threads=1
```

Multi repo, one fixed worker set:

```sh
TEMPER_FORGEJO_E2E=1 \
  cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --test-threads=1
```

These wakeup regressions use the same throwaway Forgejo + real `forgejo-runner`,
register the production `/forgejo/webhook` trigger, and launch fake-agent
Forgejo workers with authenticated Unix wake sockets and `--poll-ms 120000`. The
multi-repo variants provision a second repo, register webhooks for both repos,
pass both `--repo` values to one role/mechanical worker set, and require either
one issue in each repo or one cross-repo fan-out intake to converge in less than
the poll interval.

On timeout the panic prints the trigger URL (trigger logs are on test stderr),
repo-specific stalled assertion text, worker log paths/tails, runner log tail,
and per-repo CI diagnostics. Run these serially for the same CPU/isolation
reasons as the multi-process suite.

## Running it in CI

This repository has **no in-repo CI config**, so there is no committed job to
edit; instead, configure your CI system with a **dedicated job**, separate from
the default hermetic suite (which must keep running without this gate). The
default job runs plain `cargo test` (the Forgejo test is `#[ignore]`d **and**
env-gated, so it never fires there). The dedicated job:

1. **Provides the two pinned binaries.** Either let the first run download +
   checksum them into `.cache/forgejo/` (needs network to Codeberg /
   code.forgejo.org), or pre-stage them and point
   `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY` at the absolute
   paths (the offline/sandboxed path — see [Environment knobs](#environment-knobs)).
   Cache `.cache/forgejo/` across runs to avoid re-downloading.
2. **Runs on a host that allows host-mode jobs.** The `forgejo-runner` registers
   with `--labels host:host` and executes workflow steps **directly on the CI
   host** (no containers), so the job must run on a runner that permits spawning
   child processes and binding an ephemeral localhost port — not a locked-down
   container-only executor.
3. **Invokes exactly:**

   ```sh
   TEMPER_FORGEJO_E2E=1 \
     cargo test -p temper-testing --test forgejo_multiprocess -- --ignored --test-threads=1
   ```

   `--test-threads=1` keeps the five per-scenario servers from running at once.

The job should not block the default pipeline gate (it is slower and
host-dependent); treat it as a periodic / on-demand real-backend check. The
runner smoke test (`--test forgejo_runner`) is a cheaper pre-flight that confirms
the host can boot a server + runner before the full suite.

## The pinned binaries

The first gated run downloads the pinned Forgejo **and** `forgejo-runner`
binaries into `.cache/forgejo/` (gitignored) and verifies each SHA-256 before
use; later runs reuse them.

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
| `TEMPER_FORGEJO_E2E=1` | opt in (required); without it the tests no-op |
| `TEMPER_FORGEJO_BINARY` | absolute path to a pre-downloaded **server** binary; skips its download and checksum (operator vouches for it) |
| `TEMPER_FORGEJO_VERSION` | override the pinned server version in the default download URL |
| `TEMPER_FORGEJO_URL` | override the server download URL (checked only when paired with `TEMPER_FORGEJO_SHA256`) |
| `TEMPER_FORGEJO_SHA256` | override the expected server checksum |
| `TEMPER_FORGEJO_RUNNER_BINARY` | absolute path to a pre-downloaded **runner** binary; skips its download and checksum |
| `TEMPER_FORGEJO_RUNNER_VERSION` | override the pinned runner version in the default download URL |
| `TEMPER_FORGEJO_RUNNER_URL` | override the runner download URL (checked only when paired with `TEMPER_FORGEJO_RUNNER_SHA256`) |
| `TEMPER_FORGEJO_RUNNER_SHA256` | override the expected runner checksum |

A mismatched checksum fails loudly; the partial download is never published to
the cache path. A sandboxed/offline machine should point the two `*_BINARY`
overrides at pre-downloaded binaries.

## Why blocking HTTP in Temper

Temper downloads the binary and polls readiness with a **blocking**
`reqwest` client. The async `temper-forge-forgejo` backend needs a Tokio
reactor, so driving it against the live server happens in the multi-process test
(under an async runtime), not in the Phase 1 lifecycle code.
