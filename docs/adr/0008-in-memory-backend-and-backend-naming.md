# ADR 0008: Add an in-memory Forge backend and name backends by provider

## Status

Accepted

## Context

Harness had one concrete `Forge` backend, the filesystem store in the `harness-fs`
crate. It is deterministic and good for development, but every operation touches
the filesystem: temporary directories must be created and cleaned up, records
round-trip through JSON, and stored data is re-validated on every read.

Most workflow-layer tests do not care that the backend is filesystem-backed.
They need *a* deterministic `Forge` to drive the executor, planner, leases,
journaling, and reconciliation. For those tests a pure in-process backend is
simpler and faster, with no temp-directory lifecycle and no JSON serialization.

Two further problems shaped the decision:

- The crate name `harness-fs` did not signal that it is one *Forge backend*
  among several. As more backends arrive (in-memory now, Forgejo or GitHub
  later) an unprefixed name does not scale.
- The filesystem backend's only way to exercise the `ForgeError::Backend` error
  path in a test was to corrupt a JSON file on disk. An in-memory backend has no
  durable store to corrupt, so it needs another way to surface backend failures.

## Decision

Adopt a `harness-forge-<provider>` naming convention for concrete backends:

- rename `harness-fs` to `harness-forge-filesystem` (the `FilesystemForge` type
  name is unchanged), and
- add `harness-forge-memory` with an in-memory `MemoryForge`.

`MemoryForge` reproduces the filesystem backend's observable contract: the same
deterministic identifier scheme, the same one-second logical clock, the same
ordering, and the same conjunctive query semantics. This lets workflow tests
move between the two reference backends without re-baking expectations.

Duplicate the small set of pure helper logic (ordering, query matching,
label/assignee/state updates, deterministic id and clock construction) into the
in-memory backend rather than extracting a shared backend-support crate. The
helpers are small and stable, the duplication keeps each backend self-contained,
and it avoids a premature shared abstraction. If a third backend needs the same
logic, revisit this and extract a shared crate then.

Give the in-memory backend a small one-shot fault hook
(`MemoryForge::fail_next`) covering the operations the workflow runtime
exercises. A test arms a fault and the next call to that operation returns
`ForgeError::Backend` before touching state. This replaces filesystem corruption
as the way to test backend error paths and stays deterministic.

Run the workflow-layer integration tests against `harness-forge-memory`. Keep
the filesystem backend's own conformance tests on `harness-forge-filesystem`,
because those tests exist to verify the filesystem backend specifically.

## Consequences

- Workflow tests no longer create or clean up temporary directories; the shared
  `TestRoot` helper now wraps a cloned in-memory store.
- There are two reference backends to keep behaviourally aligned. The shared
  contract is documented in `docs/reference/in-memory-backend.md` and
  `docs/reference/filesystem-backend.md`; behaviour changes must update both
  backends and both pages.
- The duplicated helper logic must be changed in both crates when the shared
  semantics change. This is an accepted, documented trade-off.
- The `harness-forge-` prefix sets the pattern for future backends.
