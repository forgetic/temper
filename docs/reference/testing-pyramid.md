# Testing pyramid

Temper's pyramid is deliberately bottom-heavy, but the middle is where most
feature confidence should land. The codebase has fast seams that exercise real
workflow behavior without booting real Forgejo: memory/filesystem forges,
in-process daemon/worker transports, deterministic fake agents, jig fake LLMs,
and the `temper-sim` lab runtime.

For a numeric snapshot, see [Test inventory](testing-inventory.md).

## Target shape

1. **Unit / component tests** prove pure logic, parsing, mapping, and local
   invariants. They live next to the code in `src` test modules.
2. **Contract tests** prove portable interfaces and provider mappings. They
   live under each backend crate's `tests/` directory and use memory stores or
   mock HTTP clients, never the network.
3. **Hermetic integration tests** are the default place for behavior changes.
   They compose workflow, engine, runner, worker, fake agents, local git, and
   in-process transports while staying deterministic and fast.
4. **Simulation / machine tests** cover timing, retries, concurrency, recovery,
   and chaos under seeded virtual time. Use them when sleeps or races would be
   required in ordinary tests. `temper-sim` offers hand-rolled protocol workers
   for cheap HTTP-path/misbehavior coverage plus a real `WorkerMachine` /
   `WorkerShell` harness over `temper-daemon-transport::InProcessTransport` for
   high-fidelity worker-loop coverage under `LabRuntime`.
5. **UI model and DOM tests** protect web reducers, feed contracts, server/read
   model seams, and rendered interactions with fake feeds.
6. **Live e2e tests** are ignored live suites. A narrow subset are default
   capstones; the rest stay in the manual/all-e2e lane. They prove real Forgejo,
   git, webhooks, host-mode CI, binary wiring, and real provider credentials only
   where no hermetic seam can prove the same thing.

The goal is not maximum formal coverage. The goal is to put each assertion at
the highest-confidence layer that is still cheap, deterministic, and local.

## Where a new test should live

- Pure parser, formatter, DTO, config, or mapper: keep it in the owning `src`
  module's `mod tests` or nearby `*_tests.rs` module.
- Forge trait semantics: add a backend contract test. Prefer a shared fixture
  shape across memory/filesystem/Forgejo/GitHub when the behavior is portable.
- Provider request/response mapping: use that provider crate's mock HTTP client
  support. Add live Forgejo only for a real-provider quirk that mock HTTP cannot
  represent.
- Workflow validation, planning, gates, queues, leases, reconciliation, or
  recovery: use `crates/temper-workflow/tests/` with `MemoryForge` and focused
  fixtures.
- Engine appliers, feeds, webhook route, role routing, and worker protocol:
  use `crates/temper-engine/tests/` with memory forge or an in-process server.
- Runner behavior and role/mechanical workers: use `crates/temper-runner/tests/`
  with `temper-testing` fakes and memory/filesystem forges.
- Worker workspace, executor, real git checkout, or out-of-process agent
  boundary: use `crates/temper-worker/tests/` with local `file://` repos and
  deterministic fake agents.
- Daemon/worker single-process behavior: prefer the in-process daemon transport
  harness before spawning a real binary.
- Agent loop or LLM protocol behavior: use `jig` fake LLMs, request oracles, or
  sans-IO machine tests. Real OAuth/provider tests must be ignored and
  environment-gated.
- CLI/config artifact behavior: put parser/unit tests in the CLI crate.
  Use root integration tests only when installed binaries or full artifact
  layout matter.
- UI event semantics: first test the reducer/model; then add a happy-dom test
  only if rendered behavior or browser events matter.
- End-to-end Forgejo behavior: use the ignored root e2es only for real webhooks,
  real git authentication, real Actions CI, close-on-merge, or binary wiring.

## Design rules

- Prefer one story test plus small edge-case tests. A story test should assert
  user-visible state transitions, not every internal function call.
- Keep live tests narrow. Each real-world mechanism should have one capstone;
  do not re-prove workflow logic in live Forgejo when memory or simulation can
  prove it faster.
- Use deterministic fakes rather than sleeps. If timing is the behavior, prefer
  `temper-sim` virtual time or a controlled cadence seam.
- Do not read ambient user config, credentials, or env in default tests. Use
  tempdirs, explicit `LoadOptions`, fake tokens, and per-test fixtures.
- Do not share mutable global state between tests. If a live test must
  serialize, use the root e2e file lock and/or nextest test groups.
- Name tests by behavior: `thing_does_result_when_condition`, not implementation
  details.
- Assert on durable outputs: Forge state, workflow decisions, pushed branch SHA,
  emitted protocol messages, rendered DOM, or log diagnostics needed on failure.
- Add enough failure diagnostics for async/process tests: log tails, scenario
  names, issue/PR ids, head SHAs, CI diagnostics, and seed values.
- Keep fixtures canonical. When an example workflow and an embedded fixture must
  match, assert byte equality as part of the closest story test.
- Real provider tests must be `#[ignore]`, must self-skip without an opt-in env,
  and must never print secrets.

## Tooling and setup

Use the aliases in `.cargo/config.toml`:

```sh
cargo dev-check          # fast workspace type check
cargo dev-scenario-check # validate checked-in scenario manifests and local refs
cargo dev-scenario-run scenarios/<name> # sole manual alias; run implicit live topology
cargo dev-scenario-validate-feature ... # resolve and run one exact-head mapping
cargo dev-test-quick     # default non-ignored suite via nextest
cargo dev-test-build     # prebuild all test binaries
cargo dev-test-e2e-capstones  # ignored live capstones used by dev-test-full
cargo dev-test-e2e            # short shorthand for the capstone lane
cargo dev-test-e2e-all        # every ignored/manual live test
cargo dev-test-full           # quick + ignored live capstones
```

The web UI is separate from Cargo:

```sh
(cd crates/temper-web/ui && npm test && npm run build)
```

CI runs format, depgraph, file-size, ambient-env, build, scenario manifest
checks, quick Rust tests, the two live e2e capstones, clippy, and then a
separate web job for Vitest/build. The full ignored/manual e2e lane remains an
explicit diagnostic command. Aggregate `feature/*` PRs into `main` also run
the exact checked-out head through one resolved mapped scenario and retain the
mapping plus structured evidence. `cargo dev-scenario-run scenarios/<name>` is
the sole manual scenario alias; it requires an explicit path and executes the
implicit topology of real Forgejo, a host `forgejo-runner`, standalone Temper,
and Jig fake-LLM agents.

MemoryForge, filesystem-forge, in-process, hermetic real-stack, and simulation
suites remain lower-level coverage. They are not alternate scenario CLI modes or
feature-landing scenario evidence.

## Current effectiveness assessment

Strengths:

- The middle of the pyramid is unusually strong. Many tests are e2e-ish while
  still default-fast: they drive real workflow/engine/worker machines, real
  local git, real protocol DTOs, and fake deterministic agents.
- The live Forgejo suite is valuable and appropriately small. It covers the
  residual real-world risks: webhooks, git auth, Actions CI, process wiring,
  and provider behavior.
- `temper-sim` gives high-confidence coverage for time, scheduling, and failure
  interleavings without wall-clock sleeps.
- UI tests already separate reducer/feed contracts from DOM interaction tests.

Weaknesses and debt:

- Test support is normalized in spirit but fragmented in files. `block_on`,
  `TestRoot`, repo builders, mock HTTP clients, temp workspaces, free-port
  helpers, and log-tail diagnostics are repeated across crates.
- Backend contract coverage is broad but not yet expressed as one reusable
  provider-conformance corpus. That makes portable Forge semantics easy to drift
  between memory, filesystem, Forgejo, and GitHub.
- Root Forgejo e2es share patterns but not one common process/world harness.
  More of `tests/support/*` could move into `temper-testing`.
- Ignored tests now have separate capstone and manual/all-e2e lanes, but
  provider live tests still rely on internal env self-skips; keep that contract
  explicit as provider probes grow.
- `temper-sim` still has a fidelity gap: its worker simulant is hand-rolled.
  Issue #165 should move the real worker machine/shell into that layer.
- The UI test command is not represented by a Cargo alias, so agents must read
  this doc or CI to remember the Node validation path.

## Cleanup roadmap

1. Extract common no-park `block_on`, temp workspace, repo/user builders, and
   log-tail helpers into `temper-testing` when more than one crate needs them.
2. Define a reusable Forge backend conformance suite for portable operations;
   keep provider-only HTTP mapping tests beside each backend.
3. Promote root Forgejo process helpers, config/credential builders, and binary
   discovery into `temper-testing` so new live capstones do not copy wiring.
4. Add shared `file://` git fixtures for worker and daemon rehearsals.
5. Add explicit nextest groups or naming conventions for every live category if
   ignored provider smoke tests grow beyond self-skipping probes.
6. Resolve issue #165 by adding a real-worker `temper-sim` harness now that the
   reusable `temper-daemon-transport` glue provides the co-resident daemon
   carrier outside CLI internals.
7. Set `TEMPER_SIM_SEED_BASE` in CI so deterministic chaos explores new seeds
   while still printing reproducible failures.
8. Consider a repo-level web validation alias/script if agents keep missing
   `npm test` and `npm run build`.
