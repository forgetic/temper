# Product-manager chat local API

Harness provides the product-manager conversation **integration surface**: Forgejo
transcripts, product-manager LLM turns, draft intake issue storage, and explicit
filing into the normal workflow. External repositories may build a web app,
PWA/Android wrapper, Matrix bridge, or voice UI on top of this API. This
repository does not ship those frontends.

## Command

```sh
harness-product-manager-chat serve \
  --bind 127.0.0.1:39200 \
  --base-url https://git.ekanayaka.io \
  --repo ai/harness \
  --auth chatgpt-oauth
```

`--bind` defaults to `127.0.0.1:39200`. Non-loopback binds require both
`--allow-non-loopback` and `HARNESS_PRODUCT_CHAT_SERVICE_TOKEN`.

Secrets come from env, never argv:

- `HARNESS_PRODUCT_CHAT_HUMAN_TOKEN`: Forgejo token for human transcript turns.
- `HARNESS_PRODUCT_CHAT_PRODUCT_MANAGER_TOKEN`: Forgejo token for product-manager
  replies and filed intake issues.
- `HARNESS_PRODUCT_CHAT_SERVICE_TOKEN`: optional local API bearer token; required
  for non-loopback binds.
- `HARNESS_AGENTS_AUTH`, `HARNESS_AGENTS_CODEX_MODEL`,
  `HARNESS_AGENTS_AUTH_FILE`, and provider-specific auth envs follow the normal
  `harness-agents` rules. CLI `--auth`, `--codex-model`, and `--auth-file`
  override the matching defaults.

## Authentication

When `HARNESS_PRODUCT_CHAT_SERVICE_TOKEN` is set, every request must include:

```text
Authorization: Bearer <token>
```

Forgejo and LLM credentials are never returned in API responses.

## Endpoints

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
  "transcript_url": "https://git.ekanayaka.io/ai/harness/issues/3",
  "drafts": []
}
```

The session id is the transcript correlation key. Sessions are in-memory for the
running service; after restart, resume by transcript issue number.

### `GET /sessions/{id}`

Returns the active in-memory session metadata and latest draft list.

### `POST /sessions/{id}/messages`

Appends one human turn to the Forgejo transcript, runs one product-manager LLM
turn, appends the product-manager reply, and stores the latest drafts.

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
  "transcript_url": "https://git.ekanayaka.io/ai/harness/issues/3"
}
```

Slash commands are handled locally before the LLM sees a turn. In particular,
posting `{"message":"/help"}` returns the command list in `reply`; it is not
mirrored to the transcript and does not call the product-manager model. Only the
explicit file endpoint creates workflow intake issues.

### `POST /sessions/{id}/drafts/{slug}/file`

Files the latest draft with the matching slug as a normal workflow intake issue
labeled `untriaged`. The created issue body includes a transcript backlink and a
hidden idempotency marker. Repeating the same request returns the existing issue.

Response (`200`):

```json
{
  "created": false,
  "issue": {
    "number": 4,
    "url": "https://git.ekanayaka.io/ai/harness/issues/4",
    "title": "Add product-manager Matrix text adapter"
  },
  "transcript_url": "https://git.ekanayaka.io/ai/harness/issues/3"
}
```

## Error shape

Errors return JSON:

```json
{ "error": "session not found" }
```

Common statuses are `400` for malformed input, `401` for missing/invalid bearer,
`404` for unknown sessions/drafts, and `500` for Forgejo or LLM failures.

## Safety and concurrency

The API is intended for a trusted local frontend and binds to loopback by
default. It runs requests sequentially in the current implementation and may hold
one active request per session. Product transcript issues remain labeled
`product`; workflow intake is created only through the explicit file endpoint.
