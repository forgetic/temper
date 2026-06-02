# Harness dogfood launcher

This directory targets the live Forgejo repo
`https://git.ekanayaka.io/ai/harness` using the local credential note at
`~/Documents/personal/forgejo-rhi`. Runtime state under `logs/`, `run/`, and
`secrets/` is ignored by git.

## Run

```sh
cd examples/dogfood
./run.sh
```

The script:

1. builds the worker/trigger with Cargo's development profile (`target/debug`)
   unless `HARNESS_SKIP_BUILD=1`;
2. parses the live role credentials into `secrets/roles.env` (`0600`);
3. starts a local `harness-trigger-forgejo`;
4. opens `ssh -R` through `rhi` so the remote Forgejo can call the local trigger;
5. ensures the non-workflow `product` label, grants the role/product users write
   access to `ai/harness`, and registers/updates one repo webhook (CI workflow
   commits are explicit; set `DOGFOOD_CONFIGURE_CI=1` only for an intentional
   setup run);
6. starts a local host-mode `forgejo-runner` registered with the live instance;
7. starts a tiny dogfood-only intake labeler so newly filed issues get the
   workflow's `untriaged` label automatically; and
8. launches reviewer, human, architect, and mechanical workers; engineer and
   owner auto-merge workers stay skipped until real coding automation is enabled.

Then file workflow intake issues in `https://git.ekanayaka.io/ai/harness/issues`
without adding labels by hand. Issues labeled `product` are treated as product
discussion/planning records, not workflow intake, and the intake labeler will not
add `untriaged` to them. Press `Ctrl-C` or run `./run.sh stop` to stop local
processes.

## Product-manager chat

For the terminal-only product discussion MVP, run:

```sh
./run.sh product-chat
```

This builds `harness-product-manager-chat`, parses `secrets/roles.env`, maps the
configured `DOGFOOD_PRODUCT_CHAT_HUMAN_USER` token (default `free`) to
`HARNESS_PRODUCT_CHAT_HUMAN_TOKEN`, and maps the separate `product-manager`
token to `HARNESS_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN` for product-manager
replies and confirmed filing. If the private note's admin user is exactly the
configured product-chat human, that same-user API token is accepted; otherwise
missing human/product-manager tokens fail closed with no bot/admin fallback for
a different transcript author. The REPL creates a Forgejo transcript issue
labeled `product` only, mirrors turns as comments, shows draft intake issues,
and files one as a normal `untriaged` workflow issue only after `/file <n>`.
Resume an existing product transcript with:

```sh
./run.sh product-chat --transcript-issue 3
```

## Notes

- The webhook remains registered after stop; it will work again on the next run.
  Polling is set to 10s because this live instance may not emit webhooks for
  every label-only workflow transition.
- Tokens/passwords are not printed. Logs live in `logs/`.
- `product-manager` is a separate non-workflow identity, not the workflow
  `owner` role. Its credentials are optional for normal dogfood workers, but
  `./run.sh product-chat` requires `HARNESS_FORGEJO_TOKEN_PRODUCT_MANAGER` in
  `secrets/roles.env` (parsed from the private note). Product-chat human turns
  use `DOGFOOD_PRODUCT_CHAT_HUMAN_USER`, not the workflow `human` alias.
- The local runner executes the repo's CI on this machine using Cargo's dev
  profile (`cargo dev-check`).
- LLM auth defaults to ChatGPT/Codex OAuth from `~/.pi/agent/auth.json` with
  `HARNESS_AGENTS_CODEX_MODEL=gpt-5.5`; run `pi /login openai-codex` if needed.
- The engineer and owner auto-merge workers are skipped while
  `DOGFOOD_ENABLE_ENGINEER_AUTOMATION=0` because the current engineer does not
  yet run a coding tool against this checkout. Do not enable them for live
  dogfood until a real coding workspace seam is wired.
