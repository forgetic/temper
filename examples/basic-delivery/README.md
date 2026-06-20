# Basic-delivery example

A **deliberately minimal**, no-human-in-the-loop Temper demo: it drives a single,
thin intake issue **from submission to a merged PR with nobody in the loop**.
The architect and engineer phases are real Temper coding-agent phases, but the
LLM backend is a local `jig` fake provider loaded with a scripted
`fixtures/basic-delivery.json` fixture, so the happy path needs no real
provider credentials or token spend.

It is the "happy path, nothing fancy" counterpart to
[`examples/reference-delivery/`](../reference-delivery/): **one** repo, the
human-capable workflow roles `architect` and `engineer`, the `bot` mechanical
automation authority, CI, webhooks on from the start, and landing gated on
**CI alone** — no reviewer, no owner, no human.

## What it demonstrates

The run now follows the long-term local developer UX:

1. `run.sh` boots throwaway Forgejo + a host-mode `forgejo-runner`.
2. It starts a local jig server from `~/src/rust/jig` by default, using
   `fixtures/basic-delivery.json`, captures the printed base URL, and passes that
   to Temper as a DeepSeek-compatible provider URL. Jig ignores the dummy API key
   stored by init.
3. It creates the Forgejo site admin, then runs `temper init --non-interactive`
   with `--forge`, `--repo`, `--bind`, `--admin-user`, `--provider deepseek`,
   `--provider-url`, `--config`, and `--secrets`. Init provisions the empty repo,
   labels, webhook, and writes `run/config.toml`, `run/credentials.toml`,
   `run/workflow.json`, and `run/webhook-secret`.
4. Because init no longer seeds project content, `run.sh` creates the initial
   default-branch commit explicitly. That commit contains only a small
   `README.md` and `.forgejo/workflows/ci.yml` copied from `config/ci.yml`.
5. `run.sh` launches `temper serve standalone --config run/config.toml
   --credentials run/credentials.toml`. One process hosts the daemon, webhook
   route, poll/mechanical backstops, worker, and jig-backed coding agent.
6. Only after serve-standalone readiness, `run.sh` uses the site-admin token to
   `POST /api/v1/repos/{owner}/{repo}/issues` and file **one unlabeled intake
   issue**. The issue-created webhook is the demonstrated wake path.
7. The architect rewrites the thin intake into a ready code issue, the engineer
   opens a PR, the real Forgejo runner runs CI, and the bot auto-merges when CI
   is green.

No reviewer approves; no owner or human acts. The bot is the **sole landing
authority** and lands purely on CI.

## What is real vs. scripted

- **Real:** Forgejo, host-mode runner CI, `temper init`, explicit git repo
  population, the direct Forgejo issue-create API call, the standalone daemon,
  webhooks, leases, per-role apply tokens, worker workspaces, PR creation, CI
  reads, and mechanical landing.
- **Scripted:** only the LLM replies served by jig and the thin intake body in
  `config/intake-issue.md`.

## Prerequisites

- Rust workspace build tools.
- `curl`, `git`, `mkfifo`, and Python 3.
- A local jig checkout at `~/src/rust/jig` by default, with
  `fixtures/basic-delivery.json`. Override `JIG_REPO`, `JIG_BIN`, or
  `JIG_FIXTURE` in `config/temper.env` if needed.
- The pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries, either in
  the shared `.cache/forgejo/` cache or through `TEMPER_FORGEJO_BINARY` and
  `TEMPER_FORGEJO_RUNNER_BINARY`.
- A host where the runner may execute host-mode jobs directly. No containers.

`run.sh` builds the needed development-profile binaries by default with
`cargo build -p temper` and, unless `JIG_BIN` is set or `TEMPER_SKIP_JIG_BUILD=1`,
`cargo build -p jig` in the jig checkout.

## Layout

```text
examples/basic-delivery/
├── README.md
├── config/
│   ├── temper.env       # non-secret knobs
│   ├── workflow.json    # canonical basic-delivery spec copy for comparison
│   ├── intake-issue.md  # deliberately thin seed intake body
│   └── ci.yml           # CI committed explicitly into the demo repo
├── secrets/
│   └── .env.example     # legacy/local override template; not needed for jig
└── run.sh               # launcher / teardown / validators
```

Runtime data goes under gitignored `run/` and `logs/`. The init-emitted
`credentials.toml` is in `run/` and is removed during teardown.

## Quick start

From this directory:

```sh
./run.sh start                  # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh validate-webhooks      # while the run is still alive
./run.sh stop
```

Progress is printed without secrets (server URL, jig URL, seeded issue URL, and
where logs live). The default `BASE_URL` (`http://127.0.0.1:4100`) and
`DAEMON_BIND` (`127.0.0.1:38100`) are distinct from reference-delivery.

## Useful knobs

Edit `config/temper.env` or export variables before launch:

- `OWNER=acme` / `NAME=service` — the single repo initialized and scanned.
- `JIG_REPO=$HOME/src/rust/jig` — jig checkout used by default.
- `JIG_BIN=` — optional prebuilt jig binary.
- `JIG_FIXTURE=fixtures/basic-delivery.json` — fixture passed to jig.
- `DAEMON_POLL_CADENCE_SECS=120` — long poll backstop; webhooks drive prompt
  progress.
- `DAEMON_MECHANICAL_CADENCE_SECS=2` — mechanical CI/landing reconciliation
  cadence.
- `RUN_MAX_ITERATIONS=250` — max agent iterations per job.

## Validation and troubleshooting

Run the validator before teardown; `./run.sh stop` removes the throwaway Forgejo
state and init-emitted credentials.

```sh
./run.sh validate-webhooks
```

It checks `logs/provision.log` (init/webhook/initial-commit/intake records) and
`logs/run.log` (serve-standalone readiness, webhook delivery + wake scan, daemon
assignments/results, worker register/assign/result lines, and CI-read fallback
diagnostics).

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
pkill -f 'target/debug/jig'
rm -rf examples/basic-delivery/run
```
