# Parallel-safe Forgejo tests — implementation plan

This plan makes the ignored/local Forgejo test suites safe to run with Rust's
normal parallel test harness instead of requiring `--test-threads=1` for
correctness.

Run this plan **after** `plans/restore-forgejo-fixture-auto-download/` lands. In
particular, assume ignored Forgejo tests call the normal fixture startup paths,
those paths auto-download pinned binaries on a cache miss, and the old manual
cache-population helper is gone. Do not reintroduce a manual cache-population
prerequisite.

Hand the prompt files to agents **one phase at a time, in order**. Each phase
should land green, update this README's status, and record notable findings.

## Goals

- `cargo test ... -- --ignored` for Forgejo-based tests should be correct with
  libtest's default parallelism; `--test-threads=1` may remain only as an
  optional resource-throttling knob.
- Concurrent fixture startups from an empty `.cache/forgejo/` must not corrupt or
  partially publish server/runner binaries.
- Concurrent state-cache users with the same state key must either build once or
  wait, then each receive an independent runtime copy.
- Every mutable runtime resource used by a test must be per-test/per-process
  unique: Forgejo data dirs, runner work dirs, worker roots, stop files, log
  dirs, wake sockets, webhook secrets, and repositories within a shared server.
- Webhook trigger startup should not depend on a racy "find free port, drop it,
  then bind later" pattern.
- Add regressions that intentionally exercise parallel fixture startup.
- Update docs and inline test commands so future agents do not cargo-cult
  `--test-threads=1` as a correctness requirement.

## Non-goals and constraints

- Do not make the default non-ignored `cargo test` start Forgejo or download
  binaries.
- Do not remove `.cache/forgejo/` binary/state reuse; make shared caches
  process-safe instead.
- Do not hide the problem behind one global Forgejo-test mutex. Shared cache
  locks are fine; unrelated tests should be isolated enough to run concurrently.
- Do not change workflow semantics, backend contracts, or webhook correctness.
  This is test/fixture hardening.
- Do not make host-mode `forgejo-runner` safe for unbounded machine-level load.
  Docs may still advise limiting threads on small hosts for CPU/I/O reasons.
- Do not add external services beyond the existing pinned binary auto-download
  behavior restored by the prerequisite plan.

## Design sketch

### Resource ownership invariant

Every Forgejo test resource falls into exactly one bucket:

1. **Shared cache, guarded by a process-safe lock and atomic publish.** This
   includes pinned server/runner binaries and cached state trees under
   `.cache/forgejo/`.
2. **Per-test runtime resource with a unique name.** This includes copied
   Forgejo data dirs, runner work dirs, worker roots, wake sockets, stop files,
   logs, webhook secrets, temporary git checkouts, and any test-only files.
3. **Provider object isolated by server/repo.** Fixed users and repo names are
   fine only inside one throwaway server or one cached state. When two tests
   share one live server, repository names must be unique in that server.

Anything outside these buckets is a bug to remove or document as a deliberate
resource throttle.

### Cache publishing

After the auto-download restore, several parallel tests may all discover a
missing binary. Each pinned binary should have a per-target lock. The resolver
should check for the target before locking, lock, check again, download once,
write a unique temporary file in the cache directory, verify/set executable, and
atomically rename. If another process published the target first, discard the
local temp file and return the published target.

State-cache publishing already has a per-key lock and atomic directory rename.
This plan should audit and tighten the fast path so a `READY` marker implies the
metadata and tree are readable, and so stale/corrupt partial caches are rebuilt
under the lock rather than observed by concurrent readers.

### Runtime isolation

The existing `ForgejoServer` temp data-dir counter is thread-safe. Before Phase
1, `ForgejoRunner` derived its work dir from `NEXT_RUNNER.load()` before
incrementing the counter for the name, so two threads could choose the same
runner work dir. Phase 1 generates one runner instance id with `fetch_add` and
uses it for both the work dir and registered name.

Tests that spawn webhook triggers should own an already-bound listener or use a
startup helper that retries bind races before returning. Tests should not leave
shared temp paths such as `temper-forgejo-*-unused` where future code might start
writing.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Fixture cache and process identity concurrency.**
   `prompts/phase-1-fixture-cache-and-identity.md`

   Landed after confirming the auto-download restore commit was present. The
   fixture binary cache now uses per-target process-safe locks, rechecks after
   locking, writes unique same-directory temp files, and publishes by atomic
   rename. State-cache fast paths validate `READY`, `tree/`, and typed
   `metadata.json` before reuse. Runner names and work dirs share one allocated
   id, unique-dir generation has parallel unit coverage, and web startup retries
   only clear address-in-use bind races.

2. ☑ **Phase 2 — Per-test runtime isolation and webhook trigger startup.**
   `prompts/phase-2-runtime-isolation-and-trigger-startup.md`

   Added a `RunWorkspace` helper in `temper-testing` with parallel uniqueness
   coverage and moved Forgejo webhook worker roots, stop files, logs, wake
   sockets, trigger secrets, and temporary git credentials under per-test
   workspaces. Webhook triggers now start through
   `temper_production::trigger::run_with_listener`, which keeps an
   already-bound listener from port allocation through the serving loop; the
   shared trigger helper reports the actual reachable address. The audit also
   found Forgejo's default SSH authorized-keys path under the process user's
   home directory; the fixture now sets `SSH_ROOT_PATH` inside each server data
   dir so concurrent servers do not race on `~/.ssh/authorized_keys`. Fixed
   repository names remain only inside per-test throwaway servers or the declared
   shared multi-repo world.

3. ☑ **Phase 3 — Parallel stress regressions and validation commands.**
   `prompts/phase-3-parallel-stress-regressions.md`

   Added ignored stress regressions in `temper-forgejo-fixture` for concurrent
   same-state server startup and concurrent runner registration, plus a
   `temper-testing` regression proving parallel provisioned-state callers each
   receive an independent live server copy. The multi-repo webhook binary was
   validated with default libtest parallelism.

4. ☑ **Phase 4 — Documentation, findings, and final acceptance.**
   `prompts/phase-4-docs-findings-and-acceptance.md`

   Updated Forgejo how-to/reference/explanation docs and inline command examples
   so serial test threads are described only as optional host resource
   throttling. Final design and validation notes are recorded in
   `findings.md`.

## Whole-plan acceptance criteria

- ☑ Starting several ignored Forgejo fixture tests concurrently from an empty
  `.cache/forgejo/` does not corrupt binaries, leave `.part` files behind, or
  require a manual cache-population test.
- ☑ Starting several servers concurrently from the same `ForgejoState` either
  builds the state once or waits for the existing state, and every caller runs
  against its own `/tmp` copy.
- ☑ Two concurrent `ForgejoRunner::register` calls in the same test process choose
  distinct work dirs and runner names.
- ☑ Forgejo webhook tests can run with default libtest parallelism without trigger
  port races, shared wake sockets, shared logs, or shared stop files.
- ☑ No Forgejo-focused docs or test-module command examples require
  `--test-threads=1` for correctness. Any remaining mention is explicitly framed
  as optional host resource throttling.
- ☑ The final validation record includes at least one Forgejo test binary with
  multiple ignored tests run without `--test-threads=1`.

## Validation notes

Phase 3/4 validation on 2026-06-05 started from a warmed `.cache/forgejo/`
binary cache and existing state snapshots. The same-state fixture stress test
uses a unique state key and removes that key first, so it still exercised an
empty state-cache path for that key; the reference-delivery provisioning stress
used the warmed state cache (`cache_hits=2/2`).

Commands run without `--test-threads=1`:

```sh
cargo fmt --all
cargo test -p temper-forgejo-fixture
cargo test -p temper-forgejo-fixture --test parallel -- --ignored --nocapture
cargo test -p temper-testing --test forgejo_multi_repo_webhook -- --ignored --nocapture
cargo test -p temper-testing --test forgejo_parallel -- --ignored --nocapture
cargo dev-check
cargo dev-clippy
cargo dev-test
```

All commands passed. The broader optional Forgejo suites were not rerun in this
session to limit host CPU/I/O load after the focused stress tests and a
representative multi-test Forgejo webhook binary passed with default libtest
parallelism. Detailed findings are in `findings.md`.

## Relevant starting points

- Prerequisite plan: `plans/restore-forgejo-fixture-auto-download/`
- `crates/temper-forgejo-fixture/src/download.rs`
- `crates/temper-forgejo-fixture/src/state.rs`
- `crates/temper-forgejo-fixture/src/lib.rs`
- `crates/temper-forgejo-fixture/src/runner.rs`
- `crates/temper-testing/src/forgejo_server/provision_cache.rs`
- `crates/temper-testing/tests/forgejo_*.rs`
- `crates/temper-testing/tests/support/forgejo_multiprocess.rs`
- `crates/temper-testing/tests/support/forgejo_multi_repo.rs`
- `crates/temper-forge-forgejo/tests/live.rs`
- `crates/temper-production/src/trigger.rs`
- `docs/how-to/run-forgejo-multiprocess-e2e.md`
- `docs/how-to/end-a-development-session.md`
- `docs/reference/forgejo-backend.md`
- `docs/explanation/forgejo-e2e-topology.md`
