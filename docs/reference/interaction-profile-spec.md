# Interaction profile spec

`temper-interaction` defines a user-authored interaction profile contract in
three phases:

```text
RawInteractionSpec -> ValidatedInteractionSpec -> CompiledInteractionSpec
```

Runtime or compiler APIs should consume `ValidatedInteractionSpec` or a compiled
manifest, not a raw spec. The validated model has no public constructor; callers
obtain one through `RawInteractionSpec::validate` or `validate_interaction_spec`.
Compilation is infallible after validation and preserves declaration order for
deterministic runtime manifests.

## Raw spec scope

Raw spec structs live in `temper_interaction::spec` and derive serde
`Deserialize`/`Serialize` with `deny_unknown_fields`. They use plain `String` ids
so validation can report all duplicate, malformed, and dangling references in
one pass.

A spec declares:

- `id`: deterministic interaction spec id.
- `responders`: process responder declarations with `id`, `protocol`, and
  `required`. Phase 1 supports `protocol: "process-v1"`.
- `profiles`: interactive profiles with a profile id, transcript policy,
  participants, responder reference, proposal kinds, commands, and acceptance
  actions.

Each profile declares:

- `transcript`: Phase 1 supports Forge issue transcripts only (`target: "issue"`),
  an exact label set (`label_policy: "exact"`), a non-empty title prefix,
  non-empty labels, a deterministic marker namespace, and a recent-turn limit.
- `participants`: `human` and `agent` display names used for transcript and
  responder-facing turns.
- `responder`: id of a declared responder.
- `proposal_kinds`: proposal kind ids and payload contracts. Phase 1 supports
  `payload: "issue_draft"`, compatible with `IssueProposal`.
- `commands`: transport command ids, aliases such as `/file`, and an
  `accept_proposal` action referencing a proposal kind and acceptance action.
- `acceptance_actions`: explicit proposal-acceptance policy, idempotency key
  template, and a closed effect list.

Phase 1's closed effect set contains `create_issue`, with title/body templates,
labels, marker namespace, and optional transcript backlink metadata. This can
represent the current dogfood issue-filing behavior without making
`product-manager` a production constant.

## Validation contract

Validation returns `InteractionSpecValidationErrors`, a collection of
`InteractionSpecDiagnostic` values. It reports:

- duplicate responder/profile/proposal-kind/command/acceptance-action ids;
- ids or marker namespaces that violate the deterministic slug rule;
- references to undeclared responders, proposal kinds, commands, or acceptance
  actions;
- empty transcript labels, title prefixes, or marker namespaces;
- empty command aliases and alias conflicts within one profile;
- unsupported responder protocols, transcript target/policy values, payload
  contracts, acceptance policies, or effect kinds;
- empty required acceptance fields such as idempotency keys or create-issue
  labels/templates.

Validation is profile-neutral: the literal id `product-manager` has no special
meaning.

## Compiled manifests

`ValidatedInteractionSpec::compile` and `compile` project profiles into
`CompiledInteractionSpec`. Each `CompiledProfileManifest` contains:

- `ProfileManifest`: profile id, human/agent participants, recent-turn limit;
- `TranscriptManifest`: Forge transcript target, exact labels, title prefix,
  label policy, and marker namespace;
- `ResponderManifest`: responder id, protocol, and required flag;
- `ProposalManifest`: proposal kind ids plus payload validators such as
  `IssueDraft`;
- `CommandManifest`: command ids, aliases, and accept-proposal action mapping;
- `AcceptanceManifest`: accepted-action id, proposal kind, explicit acceptance
  policy, idempotency key template, and declared effects.

Forge transcript/session configs can be built from a compiled profile manifest.
The current issue-intake session path reads created-issue labels and marker
namespace from the manifest's `create_issue` effect; generic effect execution is
left to a later phase.

## Fixture

The product-manager dogfood behavior is encoded as a fixture, not as the generic
contract:

`crates/temper-interaction/fixtures/product-manager-interaction-spec.json`

It declares the `product` transcript label, `untriaged` filed-issue label,
`/file` alias, `issue` proposal kind, and explicit issue-proposal acceptance.
