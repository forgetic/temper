# Daemon e2e: topology and real CI

This explains the durable design of the daemon-topology end-to-end suite
(`tests/daemon_forgejo_e2e.rs`) — the real-backend proof of the consolidated
daemon/worker split. The how-to for running it is
[run-daemon-e2e.md](../how-to/run-daemon-e2e.md); the CI-read decision is
[ADR 0019](../adr/0019-forgejo-ci-read-via-web-ui.md); the worker/daemon
boundary is [worker-daemon-wire-protocol.md](../reference/worker-daemon-wire-protocol.md).

## What the suite proves (and what it does not)

The hermetic `crates/temper-daemon/tests/` suite already covers the daemon
*logic* — scheduling, lease CAS, failure-class policy, idempotent applies,
webhook verify/parse, backpressure, role routing — over a memory forge and
in-process HTTP. The e2e suite exists for exactly the residual real-wiring
value the hermetic suite cannot give:

1. **Real-wiring proof**: real Forgejo API behavior, real webhook delivery,
   real git push auth, and the real `temper-daemon` binary's
   env→config→composition (`FORGEJO_*`, `TEMPER_FORGEJO_TOKEN_<ROLE>`,
   CLI flags).
2. **Real CI red→green**: mechanical merge-on-green against real Actions
   verdicts — including *not* merging while CI is red.

It deliberately does **not** re-prove workflow logic; scenarios are minimal.

## Process topology

Two ignored tests; each owns the live world for one scenario:

```text
  ┌────────────────────────────────────────────────────────────┐
  │ Forgejo server (SQLite, ephemeral port, kill-on-drop)      │
  │   REST /api/v1 · web UI (CI reads) · Actions               │
  └────────────────────────────────────────────────────────────┘
     ▲ API (daemon only)   │ webhooks        ▲ git push       ▲ runs CI
     │                     ▼                 │ (role token)   │
  ┌──┴──────────────────────────┐   ┌────────┴───────────┐ ┌──┴─────────────┐
  │ temper-daemon binary        │   │ temper-testing-    │ │ forgejo-runner │
  │ (webhook route, poll +      │◄──┤ daemon-worker      │ │ (host mode)    │
  │  mechanical backstops,      │   │ (wire-protocol     │ └────────────────┘
  │  lease-gated role-routed    │   │  client + real git │
  │  appliers)                  │   │  push)             │
  └─────────────────────────────┘   └────────────────────┘
```

- **Server and runner** come from the shared `bench-forgejo` fixture
  (re-exported as `temper_testing::forgejo_server`): one throwaway Forgejo per
  scenario with Actions enabled, plus one real host-mode `forgejo-runner`
  (`--labels host:host`, no containers) — the genuine CI producer.
- **Daemon**: the real root-package `temper-daemon` binary on an ephemeral
  port. It is the only component holding Forge API credentials: forge URL and
  admin/bot token from its config/credentials files (`[forge] url` and the
  `[forge.users.<admin>]` token), web-UI credentials for the ADR 0019 CI reads,
  and the provisioned engineer token via `TEMPER_FORGEJO_TOKEN_ENGINEER` for
  role-attributed applies.
- **Worker**: `temper-testing-daemon-worker`, a deterministic Worker/Daemon
  Wire Protocol v1 client that stands in for `smith-worker`. It long-polls the
  daemon, and on assignment clones/fetches the repo over the real git remote,
  commits one deterministic change file as the engineer git identity, pushes
  the hinted branch, and reports `result(success, branch+head_sha)`. It never
  touches the Forge API. The worker lives in `temper-testing` so temper's CI
  stays self-contained; the smith pairing is covered by `smith-worker`'s
  hermetic fake-daemon contract tests.
- **Webhook**: the repo webhook posts directly to the daemon's
  `/forgejo/webhook` route (HMAC-verified); every verified delivery triggers a
  wake scan of the configured repo/role feeds.

## Scenario workflow

The daemon runs the engineer-only **daemon-delivery** workflow
(`tests/support/daemon-delivery.json`, the dogfood deployment shape):

- the mechanical `raw_intake` automation stamps a seeded unlabeled intake
  issue `code` + `ready` (there is no architect triage tier in the daemon
  topology — production work is filed engineer-ready),
- the engineer's `open_pr` produces the implementation PR (the daemon's
  `ForgeApplier` opens it from the worker's pushed branch, as the engineer
  identity, with the deterministic `pr-for-code-<N>` correlation key),
- the mechanical `land_pr` automation merges once the `ci_gate` passes.

The source issue closes on merge through the provider's native
close-on-merge keyword: the worker's commit message carries `Closes #<N>`, so
landing the merge commit on the default branch closes the issue. The daemon
topology has no role that closes issues via the API, and the e2e asserts this
real provider wiring.

## Identity is per-token

Forgejo identity **is the access token**: the e2e asserts the implementation
PR is *authored by the engineer role identity*, which proves the daemon's
per-role token routing (`RoleRoutingApplier` + per-role
`LeaseApplier→ForgeApplier` chains) through a real API, while the merge is
performed by the daemon's default (admin/bot) identity through the mechanical
backstop. Git pushes authenticate separately with the engineer token over
`http.extraheader`, mirroring the production worker tier's credential split:
the daemon holds Forge API credentials, the worker holds git credentials.

## Provisioning boundary

Provisioning is unchanged from the fixture: `provision_role_identities`
creates org + per-role users/tokens once, `provision_repository` creates the
`auto_init` repo, labels, and the marker-gated CI workflow per repository, and
`seed_intake_issue` files the realistic entry issue idempotently. The
production `temper-provision-forgejo` binary shares this code path; see
[forgejo-e2e-fixture.md](../reference/forgejo-e2e-fixture.md).

## Real CI: producing and reading

**Producing.** The provisioned repo commits `.forgejo/workflows/ci.yml`
(`runs-on: host`). Because the host runner has no offline `actions/checkout`,
the CI-pass gate keys on a **commit-message marker** (`[ci-pass]`) rather than
a checked-out file. The worker's `--ci-sentinel` knob controls whether its
commit carries the marker: `present` gives the happy path an immediately green
head; `deferred` gives the red→green scenario a failing first head SHA and a
later passing fix SHA — two real verdicts.

**Reading.** Forgejo 7.0.12 lacks Actions run/task REST endpoints, so
`list_ci_jobs` is REST-first with a **password/web-UI fallback** (ADR 0019).
The daemon's mechanical backstop performs these reads with the web-UI
credentials passed through its environment, exactly as the production
launcher wires them.

## Triggering status

Each scenario is webhook-driven: real Forgejo posts to the daemon's webhook
route and every verified delivery wake-scans the configured feeds. The
daemon's normal poll backstop is deliberately long (600 s) and the tests
assert convergence before it, so webhook wakes must carry all Forge-event
progress. Forgejo 7.0.x does not surface Actions completion as a repo
webhook, so the daemon's short mechanical cadence is the CI-verdict backstop
(the successor of the legacy fleet's narrow CI status poll). Polling remains
the correctness backstop; webhook payloads are hints only and every wake runs
a fresh Forge scan.

## What replaced the legacy fleet e2e

This suite replaced the per-role `temper-testing-worker` fleet topology
(webhook trigger process + Unix wake sockets + one OS process per role) and
its e2e targets (`forgejo_multiprocess`, `forgejo_webhook_wakeup`,
`forgejo_multi_repo_webhook`, `forgejo_worker`, `multiprocess`,
`multi_repo_multiprocess`). The workflow logic those scenarios exercised is
covered hermetically (`crates/temper-daemon/tests/`, the launcher-static
tests, and `basic_delivery_fakes`); the topology they exercised is obsolete
after the daemon/worker consolidation. The topology-agnostic fixture proofs
(`forgejo_server`, `forgejo_runner`, `forgejo_provision`, `forgejo_pr_prep`,
`forgejo_ci_web_ui`, `forgejo_workspace_pr`, `forgejo_parallel`) remain.

## Why it stays `#[ignore]`d

It boots real OS processes, executes CI **on the host**, and detects
convergence by wall-clock polling. Like the `temper-forge-forgejo` live test
it is `#[ignore]`d, so default `cargo test` stays hermetic and deterministic
(the repo's `cargo dev-test-full` alias includes it).

Ignored startup may download pinned binaries into `.cache/forgejo/` when
explicit overrides and cached files are absent. Binary and state caches are
process-safe, and each test gets unique runtime paths.
