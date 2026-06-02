# Product-manager conversations — implementation plan

This plan starts the product-discussion flow we just dogfooded in Forgejo issue
[#3](https://git.ekanayaka.io/ai/harness/issues/3), with two corrections that
shape the design:

1. the conversational role is a new **`product-manager`** agent, not the existing
   workflow `owner` role; and
2. this repository owns the **workflow/integration surfaces** only. Any rich web
   UI, Android app, or live voice UI lives in another repo and talks to the
   integration surface defined here.

Hand the prompt files to the agent loop **one phase at a time, in order**. Each
phase should land green and update this README's status.

## Goal

A human can discuss product ideas with a Harness product-manager agent, have the
conversation mirrored into Forgejo, and explicitly ask the agent to file normal
workflow intake issues on the human's behalf.

The first usable version should be cheap: **no web app**. It should run from the
dogfood setup as an interactive terminal session so we can feel the flow before
building a dedicated frontend.

## What I would do first

Start with the dogfood safety rails and a terminal MVP:

1. make `product` transcript issues safe while dogfood workers/labelers are
   running (no accidental `untriaged` relabeling);
2. add a non-workflow `product-manager` identity and LLM prompt/adapter; then
3. ship a `harness-product-manager-chat` REPL plus
   `examples/dogfood/run.sh product-chat` wrapper.

That gives the smallest feedback loop: type in a terminal, see product-manager
replies, see comments appear in Forgejo under the right identities, and use
`/file <draft>` to create a real `untriaged` issue only when explicitly asked.

## This repo owns vs. does not own

Owned here:

- the `product-manager` LLM adapter and prompt;
- the conversation/draft/filing protocol;
- Forgejo transcript persistence and intake issue creation;
- dogfood wiring and credentials/configuration handling;
- a CLI REPL MVP;
- later, a small local HTTP/SSE API that external UIs can call.

Not owned here:

- the rich web app itself;
- native Android UI;
- live voice capture/playback UI;
- broad generic chat-app functionality;
- changing the reference-delivery workflow to include product discussions.

The product-manager is **not** a workflow role and must not be inserted into
`reference-delivery.json` role queues.

## Cheap MVP flow (no web app)

```text
$ cd examples/dogfood
$ ./run.sh product-chat

Opened product conversation:
  https://git.ekanayaka.io/ai/harness/issues/NN

you> I want a way to talk to the product manager from my phone.

product-manager> I would start with a Matrix text adapter because it gives you
Android immediately, while keeping Forgejo as the transcript and issue backend.

Drafts:
[1] Add product-manager Matrix text adapter
[2] Add product-manager service API for external clients

you> /file 1

product-manager> Filed intake issue:
  https://git.ekanayaka.io/ai/harness/issues/MM
```

Forgejo state:

- the transcript issue has `product` and **not** `untriaged`;
- human comments are authored by the human's Forgejo token;
- product-manager comments are authored by the `product-manager` token;
- filed intake issues are separate issues with `untriaged` and a backlink to the
  transcript;
- filing uses an idempotency marker so repeating `/file 1` returns the existing
  intake issue.

## Non-goals for this plan

- No web UI implementation in this repository.
- No native Android app.
- No live voice implementation.
- No Matrix bot in the first cheap MVP.
- No new methods on the `Forge` trait unless a phase proves a portable gap.
- No broad Forge mutation tools exposed to the LLM. The model proposes drafts;
  the integration layer files issues only after explicit human command.

## Safety constraints

- Transcript issues must never enter the delivery workflow by accident.
- The dogfood intake labeler must not add `untriaged` to issues labeled
  `product`.
- Product-manager issue creation is explicitly outside workflow execution; when
  it creates intake, it creates ordinary Forgejo issues with the workflow's
  entry label and a correlation marker.
- The LLM receives no file/bash/tools. It returns structured text/drafts; all
  Forge mutation happens in the integration layer.
- Secrets travel through env or sourced secret files, never argv or logs.
- The product-manager identity is separate from `owner`; reusing `owner` is at
  most a temporary manual fallback and should not be baked into code.

## Phases

Status legend: ☐ pending · ☑ done

1. ☑ **Phase 1 — Dogfood safety rails + product-manager identity.**
   `prompts/phase-1-dogfood-safety-and-identity.md`

   Done: product transcript issues are skipped by the dogfood intake labeler, the
   dogfood setup ensures the non-workflow `product` label, and optional
   `product-manager` credentials are parsed and included in repo permissions
   without becoming a required workflow role. Validation run:
   `python3 -m py_compile examples/dogfood/tools/*.py`,
   `python3 -m unittest discover examples/dogfood/tools`, `cargo fmt --all`,
   `cargo dev-clippy`, and `cargo dev-check`.

2. ☑ **Phase 2 — Product-manager conversational agent.**
   `prompts/phase-2-product-manager-agent.md`

   Done: `harness-agents` exposes a non-workflow product-manager adapter and
   prompt that run one LLM turn over a transcript and return structured `reply`
   plus draft intake issues with deterministic slugs. It is not a
   `harness_runner::Agent`, registers no SDK tools, and performs no Forge
   mutation. Validation run: `cargo fmt --all`,
   `cargo test -p harness-agents product_manager`, `cargo dev-clippy`, and
   `cargo dev-check`.

3. ☐ **Phase 3 — CLI REPL MVP + Forgejo transcript/filing core.**
   `prompts/phase-3-cli-mvp.md`

   Add `harness-product-manager-chat` and `examples/dogfood/run.sh product-chat`.
   The REPL mirrors every turn to Forgejo, displays product-manager draft issues,
   and supports `/file <n>` to create an idempotent `untriaged` intake issue.
   This is the first human-facing MVP and intentionally has no web UI.

4. ☐ **Phase 4 — Local service API for external frontends.**
   `prompts/phase-4-service-api.md`

   Add a `serve` mode exposing the same conversation core over loopback HTTP
   JSON plus streaming events. The external web/PWA repo consumes this API; this
   repo still ships no frontend assets beyond API docs/examples.

5. ☐ **Phase 5 — Optional Matrix/mobile text adapter.**
   `prompts/phase-5-matrix-mobile-text-adapter.md`

   If the text flow feels useful, add a Matrix bot adapter that uses the same
   conversation core/service. This gives Android access through existing Matrix
   clients without committing to Matrix as the final rich UX. At phase start,
   decide whether the adapter belongs here or in an external repo that consumes
   the Phase 4 API.

## Future tracks outside this plan

- Rich web/PWA frontend in a separate repo, consuming Phase 4's API.
- Native Android wrapper/app only after the PWA/product flow stabilizes.
- Voice sessions: likely external UI handles capture/playback and sends
  transcript/command events into the service API. MatrixRTC/Element Call can be
  evaluated later, but is not the first voice step.

## Acceptance criteria

- A product conversation can run without starting a web app.
- Forgejo captures the transcript as comments under the correct identities.
- Transcript issues stay labeled `product` only and are not routed into the
  workflow.
- `/file <draft>` creates a normal workflow intake issue and is idempotent.
- The product-manager agent is distinct from the workflow owner role.
- External UI work can proceed in another repo against a documented local API.
- `cargo fmt --all`, `cargo dev-clippy`, and `cargo dev-check` pass at each
  code phase; default tests remain hermetic.

## Relevant starting points

- `examples/dogfood/README.md`
- `examples/dogfood/run.sh`
- `examples/dogfood/tools/label_intake.py`
- `examples/dogfood/tools/parse_secrets.py`
- `examples/dogfood/tools/configure_forgejo.py`
- `crates/harness-agents/src/decision.rs`
- `crates/harness-agents/src/prompts/`
- `crates/harness-agents/src/registry.rs` (for contrast: do **not** register the
  product-manager as a workflow role)
- `crates/harness-production/src/bin/`
- `crates/harness-production/src/forgejo_rest.rs`
- `crates/harness-production/src/provision.rs`
- `crates/harness-workflow/src/execute/ensure.rs` (correlation-key pattern)
