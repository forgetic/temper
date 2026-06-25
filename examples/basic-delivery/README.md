# Basic-delivery example

A deliberately minimal, no-human-in-the-loop Temper demo: one thin intake issue
moves from submission to a merged PR with nobody acting manually. The architect
and engineer phases are real Temper coding-agent phases, but the LLM backend is
a local `jig` fake provider loaded with `fixtures/basic-delivery.json`, so the
happy path needs no real provider credentials or token spend.

This is the fixed, happy-path counterpart to
[`examples/reference-delivery/`](../reference-delivery/): one repo, the
human-capable workflow roles `architect` and `engineer`, the `bot` mechanical
automation authority, CI, webhooks on from the start, and landing gated on CI
alone.

## What it demonstrates

1. `run.sh` boots throwaway Forgejo + a host-mode `forgejo-runner`.
2. It starts local jig from `~/src/rust/jig` with
   `fixtures/basic-delivery.json` and uses the printed URL as Temper's
   DeepSeek-compatible provider URL.
3. It creates the Forgejo site admin, then runs
   `temper --config run init --non-interactive --apply --yes`
   with the fixed repo, forge URL, bind address, admin user, and provider. The explicit
   `run/` bundle supplies both `config.toml` and sibling `credentials.toml`; the
   apply step provisions the empty repo, labels, webhook, and writes
   `run/config.toml`, `run/credentials.toml`, `run/workflow.yaml`, and
   `run/webhook-secret`.
4. Because init does not seed project content, `run.sh` creates the initial
   default-branch commit explicitly: a tiny `README.md` plus
   `.forgejo/workflows/ci.yml` copied from `config/ci.yml`.
5. `run.sh` launches
   `temper --config run serve standalone`.
6. After standalone readiness, `run.sh` uses the site-admin token to file one
   unlabeled intake issue. That issue-created webhook is the demonstrated wake
   path.
7. The architect rewrites the thin intake into a ready code issue, the engineer
   opens a PR, real Forgejo Actions CI runs, and the bot auto-merges when CI is
   green.

## Prerequisites

- Rust workspace build tools.
- `curl`, `git`, `mkfifo`, and Python 3.
- A local jig checkout at `~/src/rust/jig` with
  `fixtures/basic-delivery.json`.
- Pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries in
  `.cache/forgejo/`.
- A host where the runner may execute host-mode jobs directly. No containers.

`run.sh` always builds the development-profile `temper` binary and the local jig
binary before starting.

## Layout

```text
examples/basic-delivery/
├── README.md
├── config/
│   ├── workflow.json    # canonical basic-delivery spec copy for comparison
│   ├── intake-issue.md  # deliberately thin seed intake body
│   └── ci.yml           # CI committed explicitly into the demo repo
├── observability.md
└── run.sh               # fixed launcher / teardown
```

Runtime data goes under gitignored `run/` and `logs/`. The init-emitted
`credentials.toml` is in `run/` and is removed during teardown.

## Quick start

From this directory:

```sh
./run.sh start                  # blocks until Ctrl-C, ./run.sh stop, or RUN_SECS
./run.sh stop
```

The launcher intentionally has no editable config file. Repo, ports, cadences,
binary paths, and the jig fixture are fixed in `run.sh` to keep the happy path
short and auditable.

## Troubleshooting

`./run.sh stop` removes the throwaway Forgejo state and init-emitted
credentials, but logs are retained. Inspect `logs/provision.log` and
`logs/run.log` for init/webhook records, standalone readiness, webhook delivery
and wake scan, job assignment/result lines, worker assignment/result lines, and
CI-read fallback diagnostics.

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
pkill -f 'target/debug/jig'
rm -rf examples/basic-delivery/run
```
