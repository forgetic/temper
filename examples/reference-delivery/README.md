# Reference-delivery example

A self-contained Temper demo of the cross-repo reference-delivery workflow. The
default path boots a throwaway Forgejo server, registers a real host-mode
`forgejo-runner`, provisions `acme/service` plus `acme/service-canary`, files one
parent intake in the source repo, and uses deterministic workers to fan out,
review, test, and land child implementation PRs in both repositories.

The launcher still has an explicit `./run.sh single-repo` path for the
reviewer-gated one-repo `temper serve standalone` demo.

This is the richer counterpart to [`examples/basic-delivery/`](../basic-delivery/):
the optional single-repo path uses the same long-term UX commands (`temper
--config ... init --apply --yes` and `temper --config ... serve standalone`) but
selects `config/workflow.json`, whose happy path adds a reviewer approval gate
between the engineer PR and bot landing.

## What the default demo demonstrates

1. `run.sh start` (or `run.sh multi-repo`) boots throwaway Forgejo + a host-mode
   `forgejo-runner`.
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
- Pinned Forgejo `7.0.12` and `forgejo-runner` `3.5.1` binaries in
  `.cache/forgejo/`.
- A host where the runner may execute host-mode jobs directly. No containers.
- A local jig checkout at `~/src/rust/jig` with
  `fixtures/reference-delivery.json` only if you run `./run.sh single-repo`.

`run.sh start` builds the development-profile `temper` binary plus the in-tree
`temper-testing-worker` binary and does not use jig. `run.sh single-repo` builds
`temper` and the local jig binary.

## Layout

```text
examples/reference-delivery/
├── README.md
├── config/
│   ├── workflow.json    # reviewer-gated workflow used by both paths
│   ├── intake-issue.md  # deliberately thin single-repo seed intake body
│   └── ci.yml           # CI committed explicitly into demo repositories
├── observability.md
└── run.sh               # fixed launcher / teardown
```

Runtime data goes under gitignored `run/` and `logs/`. Run-local
`credentials.toml` is in `run/` and is removed during teardown.

## Quick start

From this directory:

```sh
./run.sh start                  # cross-repo fan-out demo (default)
./run.sh stop

./run.sh single-repo            # optional single-repo reviewer-gated standalone demo
./run.sh stop
```

The launcher intentionally has no editable config file. Repo names, ports,
cadences, binary paths, and fixtures are fixed in `run.sh` to keep both paths
short and auditable. The checked-in default is the two-repository fan-out path.

## Troubleshooting

`./run.sh stop` removes the throwaway Forgejo state and run-local credentials,
but logs are retained. For the default cross-repo path inspect
`logs/provision.log`, `logs/worker-*.log`, and `logs/runner.log`. For the
optional single-repo path inspect `logs/provision.log` and `logs/run.log`.

If a run is force-killed, clean up possible orphans:

```sh
pkill -f forgejo
pkill -f forgejo-runner
pkill -f 'target/debug/temper'
pkill -f 'target/debug/temper-testing-worker'
pkill -f 'target/debug/jig'
rm -rf examples/reference-delivery/run
```
