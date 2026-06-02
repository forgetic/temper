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
5. installs/updates `.forgejo/workflows/ci.yml`, enables Actions, ensures the
   non-workflow `product` label, grants the role/product users write access to
   `ai/harness`, and registers/updates one repo webhook;
6. starts a local host-mode `forgejo-runner` registered with the live instance;
7. starts a tiny dogfood-only intake labeler so newly filed issues get the
   workflow's `untriaged` label automatically; and
8. launches engineer, reviewer, owner, human, architect, and mechanical workers.

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
human token (or a short-lived admin fallback) to
`HARNESS_PRODUCT_CHAT_HUMAN_TOKEN`, and maps the separate `product-manager`
token to `HARNESS_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN` for product-manager
replies and confirmed filing. The REPL creates a Forgejo transcript issue labeled
`product` only, mirrors turns as comments, shows draft intake issues, and files
one as a normal `untriaged` workflow issue only after `/file <n>`. Resume an existing
product transcript with:

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
  `secrets/roles.env` (parsed from the private note).
- The local runner executes the repo's CI on this machine using Cargo's dev
  profile (`cargo dev-check`).
- LLM auth defaults to ChatGPT/Codex OAuth from `~/.pi/agent/auth.json` with
  `HARNESS_AGENTS_CODEX_MODEL=gpt-5.5`; run `pi /login openai-codex` if needed.
- This launcher uses the current reference-delivery worker behavior. The current
  engineer opens Forgejo PRs through the Harness workflow, but it does not yet
  run a coding tool against this checkout.
