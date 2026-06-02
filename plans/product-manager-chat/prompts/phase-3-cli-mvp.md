# Phase 3 — CLI REPL MVP + Forgejo transcript/filing core

Ship the first human-facing product-manager MVP with **no web app**: an
interactive terminal REPL that mirrors conversation turns to Forgejo and files
workflow intake issues only after explicit human command.

## Bootstrap

1. Follow the normal session bootstrap in `AGENTS.md`.
2. Read:
   - `plans/product-manager-chat/README.md`
   - Phase 1 and Phase 2 changes
   - `examples/dogfood/README.md`
   - `examples/dogfood/run.sh`
   - `crates/temper-production/src/worker_args.rs` for CLI parsing style
   - `crates/temper-production/src/forgejo_rest.rs` for REST helper style
   - `crates/temper-workflow/src/execute/ensure.rs` for correlation-key pattern
   - `docs/reference/workflow-layer.md` idempotent create section
3. Keep the MVP terminal-only. Do not add a web app or frontend assets.

## Goal

A user can run:

```sh
cd examples/dogfood
./run.sh product-chat
```

and get an interactive session that:

- creates or resumes a Forgejo transcript issue labeled `product` only;
- posts human turns as the human/user Forgejo identity;
- posts product-manager replies as the product-manager Forgejo identity;
- displays draft intake issues returned by the LLM;
- supports `/file <n>` to create a normal `untriaged` workflow issue;
- creates each filed issue at most once per transcript + draft slug.

## Binary and wrapper

Add a production-owned binary, for example:

```text
crates/temper-production/src/bin/temper-product-manager-chat.rs
```

and wire it in `crates/temper-production/Cargo.toml`.

The binary should support a REPL mode such as:

```sh
temper-product-manager-chat repl \
  --base-url https://git.ekanayaka.io \
  --repo ai/temper \
  --auth chatgpt-oauth \
  [--codex-model gpt-5.5] \
  [--auth-file ~/.pi/agent/auth.json] \
  [--transcript-issue 3]
```

Secrets must come from env, not argv. Suggested env names:

```sh
TEMPER_PRODUCT_CHAT_HUMAN_TOKEN=...
TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN=...
```

Optional username/password envs can be added only if the Forgejo backend path
needs them; issue/comment REST calls should only need tokens.

Extend `examples/dogfood/run.sh`:

- add `product-chat` to usage;
- build/resolve the new binary;
- load config and parse secrets;
- mint or use the human/admin token for `TEMPER_PRODUCT_CHAT_HUMAN_TOKEN`;
- require `TEMPER_FORGEJO_TOKEN_PRODUCT_MANAGER` for the product-manager token;
- pass auth/model flags consistently with existing role workers;
- snapshot `product-chat` runs if the command blocks long enough that editing
  the script during a session could otherwise affect teardown/behavior.

Normal `./run.sh start` must remain unchanged.

## Transcript issue behavior

Default behavior: create a new transcript issue.

Also support `--transcript-issue <number>` to resume/post into an existing
product transcript issue. This is useful while dogfooding.

Transcript issue requirements:

- title default: `Product conversation: <date/time or short topic>`;
- body contains a hidden marker, e.g.
  `<!-- temper:product-chat-session=<session-key> -->`;
- labels: `product` only;
- no `untriaged`;
- if `--transcript-issue` is supplied, verify the issue exists and has `product`.

The product-manager LLM context should include recent conversation turns and the
transcript URL. Do not include tokens or internal logs.

## REPL commands

Minimum commands:

- plain text: send a human turn, run the product-manager, post/display reply;
- `/drafts`: show the latest draft list;
- `/file <n>`: file draft number `n` as workflow intake;
- `/issue`: print the transcript issue URL;
- `/help`;
- `/quit` or EOF.

Keep the UI simple. This is a feel-test MVP.

## Filing behavior

When `/file <n>` is entered:

- use the product-manager Forgejo token to create the intake issue;
- add the workflow entry label `untriaged`;
- body includes:
  - the draft body;
  - backlink to the transcript issue;
  - a hidden correlation marker such as
    `<!-- temper:product-chat-file=<session-key>:<draft-slug> -->`;
  - `requested-by: <human>` if the current human identity is known.
- before creating, search existing issues for the same correlation marker and
  return the existing issue if found.

The model should never perform filing itself. The CLI command is the explicit
human confirmation boundary.

## Core factoring

Do not bury all logic in `main`. Add a small integration core module under
`temper-production`, for example:

```text
src/product_chat.rs
src/product_chat_args.rs
```

The core should be reusable by Phase 4's service API:

- create/resume transcript;
- append human comment;
- call product-manager agent;
- append product-manager comment;
- track latest drafts;
- file draft idempotently.

If you need traits/fakes to test without network/LLM, add them now.

## Tests

Default tests must not hit the live Forgejo or LLM provider.

Add offline tests for:

- argument parsing and token redaction in `Debug`;
- transcript marker rendering/parsing;
- filing correlation marker rendering/search behavior;
- `/file <n>` refuses invalid draft numbers;
- product-chat core can be driven with fake Forgejo/comment store and fake
  product-manager response, if you introduce fakes.

Manual live validation is optional for this phase, but if you run it, use the
ChatGPT OAuth default and record a short note in this plan README.

Run:

```sh
cargo fmt --all
cargo test -p temper-production product_chat
cargo dev-check
```

## Documentation updates

- Update `examples/dogfood/README.md` with the `./run.sh product-chat` flow.
- Document required env/secrets and the fact that product-manager is not a
  workflow role.
- Update `plans/product-manager-chat/README.md` phase status when complete.

## Acceptance criteria

- `./run.sh product-chat` starts a terminal product-manager session.
- Forgejo captures the transcript under human and product-manager identities.
- Transcript issues remain `product` only.
- `/file <n>` creates an idempotent `untriaged` intake issue.
- No web UI is added.
