# Reference-delivery example

A **self-contained Temper demo** of the production daemon/worker topology with the
**real in-process coding agent**: it boots a throwaway Forgejo server, registers a
real host-mode `forgejo-runner`, and drives the reference-delivery workflow from
intake to a merged green PR with a single `temper run` process. It is the richer
counterpart to [`examples/basic-delivery/`](../basic-delivery/): it adds a
**reviewer approval gate** to basic-delivery's architect + engineer, and (in
multi-repo mode) demonstrates **cross-repo fan-out** — one parent intake materialised
into one child code issue per target repo.

## What this example proves

- Forgejo is the durable workflow state store: issues, labels, PRs, reviews,
  dependencies, CI status, and metadata blocks carry state.
- CI is real: the bundled `forgejo-runner` runs the checked-in host-mode workflow
  on this machine.
- The whole orchestration is **one process**: `temper run` hosts the daemon, one
  worker, and the real coding agent on a single event loop. Architect triage,
  engineer implementation, and reviewer approval are real LLM work.
- Webhooks are real wake hints: `temper run` owns the Forgejo webhook route
  (`POST /forgejo/webhook`); a delivery drives a targeted wake scan instead of
  waiting on the long poll backstop, which remains the liveness correctness
  backstop.

## What is real vs. canned

- **Real:** the Forgejo server + host-mode `forgejo-runner` and its CI, the
  provisioning, the daemon's Forge API authority (webhooks, leases, per-role apply
  tokens, cross-repo child materialisation, mechanical landing), the worker's git
  workspaces, and the **coding agent** (architect / engineer / reviewer are real
  LLM work).
- **Canned:** only the seed intake body (`config/intake-issue.md`, or the
  generated cross-repo parent body).

The coding agent's LLM provider is selected with `TEMPER_RUN_AUTH` in
`config/temper.env` (default `chatgpt-oauth`); provider credentials live in
`secrets/.env` (gitignored). See `secrets/.env.example`.

## Prerequisites

- Rust workspace build tools.
- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries, either in the
  shared `.cache/forgejo/` cache or through `TEMPER_FORGEJO_BINARY` and
  `TEMPER_FORGEJO_RUNNER_BINARY` in `config/temper.env` or the environment.
  Ignored Forgejo fixture tests fill the shared cache automatically on first
  startup when network access is available.
- A host where the runner may execute host-mode jobs directly. No containers.
- Provider credentials for the coding agent (see `secrets/.env.example`).

`run.sh` builds the needed development-profile binaries by default with
`cargo build -p temper`: the unified `temper` binary, `temper-provision-forgejo`,
and `temper-validate-reference-delivery`. Set `TEMPER_SKIP_BUILD=1` only when those
paths are already current.

## Layout

```text
examples/reference-delivery/
├── README.md
├── observability.md
├── config/
│   ├── temper.env       # non-secret knobs
│   ├── workflow.json    # the reference-delivery workflow spec
│   ├── intake-issue.md  # the thin seed intake body (single-repo mode)
│   └── ci.yml           # host-mode pass-through CI (validates the engineer's
│                        #   real product diff)
├── secrets/
│   └── .env.example     # provider-auth + local-override template
└── run.sh               # launcher / teardown / validators
```

Runtime data goes under gitignored `run/`, `logs/`, and `secrets/credentials.toml`
(the provisioned forge identities in the runtime's own credentials format, loaded
via `temper daemon --secrets`). The single process logs to `logs/run.log`.

## Quick start

From this directory:

```sh
./run.sh start                  # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh validate-multi-repo    # while the run is still alive
./run.sh validate-webhooks
./run.sh stop
```

The default repo set is:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
```

That seeds one parent intake in `acme/service`; the architect fans it out into one
child code issue per configured repo. The default `BASE_URL`
(`http://127.0.0.1:4200`) and `DAEMON_BIND` (`127.0.0.1:38200`) are **distinct from
basic-delivery** (`4100` / `38100`), so both demos can run side by side.

## Expected flow

1. `run.sh` provisions each repo with labels, CI, role users/tokens, and a repo
   webhook (`--seed-intake no`, so intake is held back).
2. `run.sh` starts **one `temper run`** serving architect + engineer + reviewer
   across the configured repos, then files intake last so its creation webhook
   wakes a ready system.
3. The architect triages the intake (single-repo: rewrites the body to a code
   spec; cross-repo: fans it out into one child code issue per configured repo and
   blocks the parent on those children).
4. The engineer opens real implementation PRs (created with `implementation` +
   `needs-reviewer`).
5. The real `forgejo-runner` runs CI for each PR head.
6. The reviewer approves PRs.
7. The mechanical backstop lands PRs after reviewer approval and current-head CI.
   It runs as the provisioned `bot` automation user: the bot's token performs REST
   merges and the bot's web-UI login reads Forgejo 7.0.x CI status (ADR 0019). The
   setup-only site admin is not involved, and the owner role does not perform
   normal merges.
8. If a merge conflict is reported, mechanical automation removes `landing`, adds
   `merge-conflict`, and the engineer requeues after updating the PR head; fresh CI
   is still required, but a second review is not.
9. (Cross-repo) once each child PR merges and its code issue closes, the parent
   dependency aggregate unblocks and resolves.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `REPOS="owner/a owner/b"` — repo set served by the run.
- `CROSS_REPO_INTAKE=auto|1|0` — one parent fan-out issue or independent intakes.
- `SERVED_ROLES="architect engineer reviewer"` — the workflow roles the run serves.
- `TEMPER_RUN_AUTH=chatgpt-oauth|anthropic-oauth|deepseek` — the coding agent's LLM
  provider/auth.
- `RUN_MAX_ITERATIONS=250` — max agent iterations per job.
- `DAEMON_POLL_CADENCE_SECS=120` — long poll backstop; webhooks drive prompt
  progress. **Do not shorten this** to compensate for webhooks.
- `DAEMON_MECHANICAL_CADENCE_SECS=2` — mechanical CI/landing reconciliation cadence.

## Validation and troubleshooting

Run validators before teardown; `./run.sh stop` removes the throwaway Forgejo
state.

```sh
./run.sh validate-webhooks
./run.sh validate-multi-repo
```

The multi-repo validator checks provisioning logs, the unified `logs/run.log`
(serving readiness, webhook delivery + wake scans, daemon assignments and results,
the in-process worker lifecycle), mechanical CI-read diagnostics, and live Forge
state for the parent/child dependency shape. See `observability.md` for log
contents and expected event trails. A PR stuck with `landing` usually lacks
current-head CI or reviewer approval; if `logs/run.log` mentions missing web-UI
credentials for the CI read fallback, the run was not launched with the provisioned
`bot` username/password. A PR stuck with `merge-conflict` needs an engineer
conflict-resolution push before mechanical landing retries.

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
rm -rf examples/reference-delivery/run
```

## Related checks

Live Forgejo + runner daemon-topology e2e (see
[run-daemon-e2e.md](../../docs/how-to/run-daemon-e2e.md)):

```sh
# both daemon scenarios
cargo test --test daemon_forgejo_e2e -- --ignored

# the single-process temper run path (one daemon + worker + agent)
cargo test --test run_forgejo_e2e -- --ignored
```
