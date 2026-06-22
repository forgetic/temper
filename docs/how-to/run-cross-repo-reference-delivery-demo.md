# Run the cross-repo reference-delivery demo

This recipe runs Temper's local jig-backed operator demo with one parent intake
issue in the first repo that fans out into child work across the configured repo
set. It uses deterministic jig role replies, a real throwaway Forgejo, and a real
host-mode `forgejo-runner`; it does not use provider auth.

For the conceptual model, read `docs/explanation/cross-repo-workflows.md` first.

## Prerequisites

Follow `examples/reference-delivery/README.md`. In short:

- provide the pinned Forgejo / `forgejo-runner` binaries via the shared cache or
  explicit `TEMPER_FORGEJO_BINARY` / `TEMPER_FORGEJO_RUNNER_BINARY` paths;
- run on a host that permits host-mode runner jobs;
- let `run.sh` build the root `temper` package and the vanilla jig standalone
  server from the sibling jig checkout, or provide current binary/fixture
  overrides.

## Configure

From `examples/reference-delivery/`, edit `config/temper.env` or export env vars:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
DAEMON_POLL_CADENCE_SECS=120
DAEMON_MECHANICAL_CADENCE_SECS=2
RUN_MAX_ITERATIONS=8
```

`auto` enables cross-repo intake seeding when `REPOS` contains more than one
repo. Set `CROSS_REPO_INTAKE=0` for independent per-repo intakes, or set
`REPOS=` and `CROSS_REPO_INTAKE=0` for single-repo mode.
`DAEMON_POLL_CADENCE_SECS` controls role-worker polling so webhook wakeups remain
visible; `DAEMON_MECHANICAL_CADENCE_SECS` controls the mechanical CI/landing
backstop.

## Run

```sh
cd examples/reference-delivery
./run.sh start
```

The launcher provisions every configured repo. It seeds exactly one parent issue
in the first repo. The issue body lists target repositories; the deterministic
jig architect creates one child code issue per repo. The jig engineer writes a
real product file in each checkout, the real runner executes CI, the jig reviewer
approves, and the mechanical worker lands PRs as the provisioned `bot` automation
user. The bot token performs REST mutations (merges) and the bot's web-UI
username/password reads Forgejo 7.0.x CI status (ADR 0019). The site admin is
used only for initial provisioning and never participates in the workflow.

## Validate logs and Forge state

In another terminal, before `./run.sh stop` removes the throwaway Forgejo data:

```sh
cd examples/reference-delivery
./run.sh validate-multi-repo
```

The validator checks per-repo provisioning, webhook registration/delivery,
jig-backed agent activity, target repos without duplicate parent intakes, and
live Forge state for the seeded parent including landed child issues.

Open Forgejo and confirm:

1. the source repo has the parent intake issue;
2. each repo has one child code issue linked from that parent;
3. each child has a merged implementation PR in its own repo;
4. the parent unblocks only after all children land.

For event/log names, see `examples/reference-delivery/observability.md`.

## Provider-backed examples

Provider-backed dogfood/product-chat launchers live outside this deterministic
operator demo. Temper's `examples/reference-delivery/` intentionally stays
jig-backed and provider-auth-free.
