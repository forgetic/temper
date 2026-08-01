# Write Temper tests

Use this guide when adding or changing tests. For the full rationale and the
current inventory, see [Testing pyramid](../reference/testing-pyramid.md) and
[Test inventory](../reference/testing-inventory.md).

## Pick the cheapest high-confidence layer

1. **Pure/unit first.** Put parser, mapper, DTO, config, prompt, reducer, and
   local state-machine checks next to the code in `src` test modules.
2. **Contract next.** For Forge trait semantics or provider request mapping,
   add backend contract tests under the backend crate's `tests/` directory.
   Use mock HTTP clients for Forgejo/GitHub request shapes.
3. **Hermetic integration by default.** For workflow, engine, runner, worker,
   and agent behavior, prefer MemoryForge or filesystem forges, in-process
   transports, deterministic fake agents, Jig fake LLMs, and local `file://`
   git.
4. **Simulation for time and races.** Use `temper-sim` when behavior depends on
   timers, long-poll interleavings, retries, cancellation, or chaos. It has two
   worker fidelities: hand-rolled protocol simulants for cheap HTTP-path and
   misbehavior scenarios, and a real `WorkerMachine`/`WorkerShell` harness over
   `temper-daemon-transport::InProcessTransport` for production worker-loop
   coverage under `LabRuntime`.
5. **Live e2e last.** Add ignored Forgejo/provider tests only for real webhooks,
   git auth, host-mode Actions CI, binary wiring, close-on-merge, or real
   provider credentials that no hermetic layer can prove.

These Rust layers are lower-level product coverage, not execution modes for
`temper-scenario`. Feature-landing scenario evidence comes from the implicit
real-Forgejo/host-runner/standalone-Temper/Jig topology.

## Assertion ownership quick reference

- **Default hermetic real-stack tests** own behavior that can be proven with a
  memory/filesystem forge, local `file://` git, in-process daemon/worker
  transport, deterministic fake agents, or a jig fake LLM. Put implementation-PR
  handoff, retry/idempotency, role routing, worker protocol, and PR-body
  assertions here before reaching for live Forgejo.
- **Deterministic `temper-sim` tests** own time, scheduling, retries,
  cancellation, long-poll interleavings, and race/chaos cases. If the proposed
  assertion would otherwise need sleeps or wall-clock timing, make it a sim.
- **Slim live Forgejo capstones** own only the residual routine proofs that the
  hermetic stack cannot supply: real webhooks, real git auth, host-mode Actions
  CI, close-on-merge, and installed binary/config wiring that should gate
  `cargo dev-test-full`.
- **Manual/all-e2e live tests** own exhaustive or diagnostic live coverage:
  lower-level fixture smokes, provider/OAuth probes, provisioning edge cases,
  redundant root Forgejo stories, and environment-sensitive scenarios. Add new
  live tests here first; promote to the capstone list only with a distinct
  routine-gating risk.

## Where to put the test

- Workflow validation/planning/execution/recovery:
  `crates/temper-workflow/tests/`.
- Engine appliers, feeds, webhook route, role routing, worker protocol:
  `crates/temper-engine/tests/`.
- Runner scans, role workers, mechanical workers, fake CI/fake agents:
  `crates/temper-runner/tests/`, with reusable pieces in `temper-testing`.
- Worker executor, workspace, local git, out-of-process fake agent:
  `crates/temper-worker/tests/`.
- Agent loop and LLM protocol: sans-IO machine tests, jig fake LLMs, or ignored
  provider/request-oracle tests when real credentials are required.
- Web UI: reducer/feed contract first, happy-dom only for rendered interaction.
- Root `tests/`: root binary wiring or full live composition only.

## Rules

- Test durable behavior: Forge state, workflow decisions, protocol messages,
  pushed SHAs, rendered DOM, or operator-visible artifacts.
- Do not re-prove workflow logic in live Forgejo; prove it hermetically and keep
  live scenarios narrow.
- Use fixed timestamps, seeded randomness, tempdirs, fake tokens, and cleanup
  guards. Avoid sleeps unless the test is explicitly about wall-clock behavior.
- Default tests must not read real user config, credentials, or ambient env.
- Live/network/provider tests must be `#[ignore]`, self-skip without opt-in env,
  and never print secrets.
- New heavyweight root Forgejo e2es must use the root e2e lock and nextest group
  unless they are proven lightweight. Add them to the manual/all-e2e lane first;
  promote a test to the default capstone list only when it protects a distinct
  real-stack risk that should run in `cargo dev-test-full`.
- Prefer shared helpers in `temper-testing` when a setup pattern appears in more
  than one crate; keep one-off helpers local.

## Commands

Targeted Rust:

```sh
cargo test -p <crate> <test_name_or_filter>
cargo nextest run -p <crate> --test <target>
```

Workspace quick path:

```sh
cargo dev-check
cargo dev-test-quick
```

Before handoff when Rust behavior changed:

```sh
cargo dev-fmt
cargo dev-clippy
cargo dev-check
cargo dev-test-quick
```

Ignored live lanes:

```sh
cargo dev-test-e2e-capstones  # default live capstones; also run by dev-test-full
cargo dev-test-e2e-all        # every ignored/manual live test
```

Web UI:

```sh
npm --prefix crates/temper-web/ui test
npm --prefix crates/temper-web/ui run typecheck
npm --prefix crates/temper-web/ui run build
```
