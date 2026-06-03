# Phase 1 prompt — interaction spec and validation contract

You are implementing Phase 1 of
`plans/user-defined-interactive-agents/README.md`.

## Session bootstrap

1. Confirm you are in `/home/free/src/rust/temper`.
2. Read `README.md`, `AGENTS.md`, `docs/README.md`, and
   `docs/reference/development-conventions.md`.
3. Read:
   - `docs/explanation/interactive-agent-interfaces.md`
   - `docs/reference/interactive-conversation-interface.md`
   - `docs/reference/interactive-process-responder-protocol.md`
   - `docs/reference/agent-lessons/0024-product-manager-is-an-interactive-profile.md`
   - `docs/reference/agent-lessons/0025-process-boundary-for-interactive-responders.md`
   - `plans/interactive-agent-interfaces/README.md`
   - `plans/user-defined-interactive-agents/README.md`
4. Inspect `crates/temper-interaction/src/` and the product-chat production
   modules so you know which product-manager constants must eventually move to
   config.

## Goal

Add the user-defined interaction profile **specification and validation** layer.
This phase establishes the contract; it does not need to finish all runtime
refactoring.

The new contract should make concrete profiles such as `product-manager` data,
not code. Follow the workflow layer's phase model where practical:

```text
RawInteractionSpec -> ValidatedInteractionSpec -> later compiled manifests
```

## Tasks

1. Add spec/validation modules to `temper-interaction`.

   Suggested modules:

   - `spec`: raw serde-loadable structs;
   - `validated`: normalized profile model with no public constructor except
     validation;
   - `ids`: typed ids if the existing deterministic ids are not sufficient;
   - `validate`: diagnostic collection and validation entry points.

   Keep the crate provider-neutral except for existing Forge transcript helpers.
   Do not add provider SDK dependencies.

2. Model at least these raw/validated concepts:

   - interaction spec id;
   - interactive profiles;
   - participants / display names;
   - transcript policy: target kind for now can be Forge issue, title prefix,
     label set/policy, marker namespace, recent-turn limit;
   - responder declaration: id, process protocol/version intent, required flag;
   - proposal kinds and built-in payload contracts such as `issue_draft`;
   - transport command declarations: command id, aliases, action reference;
   - acceptance action declarations: proposal kind, explicit acceptance policy,
     idempotency key template, closed effect list.

   It is acceptable to keep the closed effect set small in Phase 1, as long as
   issue creation with labels/body/template/backlink can be represented for the
   existing dogfood use case.

3. Add validation diagnostics for:

   - duplicate ids;
   - invalid deterministic slug ids;
   - references to undeclared proposal kinds, responders, commands, or acceptance
     actions;
   - empty transcript labels/title prefix/marker namespace;
   - command aliases that are empty or conflict within one profile;
   - unsupported effect kinds or payload contracts;
   - unknown fields in raw serde structs (`deny_unknown_fields`).

4. Add a product-manager **fixture spec** under an appropriate fixture/example
   path, not as production constants. It should encode the current dogfood
   behavior: product transcript label, untriaged filed-issue label, `/file` alias,
   and issue proposal acceptance. This fixture exists to test the generic system.

5. Add tests proving:

   - a generic non-product profile validates;
   - the product-manager fixture validates;
   - validation catches duplicates, bad refs, unknown fields, and alias conflicts;
   - no validation logic depends on the literal profile id `product-manager`.

6. Update docs/reference for the new spec contract. If the docs would become too
   large, create a focused `docs/reference/interaction-profile-spec.md` and link
   it from `docs/README.md` and `docs/reference/interactive-conversation-interface.md`.

7. Update Phase 1 status in `plans/user-defined-interactive-agents/README.md`.

## Constraints

- Do not change `temper-forge`.
- Do not expose Forge mutation authority to responders.
- Do not remove current product-chat commands yet; this phase may leave them as
  compatibility code.
- Keep product-manager examples in fixtures/examples/tests, not as the generic
  contract.

## Validation

Run and record:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
