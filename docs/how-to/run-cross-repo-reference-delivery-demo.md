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

From `examples/reference-delivery/`, edit `config/harness.env` or export env
vars before running:

```sh
REPOS="acme/service acme/service-canary"
CROSS_REPO_INTAKE=auto
POLL_MS=120000
WEBHOOKS=1
ARCHITECT_CLOSE_PRODUCED_ISSUES=1
```

`auto` enables cross-repo intake seeding when `REPOS` contains more than one
repo. Set `CROSS_REPO_INTAKE=0` to return to independent per-repo intake issues,
or set `REPOS=` and `CROSS_REPO_INTAKE=0` for the legacy single-repo smoke.

To let the engineer open real PRs, bind the declared coding workspace tool before
`start`:

```sh
export HARNESS_CODING_WORKSPACE_ROOT=/path/to/clean/checkout
export HARNESS_CODING_WORKSPACE_COMMAND='your-coder --context "$HARNESS_CODING_WORKSPACE_CONTEXT"'
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
ids for every child. The architect should fan out child code issues, one per
repo. Each child then follows the ordinary per-repo path: engineer PR, CI,
review, owner merge, landed reconciliation, and architect-side closure of the
produced code issue. That closure clears the child's `in-progress` label and is
the portable landed signal used to unblock the cross-repo parent.

## Validate logs

In another terminal:

```sh
cd examples/reference-delivery
./run.sh validate-multi-repo
```

The validator checks per-repo provisioning, webhook registration and delivery,
worker wake consumption, and that target repos were provisioned without duplicate
parent intakes. For the workspace/PR guard path, also run:

```sh
cargo test -p harness-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff
HARNESS_FORGEJO_E2E=1 cargo test -p harness-testing --test forgejo_workspace_pr -- --ignored --test-threads=1
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
