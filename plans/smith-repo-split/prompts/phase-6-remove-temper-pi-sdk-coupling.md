# Phase 6 prompt — remove Temper pi-SDK coupling and close parity

You are implementing Phase 6 of `plans/smith-repo-split/README.md`. Assume Smith
successfully provides both interactive and workflow process responders, and the
coverage ledger shows Smith or Temper replacements for every moved test.

## Read first

- `plans/smith-repo-split/README.md`
- `plans/smith-repo-split/coverage-ledger.md`
- `docs/reference/development-conventions.md`
- `docs/reference/llm-agents.md`
- `docs/reference/interactive-conversation-interface.md`
- `crates/temper-agents/`
- `crates/temper-production/Cargo.toml`
- `Cargo.toml`
- Smith README and test docs

## Goal

Finish the split: Temper no longer depends on `pi_agent_rust` or concrete
provider SDK/auth code, while Smith owns that behavior and equivalent coverage.

## Tasks

1. Remove or deprecate `crates/temper-agents` from Temper.

   Acceptable outcomes:

   - delete the crate if all behavior moved cleanly;
   - leave only provider-neutral test fixtures/adapters if a temporary transition
     requires it, but it must not depend on `pi_agent_rust`.

2. Remove `temper-agents` dependencies from production crates and workspace
   metadata. Production workers should use process responder configuration or
   fake/test agents, not in-process pi-SDK agents.

3. Update docs and examples:

   - explain that Smith is the first concrete pi-SDK implementation;
   - document how to configure Smith process commands for product-chat and
     workflow roles;
   - keep Temper docs focused on protocols, not provider auth internals;
   - move provider-auth how-to content to Smith or clearly mark it as Smith-owned
     if it remains temporarily linked.

4. Close the coverage ledger.

   Every moved/deleted test should have a Smith or Temper replacement and a
   command. If a real-world/e2e test is still blocked by infrastructure, record
   why and keep the ignored/env-gated test in the appropriate repo.

5. Run split parity validation.

   Do not go overboard creating duplicate suites, but do run the practical gates
   that prove Temper+Smith still works together.

6. Update Phase 6 status and add any follow-up issues/plans for cleanup that is
   intentionally deferred.

## Constraints

- Do not remove real-world/e2e coverage just because it is inconvenient. Keep it
  ignored/env-gated if necessary.
- Do not leave provider secrets or auth-file implementation details in Temper
  logs/docs beyond configuration pointers to Smith.
- Do not make Temper depend on Smith as a Rust crate.
- Do not weaken workflow authority: Smith decides, Temper validates and mutates.

## Validation

In Temper:

```sh
cargo fmt --all
cargo test --workspace --all-targets
cargo dev-clippy
cargo dev-check
```

In Smith:

```sh
cargo fmt --all
cargo test --workspace --all-targets
```

Run the documented Temper+Smith real-world/e2e gates when prerequisites are
available. At minimum, verify the commands exist, are ignored/env-gated rather
than deleted, and are referenced from the coverage ledger and Smith README.
