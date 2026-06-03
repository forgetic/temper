# Phase 3 prompt — Forgejo scalable backend paths

## Goal

Make `temper-forge-forgejo` execute the Phase 2 query shapes efficiently. In
particular, labelled PR queries must not degrade into fetching every historical
PR, and dependency detail should not be loaded with an N+1 request unless the
caller requested/needs it.

## Required reading

- Phase 1 and Phase 2 implementations
- `docs/reference/forge-interface.md`
- `docs/reference/forgejo-backend.md`
- `crates/temper-forge-forgejo/src/issues.rs`
- `crates/temper-forge-forgejo/src/pulls.rs`
- `crates/temper-forge-forgejo/src/dependencies.rs`
- `crates/temper-forge-forgejo/src/ci.rs`
- `crates/temper-forge-forgejo/src/ci_ui.rs`

## Implementation tasks

1. Honor any new list detail/include flags added in Phase 2 across all reference
   backends. Defaults must preserve the documented Forge contract; scanner calls
   can opt into cheaper summaries.
2. For Forgejo issue queries, keep using provider-side `state` and `labels`
   filters. Ensure normal scan paths request `state=open` or labelled `closed`,
   never `state=all` by accident.
3. For Forgejo PR queries with labels, avoid `/pulls?state=all` + client-side
   filtering. Prefer a provider-specific path such as:
   - query the issues endpoint with `type=pulls`, `state`, and `labels` to get
     candidate PR numbers; then
   - fetch `/pulls/{number}` only for those candidates.
   If Forgejo's exact shape differs, isolate the provider-specific discovery in
   one helper and cover it with mock-contract tests. Fallbacks must be explicit
   and diagnosable, not silently broad in production hot paths.
4. Avoid dependency N+1 enrichment for list calls that only need labels/body/state.
   Load dependencies on demand for dependency-gated queues/transitions and exact
   dependency-target checks.
5. Keep CI web-UI reads behind the Phase 1 signal-needs gate. Add tests proving a
   non-CI scan path does not reach `ci_ui`.
6. Update backend reference docs for any query/detail semantics or Forgejo
   provider-specific PR label path.

## Tests to add or adjust

- Forgejo mock-contract test: labelled PR query issues a narrow label/state
  request and does not call `/pulls?state=all`.
- Forgejo mock-contract test: unlabelled closed PR history is not fetched by a
  normal scan query.
- Forgejo mock-contract test: dependency detail is skipped for summary list
  calls and loaded when requested.
- Existing Forgejo backend CI web-UI tests still pass.
- Memory/filesystem contract tests cover any new query/detail field.

## Validation

Run at least:

```sh
cargo fmt --all
cargo test -p temper-forge
cargo test -p temper-forge-memory
cargo test -p temper-forge-filesystem
cargo test -p temper-forge-forgejo
cargo test -p temper-runner
cargo dev-check
```

If the Forgejo binary cache is present, optionally run a focused ignored smoke
that exercises labelled PR queries against the live throwaway server.

## Done when

- Forgejo labelled PR queries scale with the labelled candidate set, not total
  repository PR history.
- Dependency enrichment is demand-driven.
- Backend docs describe the new behavior and any provider caveats.
- This plan README is updated with Phase 3 status and notable findings.
