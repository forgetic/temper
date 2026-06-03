# Temper dogfood launcher

This directory targets the live Forgejo repo
`https://git.ekanayaka.io/ai/temper` using the local credential note at
`~/Documents/personal/forgejo-rhi`. Runtime state under `logs/`, `run/`, and
`secrets/` is ignored by git.

## Run

```sh
cd examples/dogfood
./run.sh
```

The script:

1. builds the worker/trigger with Cargo's development profile (`target/debug`)
   unless `TEMPER_SKIP_BUILD=1`;
2. parses the live role credentials into `secrets/roles.env` (`0600`);
3. runs the engineer-automation preflight;
4. starts a local `temper-trigger-forgejo`;
5. opens `ssh -R` through `rhi` so the remote Forgejo can call the local trigger;
6. ensures the non-workflow `product` label, grants the role/product users write
   access to `ai/temper`, and registers/updates one repo webhook (CI workflow
   commits are explicit; set `DOGFOOD_CONFIGURE_CI=1` only for an intentional
   setup run);
7. starts a local host-mode `forgejo-runner` registered with the live instance;
8. starts a tiny dogfood-only intake labeler so newly filed issues get the
   workflow's `untriaged` label automatically; and
9. launches reviewer, human, architect, and mechanical workers; engineer and
   owner auto-merge workers stay skipped until a coding workspace binding is
   configured and intentionally enabled.

Then file workflow intake issues in `https://git.ekanayaka.io/ai/temper/issues`
without adding labels by hand. Issues labeled `product` are treated as product
discussion/planning records, not workflow intake, and the intake labeler will not
add `untriaged` to them. Press `Ctrl-C` or run `./run.sh stop` to stop local
processes.

## Product-manager chat

For the terminal-only product discussion MVP, run:

```sh
./run.sh product-chat
```

This builds `temper-product-manager-chat`, parses `secrets/roles.env`, maps the
configured `DOGFOOD_PRODUCT_CHAT_HUMAN_USER` token (default `free`) to
`TEMPER_PRODUCT_CHAT_HUMAN_TOKEN`, and maps the separate `product-manager`
token to `TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN` for product-manager
replies and confirmed filing. If the private note's admin user is exactly the
configured product-chat human, that same-user API token is accepted; otherwise
missing human/product-manager tokens fail closed with no bot/admin fallback for
a different transcript author. The REPL creates a Forgejo transcript issue
labeled `product` only, mirrors turns as comments, shows draft intake issues,
and files one as a normal `untriaged` workflow issue only after `/file <n>`.

By default this uses Temper's transitional in-process responder. To validate the
Smith process responder while keeping the same operator command, set
`DOGFOOD_PRODUCT_CHAT_RESPONDER=smith` in `config/dogfood.env` (or export it)
and keep `SMITH_WORKSPACE_ROOT` pointed at `~/src/rust/smith`. The launcher
builds `smith-product-manager-responder` and passes it as
`temper-product-manager-chat --responder-command ...`; Forge tokens still stay in
Temper and only provider auth arguments/env allow-list entries reach Smith.
Resume an existing product transcript with:

```sh
./run.sh product-chat --transcript-issue 3
```

## Engineer automation preflight

Run this before trying to re-enable the engineer/owner pair:

```sh
cd examples/dogfood
./run.sh preflight
```

A `code` + `ready` issue is intentionally idle when the report lists blockers
for any of these keys/paths:

- `DOGFOOD_ENABLE_ENGINEER_AUTOMATION` must stay `0` by default and become `1`
  only for an intentional live issue.
- The compiled reference workflow must declare
  `roles[engineer].external_tools[id=coding_workspace]`.
- `TEMPER_CODING_WORKSPACE_ROOT` must point at a git checkout and
  `TEMPER_CODING_WORKSPACE_COMMAND` must name the operator-configured coder.
- `DOGFOOD_PR_DIFF_GUARD=1` and `DOGFOOD_ALLOW_BOOKKEEPING_ONLY_PR=0` keep the
  reviewer/owner diff guard active.
- `secrets/roles.env` must be `0600` and contain engineer and owner tokens.

The generated role prompt contains workflow mechanics and authorized actions.
Reference-delivery behavior (how to implement, when to use `coding_workspace`,
and why bookkeeping-only PRs are forbidden) comes from the workflow fixture's
`charter`, `prompt.guidance`, `prompt.tool_guidance`, and `external_tools`
entries, not from production checked-in engineer/reviewer/owner prompts. The LLM
never receives shell/file tools; it can only choose a workflow action, after
which the runner invokes an explicitly declared and bound workspace provider.

Focused validation commands:

```sh
python3 -m unittest discover -s examples/dogfood/tools -p '*_test.py'
cargo test -p temper-production coding_workspace_tests::local_git_workspace_accepts_product_code_or_docs_diff
cargo test -p temper-production worker_tests::dogfood_reference_engineer_declares_coding_workspace
TEMPER_FORGEJO_E2E=1 cargo test -p temper-testing --test forgejo_workspace_pr -- --ignored --test-threads=1
```

## Notes

- The webhook remains registered after stop; it will work again on the next run.
  Polling is set to 10s because this live instance may not emit webhooks for
  every label-only workflow transition.
- Tokens/passwords are not printed. Logs live in `logs/`.
- `product-manager` is a separate non-workflow identity, not the workflow
  `owner` role. Its credentials are optional for normal dogfood workers, but
  `./run.sh product-chat` requires `TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER` in
  `secrets/roles.env` (parsed from the private note). Product-chat human turns
  use `DOGFOOD_PRODUCT_CHAT_HUMAN_USER`, not the workflow `human` alias.
- The local runner executes the repo's CI on this machine using Cargo's dev
  profile (`cargo dev-check`).
- LLM auth defaults to ChatGPT/Codex OAuth from `~/.pi/agent/auth.json` with
  `TEMPER_AGENTS_CODEX_MODEL=gpt-5.5`; run `pi /login openai-codex` if needed.
