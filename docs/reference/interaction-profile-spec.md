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
deterministic runtime manifests. Deployable services keep credentials and local
paths in a separate binding file; profile specs define behavior only.

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

The closed effect set contains:

- `create_issue`: creates a Forge issue using title/body/label/assignee
  templates, a marker namespace, optional marker key, and optional transcript
  backlink metadata.
- `add_transcript_comment`: appends an idempotent comment to the transcript using
  a body template, marker namespace, and optional marker key.

This can represent the current dogfood issue-filing behavior without making
`product-manager` a production constant. Templates currently support
`${conversation.id}`, `${conversation.transcript_url}`, `${proposal.id}`,
`${proposal.kind}`, `${proposal.title}`, `${proposal.summary}`,
`${proposal.payload.<field>}`, `${human.handle}`, `${acceptance.action_id}`,
`${idempotency.key}`, and `${effect.marker}`.

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

Forge transcript/session configs are built from compiled profile manifests. The
`AcceptanceExecutor` consumes a manifest, transcript state, selected proposal id,
Forge handle, and durable proposal data. It reloads the repository and transcript
issue before mutating, renders the declared idempotency key, searches for hidden
markers before create/append operations, and returns a typed accepted target. If
an effect omits `marker_key`, the acceptance action id is used; the product
fixture declares `marker_key: "file"` to preserve its historical hidden marker.

## Durable proposal state

Agent transcript comments include a human-readable proposal summary plus a hidden
`temper:<marker-namespace>-proposals-v1` marker containing a hex-encoded JSON
proposal snapshot. On resume, the Forge-backed transcript loader reads the
newest agent snapshot, validates proposal ids/payloads, strips the hidden marker
from responder-facing turns, and repopulates the latest proposal list. A restarted
service can therefore resume a transcript issue, list the latest proposals, and
accept one without relying on the old process cache.

## Deployment bindings

`temper-interaction` uses a separate JSON deployment binding file. It names the
Forge base URL, repository (`owner/name`), optional `default_profile`, profile
bindings (`human_token_env` and `agent_token_env`), responder bindings keyed by
responder id (`command`, `args`, `cwd`, `env_allowlist`, `timeout_secs`), and
service settings (`bind`, optional bearer `token_env`, and
`allow_non_loopback`). Secrets are loaded from the named environment variables;
they are not profile-spec data and are not passed on argv.

## Fixture and example

The product-manager dogfood behavior is encoded as fixture/example data, not as
the generic contract:

- `crates/temper-interaction/fixtures/product-manager-interaction-spec.json`
- `examples/dogfood/config/interaction-profiles/product-manager.json`

They declare the `product` transcript label, `untriaged` filed-issue label,
`/file` alias, `issue` proposal kind, and explicit issue-proposal acceptance.
The `/file` text is transport alias data; acceptance executes the generic
`accept_proposal` action and manifest effects.
