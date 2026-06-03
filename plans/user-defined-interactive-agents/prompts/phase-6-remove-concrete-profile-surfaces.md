# Phase 6 prompt — remove concrete-profile production surfaces

You are implementing Phase 6 of
`plans/user-defined-interactive-agents/README.md`. Assume Phases 1-5 are done.

## Session bootstrap

Read the normal session docs plus:

- `plans/user-defined-interactive-agents/README.md`
- all previous phase changes
- `docs/reference/development-conventions.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/llm-agents.md`
- `examples/dogfood/README.md`
- all remaining `product_chat*` source files, if any

## Goal

Finish the extraction: non-test/non-fixture production Temper code should have no
hard-coded knowledge of product-manager or any other concrete interactive
profile. Product-manager may remain only as an example spec, fixture, dogfood
configuration, docs, or external responder profile.

## Tasks

1. Remove or rename product-specific production surfaces.

   Candidates to delete, move to tests, or replace with generic modules:

   - `crates/temper-production/src/product_chat.rs`
   - `crates/temper-production/src/product_chat_service.rs`
   - `crates/temper-production/src/product_chat_repl.rs`
   - `crates/temper-production/src/product_chat_commands.rs`
   - `crates/temper-production/src/product_chat_args.rs`
   - `crates/temper-production/src/bin/temper-product-manager-chat.rs`
   - product-manager-specific DTOs/env names/routes that are no longer needed.

   If a compatibility binary must remain for one release, make it a tiny wrapper
   whose only product-manager knowledge is in examples/docs, not generic runtime
   modules, and document the planned removal. Prefer deleting once dogfood has
   migrated.

2. Ensure generic names are used for runtime concepts:

   - interaction profile;
   - conversation/session;
   - proposal;
   - acceptance action;
   - command alias;
   - transcript policy;
   - responder binding.

3. Add a grep-style regression test or CI/dev-check helper.

   The final guard should fail if non-test/non-fixture `crates/` sources contain
   concrete profile strings such as:

   - `product-manager`
   - `ProductChat`
   - `product_chat`
   - `product-chat`
   - `Product conversation`
   - `TEMPER_PRODUCT_CHAT`
   - `/file `

   Scope the guard carefully so docs/examples/plans and intentional fixture files
   are allowed.

4. Update docs:

   - `docs/reference/interactive-conversation-interface.md` should describe the
     generic user-defined profile model;
   - product-manager docs should either move under examples/dogfood or clearly
     state they are an example profile spec;
   - `docs/reference/llm-agents.md` should keep the process-boundary story
     generic;
   - add/update an agent lesson if this extraction prevents a recurring mistake.

5. Run the final regression grep manually and include the output in the handoff.

6. Mark the whole plan complete when all acceptance criteria pass.

## Constraints

- Keep product-manager dogfood available through the example spec.
- Do not delete tests that prove the product-manager fixture use case works;
  convert them to generic runtime tests using the fixture spec.
- Do not broaden responder authority or add provider SDK dependencies.
- Keep source files under the project size conventions.

## Validation

Run and record:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production --all-targets
python3 -m unittest discover -s examples/dogfood/tools -p '*_test.py'
sh -n examples/dogfood/run.sh
cargo dev-clippy
cargo dev-check

rg -n "product-manager|ProductChat|product_chat|product-chat|Product conversation|TEMPER_PRODUCT_CHAT|/file " \
  crates \
  --glob '!**/*test*' \
  --glob '!**/tests/**' \
  --glob '!**/fixtures/**'
```

The final `rg` should have no production hits, or only explicitly justified
compatibility wrapper hits documented for removal.
