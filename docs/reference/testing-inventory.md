# Test inventory

This is a static source inventory from 2026-06-25. It counts Rust
`#[test]`/`#[tokio::test]` functions under `crates/` and `tests/`, plus
Vitest `it(...)` cases under `crates/temper-web/ui/test/`. Counts are a map of
where confidence comes from, not a coverage target.

## By pyramid layer

| Layer | Tests | Ignored | Test files | Main homes |
| --- | ---: | ---: | ---: | --- |
| Unit / component | 942 | 0 | 193 | `src` test modules in every crate |
| Backend contract / integration | 271 | 0 | 37 | forge backend `tests/` dirs |
| Hermetic workflow / role | 450 | 0 | 79 | workflow/engine/runner/worker |
| Hermetic scenario | 19 | 0 | 4 | memory-forge delivery worlds |
| Hermetic process e2e | 5 | 0 | 2 | real daemon/fake agent |
| Simulation / machine | 27 | 0 | 8 | sim and machine tests |
| UI model/feed | 44 | 0 | 3 | web UI reducers/contracts |
| UI DOM/component | 33 | 0 | 3 | happy-dom component tests |
| Crate integration | 172 | 0 | 39 | crate-local public API seams |
| Live Forgejo e2e | 6 | 6 | 4 | root ignored scenarios |
| Live Forgejo preflight | 10 | 10 | 8 | fixture/provider smoke |
| Live provider smoke | 3 | 3 | 3 | real OAuth/provider request checks |

Total: 1,982 test cases across 383 files; 19 are ignored live tests.

## By package

| Package | Tests | Ignored | Test files |
| --- | ---: | ---: | ---: |
| `temper-workflow` | 258 | 0 | 37 |
| `temper-forge-forgejo` | 186 | 1 | 34 |
| `temper-engine` | 147 | 0 | 31 |
| `temper-agent` | 143 | 3 | 24 |
| `temper-runner` | 121 | 0 | 28 |
| `temper-web` | 109 | 0 | 19 |
| `temper-forge-github` | 98 | 0 | 20 |
| `temper-testing` | 96 | 6 | 25 |
| `temper-worker` | 86 | 0 | 21 |
| `temper-forge-filesystem` | 79 | 0 | 14 |
| `temper-web-ui` | 77 | 0 | 6 |
| `temper-config` | 66 | 0 | 7 |
| `temper-log` | 50 | 0 | 11 |
| `temper-cli-daemon` | 47 | 0 | 5 |
| `temper-worker-registry` | 42 | 0 | 3 |
| `temper-cli-init` | 39 | 0 | 6 |
| `temper-interaction` | 34 | 0 | 8 |
| `temper-agent-core` | 30 | 0 | 7 |
| `temper-forge-memory` | 27 | 0 | 4 |
| `temper-engine-io` | 26 | 0 | 6 |
| `temper-sim` | 23 | 0 | 6 |
| `temper-provision-forgejo-cli` | 20 | 3 | 5 |
| root `temper` package | 19 | 6 | 9 |
| `temper-agent-io` | 16 | 0 | 7 |
| `temper-worker-io` | 16 | 0 | 7 |
| `temper-protocol-worker` | 14 | 0 | 4 |
| `temper-agent-session` | 13 | 0 | 6 |
| `temper-interaction-service` | 13 | 0 | 5 |
| `temper-reference-delivery` | 13 | 0 | 2 |
| `temper-cli-common` | 12 | 0 | 2 |
| `temper-cli` | 11 | 0 | 2 |
| `temper-trigger-forgejo` | 11 | 0 | 2 |
| `temper-protocol-agent` | 9 | 0 | 1 |
| `temper-provision` | 9 | 0 | 2 |
| `temper-cli-config` | 6 | 0 | 1 |
| `temper-forge-model` | 4 | 0 | 1 |
| `temper-wake` | 4 | 0 | 1 |
| `temper-protocol-interaction` | 3 | 0 | 1 |
| `temper-reference-delivery-validator` | 3 | 0 | 1 |
| `temper-engine-service` | 2 | 0 | 2 |

## High-value suite families

- Forge contracts: memory, filesystem, Forgejo, and GitHub backend tests cover
  common trait semantics plus provider-specific request mapping.
- Workflow runtime: `crates/temper-workflow/tests/` owns validation,
  planning, gates, leases, reconciliation, crash/recovery, and safety rules.
- Engine/runner/worker: `crates/temper-engine/tests/`,
  `crates/temper-runner/tests/`, and `crates/temper-worker/tests/` exercise
  role feeds, appliers, worker protocol, local git workspaces, and fake agents.
- Simulation: `crates/temper-sim/tests/` and the machine-sim tests run real
  production machines under deterministic scheduling and virtual time. Issue
  #165 tracks replacing the sim's hand-rolled worker client with the real
  `WorkerMachine`/`WorkerShell` over in-process transport.
- Web UI: Rust server/read-model tests plus Vitest reducer, feed-contract, and
  happy-dom component tests protect both event shapes and rendered behavior.
- Live capstones: ignored root tests prove real Forgejo, real git, real
  webhooks, real host-mode Actions CI, and real `temper` binaries.

## Ignored live suites

Run the calibrated Forgejo set with `cargo dev-test-e2e`; it also executes the
provider smoke tests, which must self-skip unless their opt-in env is present.

- `tests/basic_delivery_forgejo_e2e.rs` — full `temper init` + standalone run,
  fake LLM, real Forgejo, real Actions, and merge.
- `tests/daemon_forgejo_e2e.rs` — daemon binary + deterministic wire worker,
  happy path and CI red-then-green.
- `tests/init_forgejo_e2e.rs` — `temper init --apply` local artifacts,
  live Forgejo state, idempotency, and daemon boot.
- `tests/run_forgejo_e2e.rs` — single-process `temper run` with fake LLM,
  checkpointed PR handoff, and provider-failure retry.
- `crates/temper-testing/tests/forgejo_*.rs` — server, runner, provision,
  PR-prep, CI web-UI, and parallel-fixture preflights.
- `crates/temper-forge-forgejo/tests/live.rs` — provider smoke against a live
  Forgejo fixture.
- `crates/temper-provision-forgejo-cli/tests/existing_repo_access.rs` — live
  existing-repository provisioning cases.
- `crates/temper-agent/tests/*oauth_live.rs` and
  `crates/temper-agent/tests/jig_request_oracle.rs` — real provider/OAuth
  checks, each gated by an explicit environment variable.
