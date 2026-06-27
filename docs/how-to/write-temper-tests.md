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
3. **Hermetic scenario by default.** For workflow, engine, runner, worker, and
   agent behavior, prefer memory/filesystem forges, in-process transports,
   deterministic fake agents, jig fake LLMs, and local `file://` git.
4. **Simulation for time and races.** Use `temper-sim` when behavior depends on
   timers, long-poll interleavings, retries, cancellation, or chaos. Issue #165
   tracks the next fidelity improvement here: driving the real `WorkerMachine`
   and `WorkerShell` over `InProcessTransport` under `LabRuntime`.
5. **Live e2e last.** Add ignored Forgejo/provider tests only for real webhooks,
   git auth, host-mode Actions CI, binary wiring, close-on-merge, or real
   provider credentials that no hermetic layer can prove.

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
