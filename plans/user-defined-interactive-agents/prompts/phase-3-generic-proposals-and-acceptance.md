# Phase 3 prompt — generic durable proposals and acceptance

You are implementing Phase 3 of
`plans/user-defined-interactive-agents/README.md`. Assume Phases 1-2 are done.

## Session bootstrap

Read the normal session docs plus:

- `plans/user-defined-interactive-agents/README.md`
- Phase 1-2 changes
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/interaction-profile-spec.md` if present
- `crates/temper-interaction/src/{proposal,session,transcript,transport}.rs`
- `crates/temper-production/src/product_chat*.rs`
- `docs/reference/forge-interface.md` for idempotency and Forge mutation rules

## Goal

Replace the product-manager-shaped issue-intake acceptance helper with a generic
acceptance executor over compiled acceptance manifests. Proposal acceptance should
be explicit, idempotent, and driven by user-defined effects. Proposals should be
durable or reconstructable after service restart.

## Tasks

1. Design a closed generic interaction acceptance effect model.

   The initial effect set should be enough to express current dogfood behavior:

   - create Forge issue with title/body/labels/assignees from templates and
     proposal payload;
   - render transcript backlink and hidden idempotency marker;
   - optionally add a comment to the transcript recording acceptance;
   - return a typed accepted target reference.

   Keep future effects extensible but do not expose arbitrary Forge mutation
   tools to responders.

2. Add an `AcceptanceExecutor` or equivalent runtime that consumes:

   - compiled profile/acceptance manifest;
   - current conversation/transcript state;
   - selected proposal id;
   - Forge handles for the authorized human/agent/service identity;
   - current durable proposal data.

   It must reload current Forge state before mutating and implement idempotency
   using declared correlation/marker keys.

3. Make proposal state durable or reconstructable.

   Choose one small design and document it. Examples:

   - append machine-readable proposal blocks to agent transcript comments;
   - add separate hidden proposal markers/comments;
   - reconstruct latest proposals from the latest agent reply comment if it
     contains serialized proposal metadata.

   The result should allow a restarted service to resume a transcript, list the
   latest proposals, and accept an unaccepted proposal without relying on the old
   process memory cache.

4. Replace `IssueIntakeAcceptanceConfig` / hard-coded issue filing call sites
   with manifest-driven acceptance. Keep old APIs only as deprecated/test helper
   wrappers if necessary.

5. Move `/file` semantics out of acceptance. Acceptance should be a generic
   command/action such as `accept_proposal`; `/file` is just an alias declared in
   the profile command manifest and interpreted by a transport adapter.

6. Add tests for:

   - arbitrary profile issue-creation acceptance;
   - product-manager fixture acceptance preserving current behavior;
   - idempotent retry;
   - restart/resume then accept latest proposal;
   - unsupported proposal kind/effect rejection;
   - no direct responder Forge mutation.

7. Update docs and the plan status.

## Constraints

- Do not change the `Forge` trait unless a portable gap is unavoidable and fully
  documented.
- Do not use provider-specific Forgejo behavior in `temper-interaction`.
- Do not log or expose secrets/full provider errors through transport responses.
- Preserve explicit human acceptance.

## Validation

Run and record:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production product_chat
cargo dev-clippy
cargo dev-check
```
