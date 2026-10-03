# LLM providers

Provisional, 2026-10-03. How an agent talks to LLM providers: the stack of
machines on each connection, how a prompt becomes a provider's request and
its streamed answer becomes a completion, how failures are classified, and
the server side the fake provider shows temper. Two providers from the
start, both over OAuth: Anthropic's Messages API, and ChatGPT's (OpenAI's
Responses API as ChatGPT serves it to an account). The overview is
`protocol.md` (section 6); credentials, grants and the refreshing of
accounts are `credentials.md`'s; the conversation's vocabulary is the
session's (agent-domain.md, section 5). skein's machines are designed in
`http.md`, `json.md` and `tls.md`. What is open is listed in section 15.

## 1. In one page

- **One call, one exchange.** A session's `Complete` becomes one HTTP POST
  on a connection of its own for the call's life, and a stream of
  server-sent events that ends with one completion or one failure. The
  domain's `Cancel` aborts the connection.
- **The stack:** io's socket, TLS, the HTTP client, the server-sent events
  reader, a JSON tokenizer per event, and the provider's decoder. Each
  machine has its own limits; the provider's decoder holds the completion
  as it grows, within the session's.
- **A provider is a dialect.** `temper-llm-anthropic` and
  `temper-llm-openai` each hold one provider's documents, both sides: the
  request a client writes and a server reads, and the events a server
  writes and a client reads. They know nothing of temper's domains.
- **The agent's protocol layer translates.** It turns a prompt into the
  dialect's request, and the dialect's answer into a completion. It owns
  the schemas of the tools a prompt offers, decodes each tool call's input
  into a typed call on the way in, and renders what came of a call as the
  text the LLM reads. These are provider-neutral, written once.
- **What the provider keeps, it gets back.** Anthropic's thinking blocks
  and ChatGPT's encrypted reasoning go up as opaque blocks, kept by the
  session in order and sent back verbatim, never read.
- **Failures are classified, never retried.** Status, the provider's error
  type and its rate-limit headers map onto the session's failures, with
  when to try again. An exhausted account is told apart from a rate
  limit. Whether to try again is the session's decision.
- **Credentials by name.** Each `Complete` names the grant to use; the
  agent's protocol layer puts the token, and ChatGPT's account id, into
  the request from its table (credentials.md, section 4). A request
  refused as unauthorized fails as such; with no grant left, the call
  fails before it reaches the provider.
- **Each provider's identity in one place.** The headers, the leading
  system blocks and the names a provider's subscription route expects are
  data in one module per dialect, refreshed from transcripts of the
  provider's own client when they change.
- **Tested against real traffic.** Transcripts captured from the
  providers' own clients and from temper's are the decoders' oracles; the
  fake provider serves both dialects to the agent through bytes.

## 2. In temper

```
crate                   depends on                          holds
temper-llm-anthropic    skein-lib, skein-json               the Messages API's documents: requests, events, errors
temper-llm-openai       skein-lib, skein-json               the Responses API's, as ChatGPT serves it
temper-agent-protocol   skein-lib, skein-io, skein-tls,     connections, translation, tool schemas,
                        skein-http, both dialects, domain   tool calls' decoding, results' rendering
temper-fake-llm-protocol skein-lib, skein-io, skein-http,   the fake's server: both dialects' server sides
                        both dialects, the fake's domain
```

- **A dialect crate has its own vocabulary:** a request (system blocks,
  tools, messages and their parts, settings), the parts of an answer as
  they complete, a stop reason, usage and an error. Its client side
  measures and writes a request and decodes events; its server side
  decodes a request and writes events. Both are step code over skein's
  JSON tokens and writer.
- **The worker knows its endpoint table.** A charter names
  `Endpoint(u32)`; the worker's protocol layer completes the agent's start
  message with a descriptor for each endpoint the charter names: its
  provider, the host name for TLS and `Host`, its address, the port, the
  path, its account, and its reasoning settings (effort, and Anthropic's
  thinking budget). The agent's protocol layer holds the descriptors for
  its run.
- **The agent's protocol layer** serves the sessions' calls, beside its
  channel to the worker (channel.md) and io's operations for the tools.

## 3. The stack

From the socket up, one state per machine in each connection
(programming-model.md, 4):

```
domain        Complete / Completed, Failed, Cancelled
  ▲  translation: prompt -> request; answer -> completion, tool calls decoded
dialect       the provider's decoder: events -> parts, stop, usage, error
  ▲  tokens
json          one document per event (and the error body)
  ▲  events
sse           lines, fields, an event at each blank line
  ▲  body stream
http client   head, then the body, by length or chunks
  ▲  plaintext stream
tls client    rustls, unbuffered
  ▲  ciphertext stream
io socket
```

| Machine | Its limits, here |
|---|---|
| io socket | intake cap (64 KiB), output cap (the largest request body) |
| TLS | records of 16 KiB each way; ALPN offers `http/1.1` only |
| HTTP client | the response head (16 KiB, headers 64); the request head; an error body (64 KiB, the rest skipped) |
| SSE | a line (`Limits::event_bytes`), an event (`Limits::event_bytes`) |
| JSON | depth (32), a string (`Limits::string_bytes`) |
| dialect | blocks per answer, an opaque block, a tool call's input, the answer's bytes (section 10) |

- **The request is one body,** measured and written once into a box of
  its exact length (`json.md`, 4), and moved down whole. The head says its
  `Content-Length`. Nothing is compressed: requests and responses travel
  as `identity`.
- **The response streams.** The HTTP client hands its body up as a stream;
  the SSE reader demands lines from it; each event's data is one JSON
  document, tokenized by a tokenizer reset per event; the dialect's
  decoder demands tokens. A response that is not a stream of events (an
  error, which is a JSON document) goes to the same tokenizer as one
  document.
- **ChatGPT's large events.** ChatGPT echoes the request's instructions
  and tools, three times per response, in its `response.created`,
  `response.in_progress` and `response.completed` events. The SSE reader
  holds a whole event, so `event_bytes` covers the largest echo: the
  system text and the tool schemas, plus a margin. The decoder skips what
  it does not need, without keeping it (section 13, what skein owes).

## 4. Entities and bindings

- **A call** is the protocol layer's entity for one `Complete`: the
  domain's `owner`, the endpoint, the grant it was given, the deadline
  from its `timeout`, what the prompt offered (to decode tool calls
  against), and the completion as it grows. Calls live in a slab sized
  by the sessions that may call at once: a session has one call in
  flight at a time (agent-domain.md, section 5).
- **A connection** is a socket and its stack, bound to one endpoint. It
  serves one call at a time and is kept, idle, between calls. Connections
  live in a slab of the same size as calls, so a call always finds one:
  an idle one to its endpoint, or a free slot to connect, closing the
  oldest idle connection to another endpoint if none is free.
- **Bindings are echoed tokens** (programming-model.md, 4.2): a call
  holds its connection's id and the domain's `owner`; a connection holds
  io's token for its socket and its call's id. Every terminal goes up
  under the `owner` it came with.
- **A session's name on the wire.** Both providers take a stable name per
  conversation, for their prompt caches and their accounting: Anthropic's
  session header, ChatGPT's `prompt_cache_key` and session headers. The
  protocol layer derives it from the session's `owner`, a token never
  reused for another session, and a salt drawn once at startup from
  `Env`'s randomness, written as a UUID. It needs no table, is the same
  for every turn of a session, and differs between processes. Each call
  also gets a fresh request id.

## 5. Requests

### 5.1 What every request holds

The agent's prompt (`temper_agent_domain::llm::Prompt`) is translated
into the dialect's request:

- **The system text** goes after the provider's identity blocks (section
  9), as its own block.
- **The tools offered** are the families the prompt's `tools` grants, and
  the tools the run serves (`served`): a fixed schema each, written once
  as text in the agent's protocol layer.
  - `Read` (path, skip, lines), `List` (path), `Search` (path, pattern,
    glob): with `inspect`.
  - `Write` (path, content), `Edit` (path, old, new, all): with `modify`.
  - `Shell` (command, timeout in seconds): with `shell`.
  - `Finish` (an outcome: a change's title and body, or a verdict's name,
    body and children) and `SubAgent` (brief, families, the model's name,
    a share of the budget): as the run serves them.

  The names on the wire are the dialect's identity's (section 9), so a
  provider that expects its own client's names gets them; the protocol
  layer maps them back when it decodes.
- **The messages,** in order. A user message's text, its tool results
  first where the provider wants them first. An assistant message's
  parts as they came: text, opaque blocks in their places, tool calls
  with the provider's id and name and the input as the LLM wrote it.
- **A tool call's input is sent back as written,** if it is a JSON
  object. One that is not (cut short at the token limit, or never an
  object) goes back as `{}`, since both providers refuse a request that
  holds one: its result already says why it was not run.
- **A tool's result is rendered as text,** provider-neutral, from
  `Returned`:
  - a read: its lines, each numbered, then a line saying which lines of
    how many were shown, and whether the last was cut;
  - a listing: one entry a line, a directory marked with a trailing `/`,
    then how many more there are;
  - a search: `path:line: text` a hit, then how many more, and whether
    it ran out of time;
  - a command: how it ended, then its output's head, the count of bytes
    dropped, and its tail;
  - a write or an edit: one line saying what changed;
  - every other outcome, and every problem: one short sentence the LLM
    can act on, flagged as an error;
  - not run: one sentence saying the call was not run;
  - the run's answers (`Served`): the run's own words, flagged as an
    error when it says so.

  Rendering is deterministic, and its output is bounded by what the
  outcome holds, which the tools bound.
- **The token limit** is the prompt's `max_tokens`.
- **Measured, then written.** The dialect computes the body's exact
  length from the parts, escapes included, then writes it into a box of
  that length. A body over `Limits::request_bytes` fails the call as
  invalid at the entrance, before a connection is touched.

### 5.2 Anthropic's Messages API

`POST {path}/v1/messages`, with `Accept: application/json` and the
identity's headers (section 9):

- **`system`:** the identity's blocks, then the prompt's system text as a
  text block.
- **`tools`:** `name`, `description`, `input_schema` each.
- **`messages`:** `user` messages carry `text` blocks and `tool_result`
  blocks (`tool_use_id`, the rendered `content`, `is_error`), the results
  first. `assistant` messages carry their opaque blocks (`thinking` with
  its signature, `redacted_thinking` with its data) verbatim, `text`
  blocks, and `tool_use` blocks (`id`, `name`, `input` as written).
  Consecutive tool results and the user's text that follows them are one
  user message.
- **Cache markers,** at most four: the system's last block, the last
  tool, and the last block of each of the last two user messages, each
  `ephemeral` with the identity's time to live. Each turn re-marks the
  tail, so it reads what the turn before wrote.
- **Settings:** `model`, `max_tokens`, `stream: true`, the endpoint's
  `thinking` (enabled with a budget below `max_tokens`, its text omitted),
  and the identity's `metadata` and `context_management`.

### 5.3 ChatGPT's Responses API

`POST {path}` (ChatGPT's Codex route), with `Accept: text/event-stream`
and the identity's headers, including the account's id from the grant:

- **`instructions`:** the prompt's system text.
- **`tools`:** `type: function`, `name`, `description`, `parameters`,
  `strict: false` each; `tool_choice: auto`, `parallel_tool_calls:
  true`.
- **`input`,** as items: a user message is `{type: message, role: user,
  content: [{type: input_text, text}]}`; an assistant text is a message
  item with `output_text` content, and with its item's id and phase when
  an opaque head (section 6.3) came with it; a tool call is a
  `function_call` item (`call_id`, the item's `id`, `name`, `arguments`
  as written); a result is a `function_call_output` (`call_id`, the
  rendered `output`), an error said as such in the text, since the item
  has no flag; an opaque reasoning item goes back verbatim.
- **Settings:** `model`, `stream: true`, `store: false`, `include:
  ["reasoning.encrypted_content"]` (so reasoning comes back, since
  nothing is stored), the endpoint's `reasoning` effort,
  `prompt_cache_key` (the session's name, section 4), and the identity's
  `text` verbosity.
- **No token limit is sent** (section 15): the route's own client sends
  none.

## 6. Responses

### 6.1 Building a completion

The dialect's decoder yields parts as they complete, and the protocol
layer builds the completion from them:

- **Text** goes up as one `Said::Text` per block, UTF-8 checked.
- **An opaque part** goes up as an opaque block in its place (section 13,
  what the domains owe): bytes the session keeps and sends back, never
  reads.
- **A tool call** goes up as `Said::ToolCall`: the provider's id, the
  name and the input as written, and what the input decoded to (6.4).
- **The stop reason and usage** come with the stream's last events. A
  stream that names tool calls among its parts stops as `ToolUse`,
  whatever the provider said, so the session runs them.
- **Too large.** An answer that would pass `Limits::answer_bytes` or
  `Limits::parts` is ended where it stands: the connection is closed, and
  the completion goes up with what was whole and `Stop::MaxTokens`, a tool
  call still open cut as the provider cuts one at its token limit. The
  session treats it as an answer cut short, as it does the provider's.

### 6.2 Anthropic's events

| Event | What the decoder does |
|---|---|
| `message_start` | Usage so far: input, cache reads, cache writes. A second one is malformed. |
| `content_block_start` | Opens block `index`, which must be the next: `text`, `thinking`, `redacted_thinking` or `tool_use` (its id and name). Any other kind is kept whole as an opaque block. |
| `content_block_delta` | Appends to the open block: `text_delta`, `thinking_delta`, `signature_delta`, `input_json_delta` (the input's next fragment, unescaped). A delta for another block, or of the wrong kind, is malformed. |
| `content_block_stop` | Closes the block: a thinking block is written whole, as the provider will want it back; a tool call's input is decoded (6.4). |
| `message_delta` | The stop reason, and the usage, cumulative. |
| `message_stop` | The completion is whole: it goes up. |
| `ping` | Nothing, but it is progress: the idle deadline starts again. |
| `error` | The stream fails with the error's type (section 7). |
| anything else | Skipped, so a new kind of event does not break the client. |

Stop reasons: `end_turn` and `stop_sequence` are `EndTurn`; `tool_use`
is `ToolUse`; `max_tokens` and `model_context_window_exceeded` are
`MaxTokens`; `refusal` is `Refusal`; `pause_turn`, which only server-side
tools cause, and which temper never offers, is malformed.

Usage: `input_tokens` is what was read afresh, `cache_read_input_tokens`
and `cache_creation_input_tokens` the cache's, `output_tokens` the
answer's, thinking included.

### 6.3 ChatGPT's events

| Event | What the decoder does |
|---|---|
| `response.created`, `response.in_progress` | Skipped, all of it: they echo the request. |
| `response.output_item.added` | Opens the item `output_index`: `reasoning`, `message` or `function_call`. |
| deltas (`response.output_text.delta`, `response.function_call_arguments.delta`, the reasoning summary's) | Skipped: each item comes whole with its `done`. They are progress. |
| `response.output_item.done` | The item, whole. A `reasoning` item is written back whole as an opaque block. A `message` item yields an opaque head (its `id` and `phase`) and its text (its `output_text` parts, joined); a `refusal` part makes the stop a refusal. A `function_call` yields a tool call: the id is `call_id` and the item's `id` joined by `|`, then its `name` and `arguments`, decoded (6.4). |
| `response.completed` | Usage and status; the completion is whole. Its `output` is ignored: the items came with their `done`, and with nothing stored it may be empty. |
| `response.incomplete` | The completion is whole, cut short: `max_output_tokens` is `MaxTokens`, `content_filter` is `Refusal`. |
| `response.done` | Read as `response.completed`, whose status it carries. |
| `response.failed`, `error` | The stream fails with the error's code (section 7). |
| anything else | Skipped. |

Stop: `ToolUse` if any `function_call` came, else `EndTurn`, unless cut
or refused. Usage: `input_tokens` less `input_tokens_details.cached_tokens`
is read afresh, the cached tokens are cache reads, there are no cache
writes, and `output_tokens` is the answer's, reasoning included.

### 6.4 Tool calls

A tool call's input is decoded once it is whole, by a second JSON
tokenizer over the input's bytes, into the call the prompt offered under
that name:

- **The name** must be one the prompt offered, in the dialect's spelling;
  otherwise `Problem::UnknownTool`.
- **The input** must be a JSON object (`NotAnObject`). Each field the
  tool needs must be there (`Missing`) with its type (`WrongType`) and a
  value the tool takes (`BadValue`: a path with an empty name or a NUL,
  a number out of range, a duplicate field). Fields the tool does not
  know are skipped: models add them.
- **Paths** are split at their slashes into a `tools::Path` (path.rs of
  the tools child domain), which the domain resolves.
- **Numbers** are decoded from their text as the integers each field
  takes, checked; a timeout is whole seconds.
- **The run's tools** decode into `run::Ask`: `Finish` into a declared
  outcome, `SubAgent` into its brief, families, model name and share.
- **Too large:** an input over `Limits::input_bytes` is `TooLarge`, its
  bytes not kept.

The input is kept as written beside what it decoded to, as the domain
requires: it goes back as written (5.1).

## 7. Failures

The session's failures (`temper_agent_domain_session::llm::Failure`) plus
one for an exhausted account (section 13). Rate-limit times are durations,
as every time crossing a host is.

| What came | Failure |
|---|---|
| The connection could not be made, TLS failed, or the request could not be sent whole | `Unavailable` |
| No response head by the head deadline, no event within the idle deadline, or the call's own deadline passed | `TimedOut` |
| No grant usable for the call (credentials.md, section 7) | `Unauthorized`, without reaching the provider |
| 401 | `Unauthorized` |
| 403 | `Invalid`: the account may not use this model or route, which no refresh mends |
| 429 that says the account is spent: ChatGPT's `usage_limit_reached`, or Anthropic's unified limit status `rejected` | `Exhausted`, `retry_after` until its reset |
| 429 otherwise | `RateLimited`, `retry_after` from `retry-after`, else the provider's reset, else zero |
| 529, 503, or the error type `overloaded_error` | `Overloaded` |
| 500, 502, 504, or the error type `api_error` / `server_error` | `Unavailable` |
| 400 or 413 saying the prompt is too long (`prompt is too long`, `context_length_exceeded`, `request_too_large`) | `ContextTooLong` |
| 400, 404, 413, 422 otherwise | `Invalid` |
| A 2xx that is not an event stream | `Unavailable` |
| A stream that ends, or breaks, before its last event (`message_stop`, `response.completed`, `.incomplete`, `.done`) | `Unavailable` |
| An event that is not JSON, or breaks the dialect's order | `Unavailable`, and the connection is closed |
| An `error` event or a failed response mid-stream | by its type, as above; unknown types are `Unavailable` |

- **Resets** are read from `retry-after` (seconds), from Anthropic's
  `anthropic-ratelimit-unified-reset` and ChatGPT's `resets_in_seconds`
  or `resets_at` (wall times, less `env.wall`, floored at zero).
- **What goes up besides** is the domain's: on `Unauthorized`, the
  agent's top level sends `Rejected` for the grant it gave, once per
  generation; on `Exhausted`, it sends `Exhausted` with the account and
  `retry_after` (credentials.md, section 7).
- **Malformed and cut streams** are transient: a broken stream is more
  often a dropped connection than a broken provider, and the session's
  retries are bounded.
- **The error's message** is kept, cut to `Limits::detail_bytes`, for the
  agent's log; it never decides anything.

## 8. Connections

Each connection is a state machine; whether each deadline runs follows
from the state alone, and one function arms them (programming-model.md,
5.4):

| State | Holds | Deadline | Next |
|---|---|---|---|
| Connecting | the call | connect | Handshaking; Closing, the call `Unavailable` |
| Handshaking | the call, TLS | handshake | Sending; Closing, the call `Unavailable` |
| Sending | the call, the body queued | head | Head once the body is sent; a response head first ends the upload |
| Head | the call | head | Streaming on a 2xx event stream; Erring otherwise |
| Streaming | the call, the stack's state | idle between events, and the call's | Draining when the answer is whole; Closing on a failure |
| Erring | the call, the error body | idle | Closing once the body is read, the call failed by status and type |
| Draining | nothing for the domain | idle | Idle once the body ends and both sides keep the connection; Closing otherwise |
| Idle | the endpoint | `Limits::keep_idle` | Sending with a new call; Closing |
| Closing | io's socket, settling | io's close | Free |

- **The call's terminal goes up once,** as soon as it is known: the
  completion when the stream says it is whole, before the body is
  drained; a failure when it happens. A connection that drains past its
  deadline is closed, costing nothing to the call.
- **Cancel** aborts the connection in any state that holds the call. The
  call's `Cancelled` goes up once io has settled the socket; if its
  completion or failure went up first, the cancel finds nothing, and the
  domain has the outcome that won.
- **The idle deadline** starts again with every event, `ping`s and the
  deltas the decoder skips included, so a model thinking for minutes is
  not cut while the provider keeps the stream alive. The call's own
  deadline is the session's `timeout`, from the request's start.
- **Reuse.** A connection is kept when both sides allow it, and closed
  after `keep_idle`, shorter than the providers' own idle cut, so a
  request rarely meets a connection the server has just closed. One that
  does fails as `Unavailable`, and the session tries again.
- **No retries.** A failed call is never sent again by the protocol layer
  (protocol.md, section 3).

## 9. Identity

Each dialect has one module of data for what its provider's subscription
route expects of its own client, taken from the latest transcripts of
that client (section 12):

- **Anthropic:** the `anthropic-version` and `anthropic-beta` values (the
  OAuth and client betas, interleaved thinking, the cache time to live,
  context management), the user agent and the client's other headers,
  the system blocks that must lead (a billing line and the client's
  identity sentence), the tools' names, the cache time to live, and the
  shape of `metadata`. Requests without them are refused.
- **ChatGPT:** the `originator` and user agent, the session and request
  headers, `chatgpt-account-id` (the grant's), and the text verbosity.
- **One place, one owner.** The identity is data, versioned with the
  transcripts it came from. When a provider changes what it expects,
  transcripts are captured again from its client, the module follows, and
  the dialect's tests show what changed (section 12).

## 10. Limits and the worst case

The LLM client's part of the agent's protocol layer has these limits
(`Limits` of `temper-agent-protocol`, beside its channel's):

| Limit | What it bounds |
|---|---|
| `calls` | calls and connections at once: the agent's session slots |
| `request_bytes` | a request's body |
| `head_bytes`, `headers` | a response head |
| `event_bytes` | an event, and a line within it: ChatGPT's echo of the instructions and tools, with a margin |
| `string_bytes` | a JSON string kept: the largest of an opaque block, a tool call's arguments and a text delta |
| `answer_bytes` | the completion's bytes: its text, inputs and opaque blocks |
| `parts` | the completion's blocks |
| `input_bytes` | a tool call's input |
| `opaque_bytes` | an opaque block |
| `error_bytes`, `detail_bytes` | an error body read, and what is kept of its message |
| `connect`, `handshake`, `head`, `idle`, `keep_idle` | the deadlines of section 8 |

- **The request body is the large one.** A prompt is sent whole every
  turn, so `request_bytes` is the session's byte limit, times six for the
  worst JSON escaping (a control byte becomes `\u00XX`), plus the identity,
  the system text and the tool schemas. In practice text escapes to about
  its own length; the worst case is still what is counted.
- **Per call:** the request body until it is sent; the socket's intake
  and output; TLS's records; the head; one event; the tokenizer's stack
  and one string; the completion as it grows, under `answer_bytes`; the
  error body under `error_bytes`.
- **The worst case** is `calls` times that, plus the descriptors. It is
  computed at startup and must fit (programming-model.md, 6.3); the
  agent's whole worst case adds it.
- **A completion is copied once,** from the decoder's parts into the
  domain's records: the decoder's buffers are given up as the parts go.

## 11. The fake provider's protocol layer

`testing/temper-fake-llm-domain` grows a protocol layer of its own,
`temper-fake-llm-protocol`, so that protocol worlds, the simulator and the
real loop meet it through bytes (testing.md, 4.1):

- **An HTTP server** on its listener, one request at a time per
  connection, routing by path: the Messages path to the Anthropic
  dialect, the Codex path to ChatGPT's.
- **Each dialect's server side** decodes the request, which the fake's
  protocol layer translates into its own vocabulary (`api::Query`), and
  writes the fake's answer as the provider's events: Anthropic's message, block and
  ping events; ChatGPT's created and in-progress events echoing the
  request's instructions and tools as the real one does (so the client's
  event limit is exercised), its items, and completion.
- **It checks its client as the providers do:**
  - the bearer is a token the fake's OAuth server issued and has not
    expired, else 401 (credentials.md, section 10);
  - ChatGPT's account id matches the token's;
  - Anthropic's identity blocks and headers are there, else the refusal
    the provider gives;
  - tool calls and their results pair up (as its domain already checks);
  - an assistant message carries back the opaque blocks the fake gave it,
    unchanged, as both providers require for reasoning to carry on: the
    fake's opaque blocks carry a digest of themselves, so a changed or
    missing one is refused.
- **Its failures speak the providers' shapes:** each error with its real
  status, body and headers, the exhaustion and rate-limit resets
  included.
- **Its stream faults:** a stream cut mid-event or between events, a
  malformed event, a chunk size that lies, a trickle of one byte at a
  time, a stall with pings only, and a stall with nothing.

## 12. Testing

- **Transcripts** (testing-strategy.md, 4.1): requests and responses
  captured from the providers' own clients against the real providers
  (Anthropic's command-line client, ChatGPT's Codex) for a single text,
  thinking, a tool call, parallel tool calls and a tool's result; the raw
  bytes of each response as they came, head and chunked body, and the
  request as sent, with secrets redacted when captured. They are kept
  beside the dialect's tests with what each must decode to.
- **Format, not content.** What must match is the format: header names
  and value formats, the framing, JSON keys, types and nesting, event
  names and order. Ids, times, token counts, signatures and the cut of
  text deltas vary between captures and are masked before comparing.
- **Machine worlds** for each dialect's decoder, from a seed: the
  transcripts cut at random, events generated valid and mutated, and
  hostile ones (a block that never stops, an index out of order, an
  echo past the event limit, a string past its limit, nesting too deep).
  One fuzz target per dialect, fed bytes under every demand.
- **Requests against the client's grammar.** temper's requests, reduced to
  their structure (keys, types, nesting, event names), must use only
  structure the provider's own client uses in its transcripts: temper
  invents no wire structure the provider has not seen.
- **Writer against reader.** Each dialect's server side writes what its
  client side reads back the same, and the other way round.
- **Protocol worlds** (testing.md, 2.2): the agent's protocol layer
  against the fake provider's, bytes between them cut and joined at
  random, over both dialects, with the fake's faults. They check that
  what the session asked for is what the fake was asked, that what the
  fake answered is what the session received, and that a token never
  crosses a domain boundary (credentials.md, section 10).
- **Live captures,** by hand, never in the suites: the same scenarios
  through the providers' clients, to refresh the transcripts, and through
  temper's own client, to check its requests are admitted.

## 13. What others owe

**The domains:**

- **An opaque block,** in the session's vocabulary (`Block` and the
  agent's `Said` and `Block`): the provider's bytes, kept in their place
  in an assistant message, counted against the session's bytes, and sent
  back verbatim. The fake's vocabulary gains its own.
- **An exhausted account,** a failure of its own, which ends the session
  and the run as transient (credentials.md, section 9).
- **`Unauthorized` becomes transient,** as credentials.md has it.
- **Each `Complete` names its grant** (credentials.md, section 7).
- **The endpoint's reasoning settings,** in its descriptor for now; a
  charter that names the effort for each LLM is section 15's.

**skein:**

- **The JSON tokenizer skips a value** on demand: a string, an array or
  an object, however large, without keeping it, its nesting still
  bounded. ChatGPT's echoes are mostly such values.
- **The SSE reader** holds a whole event under its maximum (http.md,
  section 4), which the echoes size. A reader that streams an event's
  data to the tokenizer would cut that to a line; whether it is worth it
  is skein's call.
- **The TLS client** takes a host name for SNI and a root store, and
  offers `http/1.1` by ALPN.

## 14. Not built yet

- **Recording real traffic** through temper itself: a mode of the agent's
  protocol layer that keeps each exchange it has with a provider (the
  request as sent and the response's raw bytes, the bearer redacted) as a
  transcript, so a deployment can capture what its providers actually
  say, and the transcripts can be refreshed without another client. The
  transcript format above is the one it would write.
- **More providers:** OpenAI's API with keys, and others behind
  OpenAI's chat completions, each a dialect.
- **MCP servers' tools,** offered beside the session's own (protocol.md,
  section 8).

## 15. Open questions

- **ChatGPT's token limit.** The Codex route's own client sends no
  `max_output_tokens`, and whether the route takes one is unknown. Without
  it, the session's output budget cannot cap an answer, only stop the
  next one; if the route takes it, it is sent.
- **Anthropic's identity.** Whether the subscription route checks the
  tools' names as well as the headers and the leading system blocks, and
  how often its client's shape changes.
- **The address of an endpoint.** io connects to addresses, and names are
  not resolved while a service runs (io.md, section 4), so an endpoint's
  address is fixed for the worker's life; workers live long while
  providers' addresses move behind their CDNs. A worker that loses its endpoints
  fails every call as `Unavailable` until it restarts. skein's DNS client
  would mend it.
- **HTTP/1.1 against both providers.** skein speaks HTTP/1.1 only; both
  providers serve it today, behind their CDNs, and a provider that
  required HTTP/2 would need skein to grow it.
- **The request as one body.** A long transcript is sent whole, every
  turn, as one box that worst-case escaping sizes at six times the
  session's bytes. Writing the body piece by piece, as the socket has
  room, would cut that to a chunk, at the cost of holding the prompt
  until it is sent.
- **Effort per LLM.** Effort and thinking are the endpoint's today; a run
  may want a cheap decision and a deep change on one model, which needs
  the charter to say so.
- **Reasoning text for transcripts.** Anthropic's thinking is asked for
  with its text omitted, and ChatGPT's summaries are skipped, so a run's
  transcript holds what the model did, not what it thought. Whether
  transcripts should carry a summary is for the engine's views.
- **403s.** They are classified as invalid, since a refresh does not mend
  them; a provider that answers 403 for a token it has just revoked would
  want `Unauthorized` instead.
