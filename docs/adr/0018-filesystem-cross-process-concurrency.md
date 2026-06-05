# ADR 0018: Filesystem backend cross-process concurrency via advisory locking

## Status

Accepted

## Context

The filesystem backend (`temper-forge-filesystem`) stores all of a repository's
issues in one `issues.json`, all pull requests in one `pull_requests.json`, and
the logical clock plus ID counters in one `metadata.json`. Each file is written
through a temporary file followed by an atomic `rename`, so a single write is
crash-safe and a reader always sees a complete old-or-new snapshot.

That is enough within one process, but not across processes. Every mutating
operation is a read-modify-write-persist: it loads the whole file, edits one
record in memory, and rewrites the file. There was **no cross-process lock**, and
the temporary path was a fixed `path.with_extension("tmp")`. So two OS processes
mutating *different* artifacts in the *same* file would both load it, both edit
their own record, and both `rename` their version over the other — last write
wins, silently dropping one update. Concurrent creates read the same
highest-number-plus-one and allocate duplicate item numbers. The shared
`metadata.json` clock and counters race the same way.

This also undermines ADR 0013. Its compare-and-swap (`expected_version`) is
checked against the version read from disk; without serialization two writers can
both read version *v*, both pass their `expected_version == v` check, and one
clobber the other. The CAS guarantee therefore only held *within* a single
process. The true multi-process end-to-end rehearsal
(`docs/how-to/run-multiprocess-e2e.md`) needs it to hold across OS processes that
coordinate solely through a shared store.

## Decision

Serialize every mutating read-modify-write-persist with a **store-level exclusive
advisory lock**.

- A single lock file lives at `<root>/.lock`. Each mutating operation acquires an
  exclusive `flock` on it (via the `fs2` crate) for its whole critical section
  and releases it when done. A store-level (not per-file) lock is correct and
  sufficient because the shared `metadata.json` clock already funnels every
  mutation onto one resource; finer-grained locking would add complexity without
  removing that serialization point.
- **Reads stay lockless.** The atomic `rename` already yields a complete
  old-or-new snapshot, and the pull-based runtime re-reads fresh state under the
  lock before each mutation, so an unlocked read at worst sees slightly stale —
  never torn — data.
- `storage.rs::write_json` now uses a **per-process-unique** temporary filename
  (process id plus an atomic counter) instead of the fixed `.tmp` extension, so
  no two writers collide on the temp path before the rename. The extension is not
  `json`, so listing code that filters on `json` still ignores it.

`fs2` is added as a dependency. Its `unsafe` is internal to that crate, so the
workspace `forbid(unsafe_code)` lint stays intact for our crates.

### Why this is filesystem-specific

This is a durability concern of a shared on-disk store, not an observable
contract change. The single-process in-memory backend (`temper-forge-memory`)
already serializes every operation behind one interior mutex and cannot be shared
across OS processes, so it needs no parallel change. That keeps the ADR 0008
observable-contract parity honest: both backends still expose identical behaviour
(including ADR 0013's CAS and `ForgeError::Conflict`); only the filesystem
backend needs locking to deliver it across processes. The N/A is recorded in
`docs/reference/in-memory-backend.md`.

## Consequences

- The `Forge` trait and all observable query/mutation semantics are unchanged;
  this is an internal robustness fix. No new error variants or fields.
- ADR 0013's compare-and-swap now holds across OS processes: two acquirers over
  one "no lease"-style snapshot yield exactly one CAS winner; the loser observes
  `ForgeError::Conflict`. Concurrent updates to distinct artifacts in one file
  lose no writes, and concurrent creates allocate distinct item numbers. A
  deterministic multi-thread backend test (real threads + a barrier, no sleeps)
  proves all three and runs in the default suite.
- Mutations now block on a single lock, so heavily concurrent writers serialize.
  For a deterministic development/test backend this is the intended trade-off;
  the lock is held only for the short read-modify-write-persist window.
- The lock file is created lazily and never removed; it is a stable coordination
  point under the store root.
- Updated `docs/reference/filesystem-backend.md` (consistency/concurrency) and
  `docs/reference/in-memory-backend.md` (the N/A note).
