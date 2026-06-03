# Phase 5 prompt — dogfood profile migration

You are implementing Phase 5 of
`plans/user-defined-interactive-agents/README.md`. Assume Phases 1-4 are done.

## Session bootstrap

Read the normal session docs plus:

- `plans/user-defined-interactive-agents/README.md`
- all previous phase changes
- `examples/dogfood/README.md`
- `examples/dogfood/run.sh`
- `examples/dogfood/config/dogfood.env`
- `examples/dogfood/tools/{parse_secrets.py,configure_forgejo.py,label_intake.py}`
- generic interaction service docs and binary args

## Goal

Move dogfood product-manager chat from product-specific production wiring to a
configured example interaction profile. The operator experience may remain
`./run.sh product-chat`, but the script should call the generic interaction
binary/service with an example spec and deployment bindings.

## Tasks

1. Add an example dogfood interaction spec, for example under:

   ```text
   examples/dogfood/config/interaction-profiles/product-manager.json
   ```

   It should encode all product-manager behavior currently hard-coded in
   production:

   - profile id/display names;
   - transcript label and title policy;
   - marker namespace;
   - issue proposal kind;
   - `/file` or equivalent command alias;
   - accepted issue creation effect with `untriaged` label and transcript
     backlink/idempotency marker.

2. Update `examples/dogfood/run.sh product-chat` to:

   - resolve/build the generic interaction binary;
   - pass the profile spec path and selected profile id;
   - pass Forge tokens and responder process bindings as deployment config/env;
   - avoid product-manager-specific Temper env var names where possible.

   It is acceptable for dogfood-local variable names to remain descriptive, but
   the core binary should not require `TEMPER_PRODUCT_CHAT_*` names.

3. Keep the dogfood safety rails:

   - transcript issues carry only the configured product label;
   - the intake labeler does not relabel product transcript issues;
   - filed issues enter the workflow only through the configured accepted action;
   - product-manager identity remains separate from workflow `owner`.

4. Update `examples/dogfood/README.md` so it explains that product-manager is an
   example interaction profile, not a core Temper role.

5. Add focused tests or shell/Python checks for:

   - dogfood example spec validates/compiles;
   - parse/config scripts still map the example product-manager user safely;
   - `sh -n examples/dogfood/run.sh`;
   - existing product-chat compatibility tests, if aliases remain.

6. Update plan status.

## Constraints

- Do not break the live dogfood flow without providing the generic replacement.
- Do not print secrets.
- Do not reintroduce product-manager constants into generic production code.
- Keep examples/dogfood as an example; core crates must remain profile-agnostic.

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
```
