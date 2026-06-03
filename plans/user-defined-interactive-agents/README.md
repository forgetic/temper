# User-defined interactive agents — implementation plan

This plan completes the direction from Forgejo issue #3 and the
`interactive-agent-interfaces` extraction: Temper should have **no production
knowledge of any concrete interactive role/profile** such as `product-manager`.
Product-manager remains a useful dogfood/example profile, but the core project
should expose a user-defined interaction system in the same spirit as workflow
specification, validation, compilation, and execution.

Status: ☑ complete.

Hand the prompt files to implementation agents **one phase at a time, in order**.
Each phase should land green, update this README's status, and record the
validation it ran.

## Problem statement

The lower-level foundation is already mostly right:

- `temper-interaction` has provider-neutral conversation ids, participants,
  turns, responder request/reply types, inert proposals, process responder
  support, Forge-backed transcripts, and generic transport DTOs.
- Product-manager chat is separated from workflow role workers and the responder
  cannot mutate Forge state directly.
- Filing currently happens only after explicit acceptance and is idempotent.

The production surface used to be too concrete:

- `temper-production` had `product_chat*` modules and the
  `temper-product-manager-chat` binary.
- Profile id, marker namespace, transcript title prefix, labels, draft issue DTOs,
  slash commands such as `/file`, and the accepted-issue transaction were encoded
  as product-manager behavior.
- The generic acceptance helper was effectively "issue proposal -> create an
  intake issue with one configured label" rather than a compiled user-defined
  acceptance transaction.
- Active sessions/latest proposals were in-memory enough that a generic frontend
  could not reliably treat proposals as durable state after restart.

This plan turned those product-manager assumptions into **profile spec fixtures**
and moved the production runtime to compiled user-defined interaction profiles.

## Target architecture

```text
RawInteractionSpec
        │ parse YAML/JSON/TOML or generated config
        ▼
ValidatedInteractionSpec
        │ normalized profiles, proposal kinds, commands, acceptance policies
        ▼
CompiledInteractionRuntime
        │ profile manifests, responder manifests, transcript manifests,
        │ proposal/acceptance manifests, transport command manifests
        ▼
Interaction service / REPL / HTTP-SSE / Matrix / mobile / voice adapters
        │ append turns, invoke process responder, persist proposals/events,
        │ execute explicit accepted actions idempotently
        ▼
Forge transcripts and normal Forge/workflow artifacts
```

Concrete behavior belongs in user config or external responder processes:

- profile/persona name (`product-manager`, `support-agent`, `release-planner`, …);
- transcript label/title/marker policy;
- proposal kinds and payload contracts;
- command aliases/buttons (`/file`, `/accept`, reactions, web buttons, voice
  commands);
- accepted-action transactions, including which labels mean workflow intake;
- responder command/binding and provider-specific prompt implementation.

Temper core should know only about generic interaction primitives, closed
provider-neutral effects, idempotency, validation, and runtime authority
boundaries.

## Non-goals

- Do not add chat/watch/realtime APIs to the `Forge` trait.
- Do not expose broad Forge mutation tools to interactive responders.
- Do not move provider SDKs or auth-file handling into Temper.
- Do not implement a rich web app, Android app, or live voice UI in this plan;
  expose generic adapter seams those frontends can consume.
- Do not remove product-manager dogfood usability before a configured example
  profile replaces it.

## Spec sketch

The exact Rust/API shape may evolve during implementation, but the user-visible
contract should support these concepts.

```json
{
  "id": "dogfood-interactions",
  "profiles": [{
    "id": "product-manager",
    "transcript": {
      "target": "issue",
      "title_prefix": "Product conversation",
      "labels": ["product"],
      "marker_namespace": "product-chat",
      "recent_turn_limit": 30
    },
    "participants": {
      "human": { "display_name": "human" },
      "agent": { "display_name": "product-manager" }
    },
    "responder": {
      "id": "product-manager-responder",
      "protocol": "process-v1",
      "required": true
    },
    "proposal_kinds": [{
      "id": "issue",
      "payload": "issue_draft"
    }],
    "commands": [{
      "id": "file-draft",
      "aliases": ["/file"],
      "transport": "repl",
      "action": { "accept_proposal": { "kind": "issue" } }
    }],
    "acceptance_actions": [{
      "id": "file-draft",
      "proposal_kind": "issue",
      "idempotency_key": "${conversation.id}:${proposal.id}",
      "effects": [{
        "create_issue": {
          "title": "${proposal.payload.title}",
          "body_template": "${proposal.payload.body}\n\n---\nTranscript: ${conversation.transcript_url}",
          "labels": ["untriaged"],
          "marker_namespace": "product-chat"
        }
      }]
    }]
  }]
}
```

The product-manager values above are an **example fixture**, not production
constants. Validation should reject references to undeclared proposal kinds,
commands, responders, effects, labels/policies, and template variables where the
contract can check them.

## Phases

Status legend: ☐ pending · ☑ done · ⚠ blocked

1. ☑ **Phase 1 — Interaction spec and validation contract.**
   `prompts/phase-1-interaction-spec-and-validation.md`

   Done: `temper-interaction` now has raw serde-loadable interaction spec types,
   typed validated profile/spec/responder/command/action ids, diagnostic-collecting
   validation, transcript/participant/responder/proposal/command/acceptance
   policy models, and a closed Phase 1 `create_issue` effect contract. The
   product-manager dogfood behavior is encoded as
   `crates/temper-interaction/fixtures/product-manager-interaction-spec.json`,
   and tests cover generic non-product profiles, the product fixture, duplicates,
   bad references, unknown fields, alias conflicts, unsupported contracts, and
   absence of `product-manager` special-casing. Validation run: `cargo fmt --all`;
   `cargo test -p temper-interaction --all-targets`;
   `cargo test -p temper-production product_chat`; `cargo dev-clippy`;
   `cargo dev-check`.

2. ☑ **Phase 2 — Compile profiles to manifests and generic session config.**
   `prompts/phase-2-compiled-profile-manifests.md`

   Done: `temper-interaction` now compiles `ValidatedInteractionSpec` into
   deterministic `CompiledInteractionSpec` profile manifests covering profile,
   transcript, responder, proposal, command, and acceptance data. Forge
   transcript/session configs can be built from compiled profile manifests,
   transcript and accepted-issue label sets are manifest-driven, and the
   product-chat compatibility path loads the checked-in product-manager fixture
   manifest instead of runtime profile constants. Tests cover deterministic
   compilation, arbitrary manifest-to-session config construction,
   product-manager fixture compatibility, and absence of `product-manager` in
   compiler/session implementation files. Validation run: `cargo fmt --all`;
   `cargo test -p temper-interaction --all-targets`;
   `cargo test -p temper-production product_chat`; `cargo dev-clippy`;
   `cargo dev-check`.

3. ☑ **Phase 3 — Generic durable proposals and acceptance transactions.**
   `prompts/phase-3-generic-proposals-and-acceptance.md`

   Done: `temper-interaction` now has a manifest-driven `AcceptanceExecutor`, a
   closed effect set for `create_issue` plus idempotent transcript acceptance
   comments, template-rendered title/body/label/assignee fields, generic hidden
   acceptance markers from declared idempotency keys, and typed accepted target
   results. Agent replies persist hidden proposal snapshots in transcript
   comments, so restart/resume can reconstruct latest proposals and accept them
   without process cache. Product-chat compatibility routes now execute through
   the generic manifest path while `/file` remains only the fixture command alias.
   Validation run: `cargo fmt --all`; `cargo test -p temper-interaction
   --all-targets`; `cargo test -p temper-production product_chat`;
   `cargo dev-clippy`; `cargo dev-check`.

4. ☑ **Phase 4 — Generic deployable interaction service and transport commands.**
   `prompts/phase-4-generic-service-and-transports.md`

   Done: `temper-production` now ships a generic `temper-interaction` binary
   with `repl` and `serve` subcommands. It loads JSON interaction specs,
   validates/compiles profile manifests, applies a separate deployment binding
   file for Forge token env names, repository selection, service bind/auth, and
   process-responder command/cwd/env/timeout bindings, then exposes profile-neutral
   REPL and HTTP/event routes across bound profiles. Generic REPL help, proposal
   rendering, and aliases such as `/file` are driven from command/proposal
   manifests; local commands stay out of responder transcripts. The generic HTTP
   service keeps event snapshots with `streaming:false`, while the existing
   product-manager binary and `/sessions`/draft-file routes remain compatibility
   aliases. Validation run: `cargo fmt --all`; `cargo test -p temper-interaction
   --all-targets`; `cargo test -p temper-production --all-targets`; `cargo test
   -p temper-production product_chat --all-targets`; `cargo dev-clippy`; `cargo
   dev-check`.

5. ☑ **Phase 5 — Dogfood/profile migration and product-manager demotion.**
   `prompts/phase-5-dogfood-profile-migration.md`

   Done: dogfood now has a checked-in example product-manager interaction spec
   under `examples/dogfood/config/interaction-profiles/`. `./run.sh
   product-chat` builds and launches the generic `temper-interaction` REPL with
   that spec, a generated deployment binding file, generic Forge token env names,
   and Smith process-responder bindings. Dogfood tests cover the example spec,
   binding generation, product-label safety rails, and credential mapping while
   existing product-chat compatibility tests remain green. Validation run:
   `cargo fmt --all`; `cargo test -p temper-interaction --all-targets`; `cargo
   test -p temper-production --all-targets`; `python3 -m unittest discover -s
   examples/dogfood/tools -p '*_test.py'`; `sh -n examples/dogfood/run.sh`;
   `cargo dev-clippy`; `cargo dev-check`.

6. ☑ **Phase 6 — Remove concrete-profile production surfaces and add guards.**
   `prompts/phase-6-remove-concrete-profile-surfaces.md`

   Done: product-chat production modules, the historical product-manager binary,
   product-specific DTOs/routes/env parsing, and reference API docs were removed.
   Dogfood product-manager remains available through the example interaction spec
   and generic `temper-interaction` deployment bindings. Generic runtime tests now
   exercise the dogfood fixture through `ForgeInteractionSession`, and
   `interaction_source_guard_tests` plus the final `rg` guard keep non-test,
   non-fixture `crates/` sources free of concrete profile strings. Validation
   run: `cargo fmt --all`; `cargo test -p temper-interaction --all-targets`;
   `cargo test -p temper-production --all-targets`; `python3 -m unittest
   discover -s examples/dogfood/tools -p '*_test.py'`; `sh -n
   examples/dogfood/run.sh`; `cargo dev-clippy`; `cargo dev-check`; final
   production-source `rg` guard (no output, exit 1 because there were no hits).

## Whole-plan acceptance criteria

Status: ☑ all criteria satisfied by Phase 6.

- A user can define a new interactive agent/profile without adding Rust code to
  Temper production paths.
- Product-manager is represented as an example spec, fixture, dogfood config, or
  external responder profile only.
- Runtime code accepts compiled profile manifests and does not encode concrete
  profile ids, labels, marker namespaces, slash commands, or transaction meaning.
- Interactive responders still cannot mutate Forge or workflow state directly;
  they return replies and inert proposals only.
- Proposal acceptance is explicit, idempotent, and executes only declared effects.
- A restarted service can resume a transcript and expose enough durable proposal
  state to avoid losing accepted-action options.
- Generic REPL/HTTP/event APIs can be used by web, Matrix, mobile, or voice
  adapters without product-manager-specific route names.
- Dogfood product-manager chat keeps working as an example deployment.
- `temper-forge`, `temper-workflow`, and `temper-runner` stay free of chat UI,
  concrete interactive profile, and LLM-provider dependencies.

## Regression guard target

By the end of Phase 6, a check like this should report no non-test/non-fixture
production hits, except paths explicitly under examples, docs, or plans:

```sh
rg -n "product-manager|ProductChat|product_chat|product-chat|Product conversation|TEMPER_PRODUCT_CHAT|/file " \
  crates \
  --glob '!**/*test*' \
  --glob '!**/tests/**' \
  --glob '!**/fixtures/**'
```

A separate example/docs grep may still find product-manager because the dogfood
fixture intentionally exercises that profile.

## Validation expectations

Every code phase should run at least:

```sh
cargo fmt --all
cargo test -p temper-interaction --all-targets
cargo test -p temper-production --all-targets
cargo dev-clippy
cargo dev-check
```

When a phase touches workflow-like validation/compilation, also run the relevant
`temper-interaction` spec/compiler tests added by that phase. When shell/Python
examples are touched, run their focused tests and `sh -n` on modified launchers.

## Relevant starting points

- `docs/explanation/interactive-agent-interfaces.md`
- `docs/reference/interactive-conversation-interface.md`
- `docs/reference/interactive-process-responder-protocol.md`
- `plans/interactive-agent-interfaces/README.md`
- `crates/temper-interaction/src/{types,proposal,transcript,session,transport,process}.rs`
- `crates/temper-production/src/interaction_*.rs`
- `crates/temper-production/src/bin/temper-interaction.rs`
- `examples/dogfood/run.sh`
- `examples/dogfood/config/dogfood.env`
