# Product-manager chat local API

This API is the current `product-manager` profile instance of Temper's broader
interaction-plane target. The generic interactive conversation contract is
specified in
[Interactive conversation interface](interactive-conversation-interface.md), and
its domain request/reply/proposal types and provider-neutral process responder
adapter now live in `temper-interaction`. The product-manager binary and
endpoints are compatibility profile wiring; do not treat their names as the
framework abstraction.

Temper currently provides the product-manager conversation **integration
surface**: Forgejo transcripts, product-manager responder turns, draft issue
proposals, and explicit filing into the normal workflow. Product-manager draft
intake issues are one proposal type: they are inert until a human explicitly
accepts one for filing. External web, mobile, Matrix, and voice frontends should
target the generic interaction API and select `product-manager` as a profile.
Concrete product-manager responder implementations, including pi-SDK-backed
ones, can live out of process behind the
[generic responder protocol](interactive-process-responder-protocol.md);
frontends still talk to Temper's interaction service rather than directly to that
responder process. The `/sessions` and `/drafts/{slug}/file` routes below are
compatibility aliases for the product-manager profile. This repository does not
ship external frontends.

## Command

```sh
temper-product-manager-chat serve \
  --bind 127.0.0.1:39200 \
  --base-url https://git.ekanayaka.io \
  --repo ai/temper \
  --auth chatgpt-oauth
```

`--bind` defaults to `127.0.0.1:39200`. Non-loopback binds require both
`--allow-non-loopback` and `TEMPER_PRODUCT_CHAT_SERVICE_TOKEN`. By default the
binary uses the in-repo in-process product-manager responder; operators select a
process responder explicitly with `--responder-command` or
`TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND`.

Secrets come from env, never argv:

- `TEMPER_PRODUCT_CHAT_HUMAN_TOKEN`: Forgejo token for human transcript turns.
- `TEMPER_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN`: Forgejo token for product-manager
  replies and filed intake issues.
- `TEMPER_PRODUCT_CHAT_SERVICE_TOKEN`: optional local API bearer token; required
  for non-loopback binds.
- `TEMPER_AGENTS_AUTH`, `TEMPER_AGENTS_CODEX_MODEL`,
  `TEMPER_AGENTS_AUTH_FILE`, and provider-specific auth envs follow the normal
  `temper-agents` rules for the in-process fallback. CLI `--auth`,
  `--codex-model`, and `--auth-file` override the matching defaults.
- `TEMPER_PRODUCT_CHAT_RESPONDER_COMMAND`: external responder program path.
- `TEMPER_PRODUCT_CHAT_RESPONDER_ARGS_JSON`: optional JSON array of responder
  arguments.
- `TEMPER_PRODUCT_CHAT_RESPONDER_CWD`: optional responder working directory.
- `TEMPER_PRODUCT_CHAT_RESPONDER_ENV_ALLOWLIST`: comma-separated env names copied
  to the responder; the child inherits no other env.
- `TEMPER_PRODUCT_CHAT_RESPONDER_TIMEOUT_SECS`: optional one-turn timeout
  (default 60).

Equivalent CLI flags are `--responder-command`, repeated `--responder-arg`,
`--responder-cwd`, repeated `--responder-env`, and
`--responder-timeout-secs`.

## Authentication

When `TEMPER_PRODUCT_CHAT_SERVICE_TOKEN` is set, every request must include:

```text
Authorization: Bearer <token>
```

Forgejo and LLM credentials are never returned in API responses.

## Generic endpoints

The preferred local API is the profile-neutral conversation surface documented in
[Interactive conversation interface](interactive-conversation-interface.md):

```text
POST /conversations
GET  /conversations/{id}
POST /conversations/{id}/turns
GET  /conversations/{id}/proposals
GET  /conversations/{id}/events
POST /conversations/{id}/proposals/{proposal_id}/accept
```

`POST /conversations` accepts optional `profile_id` (defaulting to the configured
profile, currently `product-manager`) and optional `transcript_issue`. Turns use
`{ "body": "..." }`. Events currently return a replay snapshot with
`streaming:false`; SSE remains a transport follow-up.

## Product-manager compatibility endpoints

### `GET /health`

Returns `200` with `{"ok":true}` when the process can accept requests. It still
requires the bearer token when service auth is configured.

### `POST /sessions`

Creates a new product transcript issue, or resumes an existing product transcript
when `transcript_issue` is supplied.

Request:

```json
{ "transcript_issue": 3 }
```

`transcript_issue` is optional. Response (`201`):

```json
{
  "id": "pc-...",
  "transcript_issue": 3,
  "transcript_url": "https://git.ekanayaka.io/ai/temper/issues/3",
  "drafts": []
}
```

The session id is the transcript correlation key. Sessions are in-memory for the
running service; after restart, resume by transcript issue number.

### `GET /sessions/{id}`

Returns the active in-memory session metadata and latest draft list.

### `POST /sessions/{id}/messages`

Appends one human turn to the Forgejo transcript, runs one product-manager
responder turn, appends the product-manager reply, and stores the latest drafts.

Request:

```json
{ "message": "I want a way to talk to the product manager from my phone." }
```

Response (`200`):

```json
{
  "reply": "I would start with...",
  "drafts": [
    {
      "slug": "matrix-adapter",
      "title": "Add product-manager Matrix text adapter",
      "body": "...",
      "rationale": "..."
    }
  ],
  "transcript_url": "https://git.ekanayaka.io/ai/temper/issues/3"
}
```

Slash commands are handled locally before the LLM sees a turn. In particular,
posting `{"message":"/help"}` returns the command list in `reply`; it is not
mirrored to the transcript and does not call the product-manager responder. Only
explicit acceptance through the file endpoint creates workflow intake issues.

### `POST /sessions/{id}/drafts/{slug}/file`

Explicitly accepts and files the latest draft with the matching slug as a normal
workflow intake issue labeled `untriaged`. The created issue body includes a
transcript backlink and a hidden idempotency marker. Repeating the same request
returns the existing issue.

Response (`200`):

```json
{
  "created": false,
  "issue": {
    "number": 4,
    "url": "https://git.ekanayaka.io/ai/temper/issues/4",
    "title": "Add product-manager Matrix text adapter"
  },
  "transcript_url": "https://git.ekanayaka.io/ai/temper/issues/3"
}
```

## Error shape

Errors return JSON:

```json
{ "error": "session not found" }
```

Common statuses are `400` for malformed input, `401` for missing/invalid bearer,
`404` for unknown conversations/sessions/proposals/drafts, and `500` for Forgejo
or responder failures. Transport `500` bodies are sanitized and do not include
raw model, process, credential, or environment details.

## Safety and concurrency

The generic and compatibility APIs are intended for trusted local frontends and
bind to loopback by
default. It runs requests sequentially in the current implementation and may hold
one active request per session. Product transcript issues remain labeled
`product`; workflow intake is created only through the explicit file endpoint.
