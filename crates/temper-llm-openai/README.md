This crate owns the ChatGPT Responses JSON dialect, with no domain or I/O
dependency. `Request`, `Input`, `Tool`, `Event`, `Item` and `Part` are its own
vocabulary. Requests and fake-server events are measured before allocating
an exact body. `Json` and `Collector` use skein-json and explicit depth,
string, document-byte and token-count caps.

A client tokenizes each SSE event, calls `decode_event`, then
`StreamDecoder::event(event, limits, env.wall, output)` with `MAX_OUT` slots
reserved. Items may finish out of order. The decoder holds their prepared
parts under answer/part caps and emits one ordered item at a time. While
`has_ready()` is true, the owner places it on its ready list and calls
`ready(output)` before reading another event. A completion waits for pending
parts; `end` fails a stream whose terminal never arrived. No `created` event
is required for a terminal-only response.

Reasoning is retained whole as compact opaque JSON, preserving exact string
contents. A message produces an opaque `{id, phase}` head followed by its
text. The agent combines that head with the next assistant message when
constructing `Input::Message`. Tool ids join `call_id` and item id with `|`;
an oversized input is discarded and marked `too_large`. Explicit incomplete
or refusal stops take precedence over tool use. No output-token cap is sent
to the Codex route, as temper's design requires.

Fixed identity headers are historical tongs evidence, documented with the
redacted real-provider archives in `tests/fixtures/README.md`. A fresh
provider-client capture remains necessary to verify current admission.
Whole-event token collection and bounded `List<u8>` accumulation are safe
interim storage. Skein's skipping tokenizer and capped growable byte buffer
must replace them before satisfying the assembled layer's performance design.
