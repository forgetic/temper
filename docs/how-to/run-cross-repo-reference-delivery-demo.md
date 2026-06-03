# Run the cross-repo reference-delivery demo

This recipe runs the operator demo with one intake issue in the first repo that
fans out into child work across the configured repo set. For the conceptual
model, read `docs/explanation/cross-repo-workflows.md` first.

## Prerequisites

Follow `examples/reference-delivery/README.md` for Forgejo, runner, and LLM auth
setup. The important cross-repo requirements are:

- every role token must have Forge permission on **all** involved repositories;
- provisioning must ensure workflow labels, the CI workflow, and webhooks in
  every repository;
- all repos use the same compiled reference-delivery workflow;
- generated prompts provide workflow mechanics, while role behavior and
  `coding_workspace` guidance come from `config/workflow.json` / the canonical
  fixture rather than production prompt files.

## Configure

From `examples/reference-delivery/`, edit `config/temper.env` or export env
vars before running:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
POLL_MS=120000
WEBHOOKS=1
```

`auto` enables cross-repo intake seeding when `REPOS` contains more than one
repo. Set `CROSS_REPO_INTAKE=0` to return to independent per-repo intake issues,
or set `REPOS=` and `CROSS_REPO_INTAKE=0` for the legacy single-repo smoke.

To let the engineer open real PRs, bind the declared coding workspace tool before
`start`:

```sh
export TEMPER_CODING_WORKSPACE_ROOT=/path/to/clean/checkout
export TEMPER_CODING_WORKSPACE_COMMAND='your-coder --context "$TEMPER_CODING_WORKSPACE_CONTEXT"'
```

If those are empty, ready code issues may be idle by design: the engineer prompt
will show no bound external tool, so the safe action is `no_action`.

## Run

```sh
cd examples/reference-delivery
POLL_MS=120000 ./run.sh start
```

The launcher provisions every configured repo. It seeds exactly one parent
intake issue in the first repo and writes that issue with explicit target repo
ids for every child. Production generic agents use manifest prompts and declared
external tools; any fixed fan-out or produced-issue closure behavior must come
from user workflow configuration or a test fixture, not a production
role-specific flag.

## Validate logs and Forge state

In another terminal, before `./run.sh stop` removes the throwaway Forgejo data:

```sh
cd examples/reference-delivery
./run.sh validate-multi-repo
```

The validator checks per-repo provisioning, webhook registration and delivery,
worker wake consumption, target repos without duplicate parent intakes, and live
Forge state for the seeded parent. It expects the parent to have one child
dependency per configured repo, child issues with parent/correlation metadata,
and no blocked parent with zero dependencies. For the original incident shape it
prints diagnostics such as:

```text
missing: cross-repo parent acme/service#1 expected 2 child dependencies, found 0
diagnosis: architect blocked the parent but no fan-out side effects were recorded
```

For event names and correlation fields (`worker_capabilities`, `scan_summary`,
`work_item_selected`, `role_decision_*`, `action_dispatch`,
`transition_execution`, and `mechanical_reconciliation`), see
`examples/reference-delivery/observability.md`. For the workspace/PR guard path,
also run:

```sh
cargo test -p temper-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff
TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_workspace_pr -- --ignored --test-threads=1
```

Open Forgejo and confirm:

1. the source repo has the parent intake issue;
2. each repo has one child code issue linked from that parent;
3. each child has a merged implementation PR in its own repo;
4. the parent unblocks only after all children land.

## Swap toward a real deployment

Keep the same shape: one role worker per role scanning a repo set, role tokens
with access to every repo, per-repo labels/CI/webhooks, and polling as the
backstop. Replace the demo CI marker with real CI and expose a real HTTPS
webhook URL. This remains a demo launcher, not a turnkey deployment tool.
