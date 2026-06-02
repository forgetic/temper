# Phase 4 — Local service API for external frontends

Expose the product-manager conversation core over a loopback API so a rich web
app, Android/PWA wrapper, or voice UI in another repository can use it. This
repository still does **not** implement the web app itself.

## Bootstrap

1. Follow the normal session bootstrap in `AGENTS.md`.
2. Read:
   - `plans/product-manager-chat/README.md`
   - Phase 3 product-chat core and binary
   - `crates/temper-production/src/trigger.rs` / `trigger_args.rs` for simple
     production HTTP server style, if useful
   - `docs/README.md` to choose where to place API docs
3. Keep this repo focused on integration/protocol. Do not add frontend assets,
   React/Vite/etc., or Android code.

## Goal

Add a `serve` mode to the product-manager chat binary:

```sh
temper-product-manager-chat serve \
  --bind 127.0.0.1:39200 \
  --base-url https://git.ekanayaka.io \
  --repo ai/temper \
  --auth chatgpt-oauth
```

External UIs can call this local API to:

- create/resume product conversation sessions;
- send human messages;
- receive product-manager replies and draft updates;
- file a draft as a workflow intake issue;
- read transcript/created issue URLs.

## API scope

Keep the API intentionally small. Suggested endpoints:

```text
GET  /health
POST /sessions
GET  /sessions/{id}
POST /sessions/{id}/messages
POST /sessions/{id}/drafts/{slug}/file
GET  /sessions/{id}/events   # optional SSE for streaming/status updates
```

JSON shapes should be stable and documented. Example `POST /messages` response:

```json
{
  "reply": "I would start with...",
  "drafts": [
    { "slug": "matrix-adapter", "title": "...", "body": "..." }
  ],
  "transcript_url": "https://git.ekanayaka.io/ai/temper/issues/3"
}
```

The service may initially run one request at a time per session. Do not overbuild
multi-user concurrency until the external UI needs it.

## Auth / safety

The service is intended for local use by a trusted frontend, but it still needs a
simple guard:

- bind to `127.0.0.1` by default;
- allow non-loopback bind only with an explicit flag;
- support an optional bearer token env such as
  `TEMPER_PRODUCT_CHAT_SERVICE_TOKEN` and require it when binding outside
  loopback;
- never expose Forgejo/LLM tokens to clients;
- keep product-manager filing behind the explicit file endpoint.

Secrets still come from env, same as Phase 3.

## Reuse Phase 3 core

Do not duplicate REPL logic. The `serve` mode should call the same core used by
`repl`:

- transcript create/resume;
- append human turn;
- call product-manager agent;
- append product-manager reply;
- store latest drafts;
- file draft idempotently.

If Phase 3 left core logic too CLI-specific, refactor it here before adding the
server.

## Documentation

Add a concise reference page, for example:

```text
docs/reference/product-manager-chat-api.md
```

It should document:

- what this repo provides vs. what external UI repos provide;
- CLI/env configuration;
- endpoint list;
- request/response schemas;
- idempotency and transcript/filing semantics;
- safety assumptions for loopback use.

Also link it from `docs/reference/README.md` or `docs/how-to/README.md` as
appropriate.

## Tests

Default tests must not hit live Forgejo or LLM providers.

Add tests with fake product-chat core dependencies covering:

- server binds loopback by default;
- non-loopback bind requires explicit opt-in and service token;
- unauthenticated request is rejected when service token is configured;
- `POST /sessions` creates a session response;
- `POST /messages` returns reply/drafts;
- `POST /drafts/{slug}/file` returns existing issue on repeated calls.

Run:

```sh
cargo fmt --all
cargo test -p temper-production product_chat
cargo dev-check
```

## Acceptance criteria

- External frontends can drive product-manager conversations through a local API.
- The API includes no web app assets and no frontend framework.
- Transcript and filing behavior matches the CLI MVP.
- The API is documented well enough for a separate web/PWA repo to start.
