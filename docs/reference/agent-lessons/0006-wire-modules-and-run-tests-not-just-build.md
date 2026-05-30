# Lesson 0006: Wire new modules into the crate and run tests, not just build

## Tags

`rust`, `tooling`, `forgejo`, `process`

## Trigger

The Forgejo CI phase was committed with `src/ci.rs`, `src/ci_match.rs`,
`src/ci_time.rs`, and `src/dto.rs` that were never declared in `lib.rs`. The files
imported a hallucinated API (`crate::client::ForgejoForge`, `crate::http`,
`crate::dto`, `error::{decode, ensure_status, map_http_error, ApiResult}`,
`harness_forge::{CiConclusion, CiStatus, Timestamp}`, `ForgejoForge::new(config,
client)`) that does not exist. `cargo build -p harness-forge-forgejo` was green
because the dead files were never compiled, while `cargo test` was red (26 errors)
because `tests/ci.rs` did compile them.

## What went wrong

Two compounding mistakes: (1) new source files were left undeclared in `lib.rs`,
so they were dead code the compiler skipped; (2) handoff validation used only
`cargo build`, which never exercised the orphaned files or their test, so a
non-compiling crate looked green. The draft also invented types/functions instead
of reusing the crate's real helpers (`self.request_checked`, `self.send`,
`Self::decode`, `parse_*_id`, the real `CiJob`/`CiJobStatus`/`CiJobConclusion`
model).

## Steering for future agents

- A new `src/foo.rs` is invisible until it is declared (`mod foo;`) in `lib.rs`.
  Wire modules in the same change that creates them.
- Validate with `cargo dev-check` (all targets, including tests) and the crate's
  tests (`cargo test -p <crate>`), not just `cargo build`. A green `build` with a
  red `test` means files are orphaned or only test code references them.
- Before writing against an API, confirm it exists: read the real model
  (`harness-forge/src/model.rs`, `forge.rs`) and the canonical patterns in the
  crate (e.g. `pulls.rs`, `items.rs`, `error.rs`, `ids.rs`). Do not invent imports.

## Where this is now documented

- `AGENTS.md` ("Fast local iteration" already says to use `cargo dev-check`).
- `docs/how-to/end-a-development-session.md` (run the fast validation loop and
  task-specific tests before handoff).
- The reimplemented CI lives in `crates/harness-forge-forgejo/src/ci.rs`,
  `ci_match.rs`, `ci_time.rs` (DTOs in `types.rs`), wired in `lib.rs`, with
  `tests/ci.rs` green; see `docs/reference/forgejo-backend.md`.
