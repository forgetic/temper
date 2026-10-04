This crate owns the Messages JSON dialect; it has no domain or I/O dependency.
`Request`, `Block`, `Tool`, `Event` and `Part` are its own typed vocabulary.
Requests and fake-server events are measured before allocating their exact
encoded body. `Json` validates bounded values through skein-json; `Collector`
accepts an incremental tokenizer stream under separate token and byte caps.

A client tokenizes one SSE event, calls `decode_event`, then
`StreamDecoder::event(event, limits, env.wall, output)`, reserving `MAX_OUT`
slots first. Completed blocks transfer ownership as `Output::Part`; exactly
one `Completed` or `Failed` ends the decoder. Call `end` if the body ends.
An input above `input_bytes` produces a tool call with empty input and
`too_large: true`; exceeding the answer/part cap produces `MaxTokens`.
Opaque blocks are compact JSON with exact string contents and preserved
unknown fields, ready to parse as `Json` for the next request.

The system blocks are supplied in their complete wire order. The connection
owner adds the deployment's verified billing line, `identity::SYSTEM` and
its own system text, plus verified metadata/context management. Identity
headers in `identity.rs` are historical tongs evidence; their provenance
and the real provider archives are documented in `tests/fixtures/README.md`.
A fresh Claude Code capture remains necessary to verify current admission.

`worst_case` includes fixed-capacity token/part tables, tokenizer/writer
storage and transient ownership during request/event/completion decoding.
Text accumulation currently uses bounded `List<u8>`. The growable buffer
required by temper's performance design is pending upstream and must replace
these temporary buffers before the assembled protocol layer is complete.
