# Vendored crates (rustc 1.85 compatibility)

These are unmodified copies of crates.io releases except for the minimal
patches described below. They exist because asupersync 0.3.1 and parts of its
dependency tree use language/std features that only stabilized after
rustc 1.85, which is the toolchain this machine builds with. The workspace
`[patch.crates-io]` section in the root `Cargo.toml` points at these copies.

| crate | version | patch |
|---|---|---|
| `asupersync` | 0.3.1 | (a) `src/lib.rs` prepends `#![feature(let_chains, integer_sign_cast, duration_constructors, unsigned_is_multiple_of)]` (+ `allow(stable_features)`); compiled with crate-scoped `RUSTC_BOOTSTRAP=asupersync` set in `.cargo/config.toml` `[env]`. (b) Two behavioral fixes for a lost-wakeup hang where production `sleep`/`timeout` futures never fired on an idle runtime (a bare 300ms sleep hung forever, verified empirically): `src/runtime/scheduler/three_lane.rs` clamps the I/O leader's blocking reactor poll to 25ms — upstream blocks with no timeout when no deadline is known yet, and a timer registered afterwards never kicks the reactor while followers deliberately ignore shared timer deadlines; `src/runtime/scheduler/worker.rs` (the non-default work-stealing scheduler) additionally pumps `timer_driver.process_timers()` each loop iteration, which that loop lacked entirely. |
| `asupersync-macros` | 0.3.1 | one let-chain in `src/session.rs` desugared to nested `if` (pure stable Rust, no env tricks needed). |
| `franken-evidence` | 0.3.1 | one let-chain in `src/export.rs` desugared to nested `if`. |

Additionally `Cargo.lock` pins (documented in the root `Cargo.toml`):

- `virtue-next 0.1.2` (0.1.3 needs `const_vec_string_slice`, rustc >= 1.87)
- `franken-decision 0.3.1`, `franken-kernel 0.3.1` (0.3.4 uses let-chains)

When the toolchain moves to rustc >= 1.88 (edition-2024 let-chains stable),
the whole vendor directory plus the `[env]`/`[patch]` plumbing can likely be
dropped in favor of stock `asupersync` from crates.io (re-test 0.3.4 then).
