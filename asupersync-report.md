# asupersync 0.3.4 — code quality review

Reviewed 2026-06-11 (Claude Code; six parallel subsystem reviews + build/test attempts).
Source: published crates.io package, extracted at `~/.cache/asupersync-review/asupersync-0.3.4`
(with local "review patch" comments where the package was unbuildable).
Detailed per-subsystem evidence: `notes-runtime.md`, `notes-cancel.md`, `notes-channels.md`,
`notes-tests.md` in this directory.

## Headline

A real, carefully engineered async-runtime core (~15% of the crate) wrapped in an
order-of-magnitude larger shell of speculative subsystems, lab-only "guarantees",
and verification theater. The load-bearing primitives are better than expected —
several reviewers went hunting for classic lost-wakeup/cancellation races and came
up empty — but the project's *claims* (bounded cancellation, runtime-tracked
two-phase effects, Lean proofs, conformance suites) are largely not enforced or
not present in the shipped artifact. The published package cannot compile its own
test suite at all, and after fixing four independent packaging defects the test
binary needs >12 GB RAM at `-j1` to build (OOM on a 16 GB machine).

## Facts / trajectory

- Repo created 2026-01-16; 1.44M lines of Rust in `src/` by June (≈10k lines/day).
  15 releases, 6 yanked. 0.3.2→0.3.3 added ~180k lines; the 0.3.4 "patch" added
  1,100 lines of new feature code alongside a genuine security fix (zeroize +
  un-Serialize the AES-256 mailbox key, redacted Debug).
- License is "MIT + OpenAI/Anthropic rider" — denies all rights to those
  companies and forbids ML-training use. **Non-OSI.** The rider is present in the
  LICENSE file of every recent release (verified back to 0.2.9, including 0.3.2);
  0.3.3 only changed the Cargo.toml metadata from `license-file` to a LicenseRef
  naming it. There is no plain-MIT fork point among published versions.
- 0.3.3+ require nightly (`#![feature(try_trait_v2, try_trait_v2_residual)]`,
  unconditional). 0.3.2 remains the last stable-toolchain release.
- `#![deny(unsafe_code)]` headline, but 40 files opt out; ~507 unsafe sites
  (normal for a runtime; the marketing needs the asterisk).
- Scope: the "runtime" also contains QUIC (native impl), RaptorQ FEC, Redis/NATS/
  JetStream/Kafka clients, gRPC, WebSocket, DNS, database pools, distributed
  systems toolkit, an observability platform, and the author's AI-agent-swarm
  control plane (`src/agent_swarm`, ATP daemon, "beads", "proof lanes") — the
  development process ships inside the product. Sampled exotic subsystems are
  substantive implementations, not stubs (zero `todo!()`/`unimplemented!()`).

## Published-package hygiene (worst finding)

`cargo test --lib` on the published crate fails for four independent reasons:
1. `src/observability/mod.rs` declares `otel_conformance_tests` — file excluded
   from the package by Cargo.toml include rules.
2. `src/lib.rs` declares a `#[path]` test module pointing at
   `tests/conformance/task_inspector_wire.rs` — also excluded.
3. ~34 errors: `#[cfg(test)]` tests in `messaging/jetstream.rs`/`kafka.rs` use
   types gated behind `feature = "test-internals"` without that gate.
4. 19 `include_str!`/`include_bytes!` targets (goldens, artifacts, scripts, docs)
   missing from the package (codec, DNS, scheduler, trace modules).
After restoring all of that (files fetched from GitHub main), rustc needs >12 GB
RSS at `-j1`, debug=0, no incremental → OOM-killed at 9G and 12G cgroup caps.
Conclusion: no one has ever built or run the published package's tests.
(`cargo check --lib` does pass cleanly on nightly 1.98, ~2 min; docs.rs builds.)

## Per-subsystem verdicts (see notes files for full evidence)

### Runtime/scheduler — sound core, latency cliffs at the seams
- Sound: three-lane scheduler, Dekker-correct parker, tri-state waker race
  handling, IO leader/follower with panic-safe lock release, region/quiescence
  genuinely enforced (`state.rs:3370-3530`).
- **B1 (HIGH for temper): `block_on` self-throttles** — after 128 wake-during-poll
  rounds it `thread::sleep`s 1→5→25 ms (`builder.rs:4135-4189`). A high-wake-rate
  top-level future (sans-IO engine pattern) converges to one 25 ms sleep per 128
  polls.
- **B2 (HIGH): `!Send`-task wakers never wake the epoll leader**
  (`three_lane.rs:6472`) — cross-thread wakes of `spawn_local` tasks stall up to
  250 ms, unbounded if a long timer deadline is pending.
- B3: governor spawn-throttle path permanently loses tasks (dead code in default
  runtime; real in embedded configs with `enable_governor(true)`).
- B4: `spawn_blocking` panics (not Err) on shutdown race (`spawn_blocking.rs:169`).
- Dead weight: complete second worker implementation (~800 lines, unreferenced),
  `epoch_gc.rs`/`epoch_tracking.rs` (2.6k lines, unreferenced), default-off
  Lyapunov/Bayesian/spectral scheduling machinery; default-ON fairness+invariant
  monitors tax every dispatch. Doc claims "lock-free" paths that are mutexes.
- Safe envelope: single worker, `Send` tasks only, reactor wired, governor off.

### Cancellation/Cx/time — protocol real, guarantees lab-only
- Real: cancel cascade, task cancel state machine, cancel lane, checkpoint/mask,
  `Scope::race` drains losers, correct atomics.
- **Theater: cleanup budgets are metered only by the lab runtime** —
  production workers never call `Budget::consume_poll`; a non-cooperative future
  is polled forever; region close can hang. **The runtime obligation ledger has
  zero production writers** — leak oracle/quiescence fence watch an empty set;
  ~25k lines of obligation "formalism" model the code rather than constrain it.
- Bugs: `sleep_until(now + >7 days)` fires early (wheel clamp, no re-register;
  `wheel.rs:483-498`, `sleep.rs:468`); **two-clock epoch skew is structural**
  (per-driver epoch vs process-global fallback epoch, `Time` is an untagged u64 —
  exactly the temper `timer_now()` vs `cx.now()` bug); `TrackedPermit` panics on
  ordinary cancellation (race-loser drop); `set_cancel_requested(false)` desyncs
  Cx vs task record; no dedicated timer thread (timers stall if workers stuck).

### Channels/sync — production-grade core, broken seams
- mpsc, oneshot, Notify, Mutex, RwLock, broadcast, watch, OnceCell, Barrier:
  end-to-end reads found **no concurrency bug**; consistent conservative design
  (coarse mutex, wake-after-unlock, generation-tagged tokens, baton-passing
  cancellation); named regression tests pin real historical races.
- **Semaphore (HIGH ×2)**: `try_acquire` panics outside task context *after*
  decrementing permits → permanent capacity leak (`semaphore.rs:374,393-406`);
  `ConcurrencyLimit`'s no-Cx path is guaranteed to panic at successful
  acquisition (`semaphore.rs:954-965`) — never executed by any test.
- `MutexGuard` "is !Send" audit comment is false (auto-Send).

### net/http — best-audited area; deploy behind a trusted boundary
- h1 parser genuinely hardened (CL/TE conflicts, duplicate framing headers,
  chunk-size strictness, bare-CR, trailer smuggling, response splitting) — real
  fuzzing evidence. Epoll reactor lifecycle (oneshot+rearm, fd identity checks,
  ENOENT/EBADF tombstones) correct.
- **MEDIUM-HIGH: no handler or response-write timeout** — only between-request
  `idle_timeout` exists; slow-read clients pin connection slots forever
  (slowloris → default 10k slot exhaustion) (`server.rs:452-459, 684-735`).
- MEDIUM: server buffers the whole request body before the handler (no streaming;
  worst case `max_connections × max_body_size`).
- Default `HostPolicy::RejectUnknown` 421-rejects every request (structural
  footgun temper already works around).

### io/bytes/fs/process/signal — strong io traits, look-alike bytes, blocking traps
- io futures layer is the strongest-reviewed code (partial progress, lying-writer
  checks, poisoning, cancellation best-effort drain).
- **bytes is not tokio-bytes**: `BytesMut`/`Vec<u8>` violate the `BufMut`
  contract — `chunk_mut()` returns `&mut []`, `advance_mut(n>0)` panics
  (`bytes_mut.rs:570-583`); generic tokio-ported write code detonates.
  `split_to`/`split_off`/`freeze` memcpy (zero-copy claim false for the mutable
  half); `set_len` zeroes data (silently corrupts the spare-capacity read
  pattern).
- process: dropping `output_async` futures (race-loser) leaks a running child +
  permanent zombie — no global reaper; mitigate with `kill_on_drop(true)`.
  SIGTERM→SIGKILL escalation itself verified correct; pipe-deadlock avoided.
  First `signal::signal()` call installs handlers for ~10 signals process-wide.
- **Architecture trap for temper**: `spawn_blocking` with a Cx but no blocking
  pool handle runs the closure **inline on the calling thread**
  (`spawn_blocking.rs:217-223`) — all fs ops route through it; `File`'s
  AsyncRead/Write does blocking syscalls directly in `poll` regardless
  (`fs/file.rs:191-241`). `Sleep` without a timer driver spawns a thread per
  sleep; IO without a driver busy-spins via immediate self-wake.

### Testing & claims — two suites stapled together
- Protective: ~22,600 in-module `#[test]` fns next to real code; sampled
  mutations (LIFO run queue, mpsc drops, permit leak, timer misorder) would be
  caught. The lab runtime really drives the real scheduler with seeded chaos and
  meaningful oracles (obligation leak, quiescence, futurelock), incl. an
  anti-truncation check.
- Theater (~90-95% of the *branded* layer): ~34 of 75 conformance/metamorphic
  files test mocks of themselves (now quarantined behind undocumented
  `legacy-internal-test-harnesses`); supervision suite re-implements the logic it
  "tests" and runs by default; "mutation testing" mutates nothing
  (`let passed = true;`); DPOR/ScheduleExplorer never aimed at a real primitive;
  golden corpus absent (golden tests cannot pass); **formal/lean contains zero
  .lean files** — two READMEs assert proofs exist; an internal reconciliation doc
  admits earlier proof claims were false.

## Quality assessment summary

- **Ideas**: genuinely good (regions/quiescence, cancellation as protocol,
  two-phase effects, deterministic lab). The pitch identifies real gaps in
  async Rust.
- **Design**: core primitives conservative and correct; macro-architecture
  undisciplined — four obligation systems, parallel dead implementations,
  control-theory machinery nothing uses, process artifacts in the product.
- **Testing**: real and substantial at the in-module layer; branded verification
  surface largely fake; published package untestable, so none of it gates
  releases as shipped.
- **Robustness**: good on the narrow happy path (Send tasks, reactor wired,
  in-ecosystem futures); failure modes cluster at integration seams — panics
  where errors belong, silent latency cliffs, leaks on drop paths.
- **Future-proofness**: poor. Nightly-only, non-OSI license, 1.4M-line single
  crate a 16 GB machine cannot test-compile, solo-maintained at AI speed with
  6/15 releases yanked. High risk of breaking changes and unreviewable diffs.
- **Trajectory**: improving in places (real remediation pass on conformance
  tests, genuine 0.3.4 security fix, honest scope-limit notes in README), but
  release discipline (features in patches, unbuildable packages) unchanged.

## Guidance for temper (pins =0.3.2)

1. Stay on 0.3.2 (last stable-toolchain release) — already done.
2. The two-clock epoch skew is structural; keep the "driver clock only" rule.
3. `HostPolicy::AllowAll` already set; fine for engine services.
4. Add `kill_on_drop(true)` to Commands whose output futures can be
   dropped/raced, or never drop them (cancel via Cx instead).
5. Audit that the engine's Cx carries a blocking-pool handle; treat asupersync
   `fs` (and any `File` via io traits) as blocking regardless.
6. If the engine's main loop is a high-wake-rate future inside `block_on`,
   measure for the 25 ms backoff cliff (B1); raise `poll_budget` if hit.
7. Avoid `spawn_local` + cross-thread wakes (B2 stalls); temper is single-
   threaded engine + blocking pool, so blocking-pool completions waking local
   tasks would hit exactly this.
8. Don't touch `Semaphore::try_acquire`/`ConcurrencyLimit` off-runtime.
9. h1 server: front it with timeouts or trusted clients only (no handler/write
   deadline upstream); keep `max_body_size`/`max_connections` conservative.
10. License: ALL recent versions (incl. the pinned 0.3.2) carry the non-OSI
    Anthropic/OpenAI rider in their LICENSE file — a real consideration for
    distribution and for AI-assisted tooling on this dependency.
