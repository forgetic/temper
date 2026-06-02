# Phase 2 — Product-manager conversational agent

Add the LLM-side product-manager adapter. This is a conversational planning
agent, not a workflow queue worker.

## Bootstrap

1. Follow the normal session bootstrap in `AGENTS.md`.
2. Read:
   - `plans/product-manager-chat/README.md`
   - `crates/temper-agents/src/decision.rs`
   - `crates/temper-agents/src/provider.rs`
   - `crates/temper-agents/src/owner.rs` and `src/prompts/owner.md` for contrast
   - `crates/temper-agents/src/registry.rs` for contrast only
   - `docs/reference/workflow-layer.md` section on agent authority boundaries
3. Do not register product-manager as a `temper_runner::Agent` or add it to
   `reference-delivery.json`.

## Goal

Expose a reusable `temper-agents` product-manager component that can run one
LLM turn over a conversation transcript and return:

- a conversational reply; and
- zero or more draft intake issues.

It must not mutate Forgejo or call workflow tools. Mutation remains in the
integration layer added in Phase 3.

## API shape

Add a focused module, for example `crates/temper-agents/src/product_manager.rs`,
with public types similar to:

```rust
pub struct ProductManagerAgent {
    provider: ProviderConfig,
}

pub struct ProductManagerConversationTurn {
    pub author: ProductManagerAuthor, // Human | ProductManager
    pub body: String,
}

pub struct ProductManagerRequest {
    pub repository: String,
    pub transcript_url: Option<String>,
    pub turns: Vec<ProductManagerConversationTurn>,
}

pub struct ProductManagerResponse {
    pub reply: String,
    pub drafts: Vec<ProductManagerDraftIssue>,
}

pub struct ProductManagerDraftIssue {
    pub slug: String,
    pub title: String,
    pub body: String,
    pub rationale: Option<String>,
}
```

Names can differ, but keep the separation clear: request/response only, no Forge
mutation.

Add `src/prompts/product_manager.md` and export it through `prompts.rs`.

## Prompt requirements

The prompt should make the product-manager role distinct from the workflow
owner:

- discuss product direction and feature ideas;
- ask clarifying questions when needed;
- propose small, fileable intake issues;
- never claim an issue was filed unless the integration layer tells it so;
- prefer cheap MVPs and dogfood feedback loops;
- keep drafts suitable for the existing architect/engineer workflow;
- output exactly one JSON object.

Suggested JSON shape:

```json
{
  "reply": "short conversational response",
  "drafts": [
    {
      "slug": "stable-lowercase-id",
      "title": "Issue title",
      "body": "Issue body to file as intake",
      "rationale": "optional reason"
    }
  ]
}
```

`slug` must be stable and deterministic because Phase 3 will use it in filing
correlation keys. The model must not include random ids or timestamps.

## Implementation details

- Reuse `run_decision` and `ProviderConfig`.
- Do not register SDK tools.
- For Anthropic OAuth, rely on the existing `run_decision` system-identity path.
- Keep parsing tolerant only to the extent `run_decision` already is; the prompt
  should still require a single JSON object.
- If provider setup fails, surface the setup error. If parsing fails in a CLI
  context later, Phase 3 can choose how to present it; this module should return
  typed errors.

## Tests

Add offline unit tests covering:

- JSON response parsing with zero drafts;
- JSON response parsing with multiple drafts;
- draft slug validation helper if you add one;
- prompt export is wired through `prompts.rs`.

Do **not** add a default live LLM test. If you add a live smoke later, gate it
behind an env var and `#[ignore]` like existing `temper-agents` live tests.

Run:

```sh
cargo fmt --all
cargo test -p temper-agents product_manager
cargo dev-check
```

## Documentation updates

- Update `crates/temper-agents/src/lib.rs` module docs to mention the
  product-manager conversational adapter as non-workflow.
- Update `plans/product-manager-chat/README.md` phase status when complete.

## Acceptance criteria

- `temper-agents` exposes a product-manager conversation adapter.
- The adapter is not a workflow role and performs no Forge mutation.
- It returns structured reply + draft intake issues.
- Default tests remain hermetic.
