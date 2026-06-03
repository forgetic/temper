# Run the cross-repo reference-delivery demo

This recipe runs Temper's local fake-agent operator demo with one parent intake
issue in the first repo that fans out into child work across the configured repo
set. It uses deterministic fake agents, a real throwaway Forgejo, and a real
host-mode `forgejo-runner`; it does not use Smith or provider auth.

For the conceptual model, read `docs/explanation/cross-repo-workflows.md` first.

## Prerequisites

Follow `examples/reference-delivery/README.md`. In short:

- populate the pinned Forgejo / `forgejo-runner` binary cache;
- run on a host that permits host-mode runner jobs;
- let `run.sh` build `temper-production` helpers and `temper-testing-worker`, or
  provide current binary overrides.

## Configure

From `examples/reference-delivery/`, edit `config/temper.env` or export env vars:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
POLL_MS=120000
WEBHOOKS=1
FAKE_ARCHITECT=closing
```

`auto` enables cross-repo intake seeding when `REPOS` contains more than one
repo. Set `CROSS_REPO_INTAKE=0` for independent per-repo intakes, or set
`REPOS=` and `CROSS_REPO_INTAKE=0` for single-repo mode.

## Run

```sh
cd examples/reference-delivery
POLL_MS=120000 ./run.sh start
```

The launcher provisions every configured repo. It seeds exactly one parent issue
in the first repo. The issue body contains a hidden deterministic fake-architect
plan, so the fake architect creates one child code issue per repo.

## Validate logs and Forge state

In another terminal, before `./run.sh stop` removes the throwaway Forgejo data:

```sh
cd examples/reference-delivery
./run.sh validate-multi-repo
```

The validator checks per-repo provisioning, webhook registration/delivery, fake
worker wake consumption, target repos without duplicate parent intakes, and live
Forge state for the seeded parent.

Open Forgejo and confirm:

1. the source repo has the parent intake issue;
2. each repo has one child code issue linked from that parent;
3. each child has a merged implementation PR in its own repo;
4. the parent unblocks only after all children land.

For event/log names, see `examples/reference-delivery/observability.md`.

## Smith-backed examples

Smith/provider-backed role decisions and the dogfood/product-chat launchers live
in the sibling Smith checkout under `~/src/rust/smith/examples/`. Temper's
`examples/reference-delivery/` intentionally stays fake-agent-only.
