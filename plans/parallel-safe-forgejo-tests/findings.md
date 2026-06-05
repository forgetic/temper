# Parallel-safe Forgejo tests — findings

Date: 2026-06-05.

## Final design

- Pinned Forgejo server and runner binaries live under `.cache/forgejo/`.
  First-use publishers take a per-target lock, write a unique same-directory
  `.part-<pid>-<id>` file, verify checksums before publish, and atomically
  rename complete binaries into place.
- Cached Forgejo states live under `.cache/forgejo/states/<cache-key>/`.
  Readers require `READY`, `tree/`, and typed `metadata.json`; publishers take a
  per-key lock, build under a unique temporary directory, stop the source server
  cleanly, then atomically rename the completed state into place.
- Runtime state is never served from the cache tree. `start_with_state` copies
  the cached `tree/` into a unique `/tmp/temper-forgejo-<pid>-<id>` directory for
  each caller before starting `forgejo web`.
- Runner registration uses one allocated identity for both runner name and work
  dir (`/tmp/temper-forgejo-runner-<pid>-<id>`), avoiding same-process work-dir
  collisions.
- `temper-testing` Forgejo webhook tests allocate per-test workspaces for logs,
  worker roots, stop files, wake sockets, secrets, and temporary credentials;
  the fixture also sets Forgejo `SSH_ROOT_PATH` inside each server data dir.

## Added regressions

- `crates/temper-forgejo-fixture/tests/parallel.rs`
  - `same_state_startups_are_parallel_cache_safe`: starts four concurrent
    `ForgejoServer::start_with_state` callers for the same unique state key and
    asserts distinct base URLs/data dirs, runtime copies outside the cache tree,
    one cache miss from an empty state key, shared cache key, and clean teardown.
  - `concurrent_runner_registrations_use_distinct_identities`: registers two
    host-mode runners concurrently against distinct live server copies and
    asserts distinct server/runner runtime resources. It reports a clear skip if
    the runner binary cannot be resolved.
- `crates/temper-testing/tests/forgejo_parallel.rs`
  - `cached_reference_delivery_state_is_safe_for_parallel_callers`: starts two
    parallel cached reference-delivery servers, performs an exact Forgejo API
    repository read against each copy, and asserts distinct runtime resources
    plus one shared provisioned-state cache key.

## Validation

The validation run started from a warmed `.cache/forgejo/` binary cache with
existing state snapshots. The fixture same-state stress regression creates a
unique state key and deletes that key before starting, so its state-cache path was
empty while the shared binary cache was warm. The `temper-testing` provisioned
parallel regression reused the warmed reference-delivery state cache
(`cache_hits=2/2`).

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

Results:

- `cargo test -p temper-forgejo-fixture`: passed; 15 non-ignored tests, 2 ignored
  integration tests listed.
- `temper-forgejo-fixture --test parallel`: passed; 2 ignored stress tests.
- `temper-testing --test forgejo_multi_repo_webhook`: passed; 2 ignored webhook
  tests ran under the default libtest parallel harness.
- `temper-testing --test forgejo_parallel`: passed; 1 ignored provisioning stress
  regression.
- `cargo dev-check`, `cargo dev-clippy`, and `cargo dev-test`: passed.

The broader optional ignored Forgejo suites (`forgejo live`, single-repo webhook,
full multiprocess, and all ignored `temper-testing`) were not rerun in this
session to avoid additional host CPU/I/O load after the representative multi-test
Forgejo webhook binary and focused stress regressions passed.

## Caveat

Default libtest parallelism is correctness-safe for the Forgejo fixture caches
and runtime paths. Operators may still add `--test-threads=1` as a host resource
throttle on small machines because these tests spawn real Forgejo, runner, worker,
and git/CI processes.
