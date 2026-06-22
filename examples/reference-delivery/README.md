# Reference-delivery example

A self-contained Temper demo of the reviewer-gated delivery workflow. The default
path boots a throwaway Forgejo server, registers a real host-mode
`forgejo-runner`, starts a deterministic local jig LLM, then drives one issue
from intake to a merged PR via `temper serve standalone`.

The example also has an explicit cross-repo fan-out path: `./run.sh multi-repo`
provisions `acme/service` plus `acme/service-canary`, files one parent intake in
the source repo, and uses deterministic workers to fan out, review, test, and
land child implementation PRs in both repositories.

This is the richer counterpart to [`examples/basic-delivery/`](../basic-delivery/):
the default path uses the same long-term UX commands (`temper --config ... init
--apply --yes` and `temper --config ... serve standalone`) but selects
`config/workflow.json`, whose happy path adds a reviewer approval gate between
the engineer PR and bot landing.

## What the default demo demonstrates

1. `run.sh start` boots throwaway Forgejo + a host-mode `forgejo-runner`.
2. It starts local jig from `~/src/rust/jig` with
   `fixtures/reference-delivery.json` and uses the printed URL as Temper's
   DeepSeek-compatible provider URL.
3. It creates the Forgejo site admin, then runs
   `temper --config run/config.toml --secrets run/credentials.toml init --non-interactive --apply --yes --workflow config/workflow.json`
   with fixed repo, forge URL, bind address, admin user, and provider.
4. Because init does not seed project content, `run.sh` creates the initial
   default-branch commit explicitly: a tiny `README.md` plus
   `.forgejo/workflows/ci.yml` copied from `config/ci.yml`.
5. `run.sh` launches
   `temper --config run/config.toml --secrets run/credentials.toml serve standalone`.
6. After standalone readiness, `run.sh` files one unlabeled intake issue. That
   issue-created webhook is the demonstrated wake path.
7. The architect rewrites intake into a ready code issue, the engineer opens a
   PR, real Forgejo Actions CI runs, the reviewer approves, and the bot merges
   when review + CI are green.

## What the multi-repo demo demonstrates

1. `run.sh multi-repo` boots the same throwaway Forgejo + runner.
2. It provisions exactly `acme/service` and `acme/service-canary` with the
   reference workflow labels, role users, credentials, and CI seed commits.
3. It starts deterministic Forgejo-backed `temper-testing-worker` processes for
   architect, engineer, reviewer, and mechanical automation across the fixed
   repo set.
4. It files one parent intake in `acme/service`; no parent intake is filed in
   `acme/service-canary`.
5. The architect fans out one ready child code issue per repo. Engineers open
   real PRs, Forgejo Actions CI passes, reviewers approve, and the mechanical
   worker lands the child PRs.
6. The parent source issue unblocks and closes only after the child work lands.

## Prerequisites

- Rust workspace build tools.
- `curl`, `git`, `mkfifo`, and Python 3.
- A local jig checkout at `~/src/rust/jig` with
  `fixtures/reference-delivery.json` for the default `start` path.
- Pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries in
  `.cache/forgejo/`.
- A host where the runner may execute host-mode jobs directly. No containers.

`run.sh start` builds the development-profile `temper` binary and the local jig
binary. `run.sh multi-repo` builds `temper` plus the in-tree
`temper-testing-worker` binary and does not use jig.

## Layout

```text
examples/reference-delivery/
├── README.md
├── config/
│   ├── workflow.json    # reviewer-gated workflow selected by temper init
│   ├── intake-issue.md  # deliberately thin seed intake body
│   └── ci.yml           # CI committed explicitly into the demo repo
├── observability.md
└── run.sh               # fixed launcher / teardown / validation
```

Runtime data goes under gitignored `run/` and `logs/`. Run-local
`credentials.toml` is in `run/` and is removed during teardown.

## Quick start

From this directory:

```sh
./run.sh start                  # single-repo reviewer-gated standalone demo
./run.sh validate               # inspect retained logs for single-repo evidence
./run.sh stop

./run.sh multi-repo             # cross-repo fan-out demo
./run.sh validate-multi-repo    # while multi-repo is still running: inspect live Forgejo state
./run.sh stop
```

The launcher intentionally has no editable config file. Repo names, ports,
cadences, binary paths, and fixtures are fixed in `run.sh` to keep both paths
short and auditable.

## Troubleshooting

`./run.sh stop` removes the throwaway Forgejo state and run-local credentials,
but logs are retained. For the default path inspect `logs/provision.log` and
`logs/run.log`. For the multi-repo path inspect `logs/provision.log`,
`logs/worker-*.log`, `logs/runner.log`, and `logs/validate-multi-repo.log`.

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
pkill -f 'target/debug/temper-testing-worker'
pkill -f 'target/debug/jig'
rm -rf examples/reference-delivery/run
```
