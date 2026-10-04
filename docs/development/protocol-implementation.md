# Protocol implementation

Work branch: `protocol-layer`, starting from `9eaeda3`.
The design authority is `docs/design/protocol.md`, with the detailed
`channel.md`, `credentials.md`, `llm.md` and `forge.md` taking precedence
over their overview where the overview has not caught up.

## Paused handoff: 2026-10-04, 12:58 UTC

The user requested a graceful pause after disconnecting the network. All
three implementation agents are idle; no task build, conformance runner
or scratch Forgejo server remains active. Resume only at the user's request.

The verified local main is `8a13dde`; the shared `protocol-layer` branch
has the next increment saved as uncommitted changes and untracked files.
Nothing is staged. The proposed isolated checkpoint and full workspace
gate have not run, and no new merge was made during this pause.

Ready for the next checkpoint, with scoped checks passed:

- Engine credential/OAuth owner, keeper boundary and worlds: 19 focused
  tests, a 12-seed socket sweep, occupied memory and strict lint.
- Agent HTTP/SSE exchange, rotating fake OAuth issuer and fake provider:
  five exchange tests, four paired provider tests, three occupied-memory
  tests and strict lint.
- Forgejo documents, signed webhooks and live v15 fixtures; fake HTTP
  translation and bounded comment-row streaming: observed fixture and
  occupied-memory tests, four fake HTTP tests and strict lint.
- Forge costs, own-echo filtering and coalesced wiki refreshes: root/forge
  unit, focused, memory and fuzzy checks, plus strict lint. The calm-world
  closed-write trace is `/tmp/temper-forge-closed-evidence.txt`.

Keep these later drafts out of that checkpoint until tested:

- `crates/temper-agent-protocol/src/client.rs` compiles and passes scoped
  lint, but needs actual Io/simulator lifecycle and memory worlds. Its
  `pub mod client` export remains in the working tree; omit that export
  from a checkpoint that excludes the file.
- The fake forge's `acceptor.rs` is unexported. Socket acceptance,
  outbound signed hooks and the engine webhook receiver are next.
- Engine `forge_cursor.rs` and `forge_blocks.rs` are unexported and
  unchecked. `tests/engine/protocol/tests/forge_cursor.rs.draft` is
  deliberately excluded from automatic test discovery.
- Item comment times, explicit cursor bounds, keyed-creation label
  reconciliation, typed record blocks and bounded Forge call plans are
  still to implement and validate.

The lockfile pins cached skein `78a07ec`; the local skein checkout remains
read only. Use offline Cargo on resume. TLS integration still waits for
the stream-contract decision described below; file/process facilities
and streaming JSON prerequisites remain pending.

## Scope and sequence

Implement the three currently designed boundaries, in the order of
`protocol.md`, section 10. People, git process codecs, MCP and the store
remain its explicitly later work. The agent's long-lived channel policy
and snapshot contents also remain separately designed domain features
(`agent-domain.md`, section 10; `protocol.md`, section 10.1). Their wire
messages still belong to the channel schema.

1. **Channel:** the domain-independent `temper-channel` wire and payload
   schemas, frame machine, sized limits, golden frames and machine world;
   domain prerequisites; then engine, worker and agent protocol adapters
   and connection lifecycles, with channel protocol worlds. Replace the
   system worlds' translation code with the production translation modules
   once those are ready.
2. **Credentials and LLM:** accounts child domain and its world; grant
   names through domains, values only in protocol tables; OAuth documents
   and durable rotation; Anthropic and ChatGPT dialects, both sides;
   agent's HTTP/SSE/JSON stack and provider-neutral tool translation; fake
   provider protocol and protocol world.
3. **Forge:** domain prerequisites for costs, comment times and hints;
   Forgejo documents and webhooks, both sides; production engine block
   codecs, bounded call plans and caches; fake forge protocol; cost worlds
   and real Forgejo conformance tooling.

## Current ownership

The coordinator owns workspace registration, the lockfile, this record,
review, integration and the full merge gate. Implementation agents own
disjoint areas until explicitly reassigned:

| Agent | Current assignment |
|---|---|
| `channel` | Forgejo documents, webhooks, fake forge protocol and isolated conformance tooling; retains channel and worker ownership |
| `llm_dialects` | Agent HTTP/SSE/JSON client, fake provider service and agent protocol worlds; retains dialect/OAuth codecs |
| `domain_prerequisites` | Engine credential tables, OAuth execution and keeper boundary; then forge domain prerequisites and engine call plans; retains engine/agent domain ownership |

All work shares the feature branch. Agents do not independently commit or
merge, or change global manifests. The coordinator reviews completed
changes and runs the checks with compilation idle before a merge.

## Upstream availability

Skein is read only at `/home/free/src/rust/skein`. Its main at the start
of this work is `d873f985afb1b8224942dba4bd280222bc7cd419`, imported into
Cargo's existing bare git cache from that local checkout. Use Cargo's
`--offline` option while Forgejo is unavailable. No checkout override or
change to skein is required. Import later main commits from the same
checkout when the user reports them ready.

Local main subsequently advanced to
`4586ba9f729f63d9381e40cbb8e845d62f1fe04a`, with an HTTP server,
SSE writer and TLS client. The channel checkpoint still uses `d873f985`.
The HTTP-server merge `78a07ec` is a main ancestor whose stream contract
is unchanged; it can unlock the server work while the following TLS
contract change is reconciled.

The next increment pins
`78a07ec16d935e47ed921083f127798a8f3a3f22`, imported from that same local
checkout into Cargo's git cache. This enables the fake services and
webhook HTTP stack. Client and server protocol worlds use plain streams;
production off-host use still requires the TLS boundary.

That TLS merge restricts each `Room` grant to one `Send`. The current
channel's documented whole-cap fallback reuses byte credit across several
frame sends while a read is outstanding (`channel.md`, 10.1). Directly
stacking it on the new TLS client violates the grant contract. Spending
the grant after one frame, or batching that frame with other queued ones,
still strands later asynchronous output behind an outstanding read. An
independent read/room demand implemented through io and TLS resolves this;
the existing read withdrawal cannot, because it gives up further reading.
Channel TLS integration awaits that upstream contract or design direction.

| Facility | At the initial skein main | Consequence |
|---|---|---|
| `skein-lib`, sockets, HTTP client, SSE reader, JSON tokenizer/writer | Available | Channel, document codecs, plaintext socket/client stacks can proceed |
| Independent room demand during a read | Not available | Implement the channel's explicit bounded room grant (`channel.md`, 10.1) |
| Partial output-drain progress | No public io event | Whole-cap `Room` proves a completed drain; finer stall progress needs upstream observability |
| io files, process pipes and spawning | Not available | In-memory channel stacks proceed; actual agent pipes/root opening wait |
| TLS client and server | Not available | Deterministic plaintext worlds proceed; off-host connections wait |
| HTTP server and SSE writer | Not available | Server document codecs proceed; fake services/webhook HTTP wiring wait |
| JSON value skipping and streamed strings | Not available | Bounded codecs proceed; history-independent streaming forge decoding waits |
| Capped growable bytes and `ByteRing` (`performance.md`, 3) | Not available | A bounded `List<u8>` can temporarily join provider deltas; replace it with upstream growable storage; process tails wait for the ring |
| HTTP request-byte accounting | `Error::Closed` proves no request went down; other faults are conservative | This distinction can proceed; exact kernel byte accounting remains upstream work |

These are dependencies, not reasons to stop independent implementation.
Missing skein functionality is reported to the user rather than patched
into the local skein checkout.

For the manual forge conformance run, an isolated binary is prepared at
`/tmp/temper-forgejo-15/forgejo-15.0.0-linux-amd64`. It reports
`15.0.0+gitea-1.22.0`, matching `forge.md`, section 13. It came from the
[official release](https://code.forgejo.org/forgejo/forgejo/releases/tag/v15.0.0),
passed the published SHA-256 check
(`3919f10a7845f3b71bacc2c7a3bfa2cd71aed58a0b8be6ab5e95f2e150b4ded7`)
and the release signature under Forgejo's published key
`EB114F5E6C0DC2BCDD183550A4B61A2DC5923710`. The conformance tool must use
its own scratch data and loopback listener.

The channel codec and machine pass their focused, bounded-memory and
randomized tests. The maximum agent start exercises repository, endpoint
and grant arrays as well as their byte fields. Both provider dialects pass
focused tests and strict Clippy, including seven masked real-provider
captures from the existing tongs recorder. Their captures establish
response grammar; their historical client headers are identified as such.

The accounts child and its first worlds pass refresh, rejection,
rate-limit, revocation, unsaved-save retry and exhaustion scenarios.
Engine, agent and worker domain prerequisites are integrated. The engine's
production payload and name translation replaces its system world's
copies, with opaque snapshots preserved, malformed outcomes retaining
landed work, and invalid relays answered without losing the link. OAuth
documents and saved records include starting from a refresh token alone.
The work branch now adds the authenticated engine listener, worker link
and agent stream bridge, and the agent channel owner. Socket replacements
wait for actual closure; credential values remain in bounded protocol
tables; fact traffic leaves owed output capacity. Agent tool grammar,
result rendering, provider documents and current-run charter/outcome
translation replace the corresponding system-world copies. The integrated
byte, lifecycle, memory and replay checkpoint is verified on local main.

## Verified milestone

`a77df99` is on local main after all four integrated checks passed:
formatting, workspace strict Clippy, the focused suite (1,662 tests in
10.546 seconds), and the fuzzy suite (23 tests in 31.688 seconds). The
15-second and 60-second budgets remain unchanged. The complete protocol
goal is still active; this commit supplies schemas, document codecs,
domain prerequisites and pure engine translation.

`3ccdd2d` is on local main after formatting, workspace strict Clippy,
the focused suite (1,722 tests in 9.742 seconds), and the fuzzy suite
(25 tests in 26.518 seconds, one ignored finding replay) passed on that
exact tip. It adds engine and worker socket owners, the agent pipe
boundary, credential tables, and agent tool/document translation.
The existing test budgets remain unchanged. HTTP execution, durable
OAuth rotation and forge implementation are the next increment.

## Integration decisions to hold in review

- Tokens and provider account-id bytes never enter domain records. Grant names remain
  `(account, generation)` and generation survives restart.
- OAuth request, response, claims and saved-record types omit `Debug`;
  typed failure diagnostics carry no secret values.
- A failed durable save retries keeping the pending token, not rotating
  OAuth again. The old granted generation may stay usable while valid;
  the candidate is not handed out before its save succeeds. This follows
  the explicit durability ordering in `credentials.md`, section 6; its
  "usable but at risk" wording needs reconciliation if it was intended
  to expose the unsaved candidate instead.
- A save timeout remains `Unsaved` after keeper cancellation settles.
  Only an explicit account cancellation yields `Cancelled`. A successful
  save that wins that race installs the durable generation before the
  cancellation terminal. Token validity is anchored to response completion
  and does not grow while waiting for persistence.
- The OAuth owner exposes an atomic keeper effect. The future file owner
  must replace the versioned record and sync both the file and directory
  before reporting `Kept`; pure and socket worlds do not establish actual
  filesystem durability.
- Component-local operation tokens need a bounded whole-engine io router.
  It must allocate global names, route them to tagged local owners and
  retain every binding through actual io `Closed`, including terminal
  operations whose old sockets are still settling.
- Forge timestamps on the channel are source metadata. They are not
  local monotonic deadlines; validity and retry times cross as durations.
- Fixture provenance is explicit. Generated examples must not be called
  real captures; existing masked peer captures can be reused only after
  inspecting their provenance.
- Protocol translation owns mechanics and formats. Retry, token budget
  allocation and account admission decisions stay in domains.
- Forge calls reserve one HTTP request at admission and settle their
  actual cost at the terminal, including failures. Refunds belong only
  to the reservation's window; extra costs also charge the priority share.
  Concurrent multi-step calls can overshoot before their costs arrive;
  this terminal accounting does not enforce a hard per-request rate cap.
  Startup HTTP costs must be reported exactly once as well.
- Wiki hints refresh only currently held notes scopes for their
  repository, with deployment notes restricted to the home repository.
  A bounded set coalesces hints and dispatches one scope per turn. The
  configured engine's own echoes are dropped before hint routing.
- Real Forgejo v15 omits unknown labels on successful create/add writes.
  The protocol must inspect returned labels; keyed creation recovery
  needs a repeat-safe label reconciliation phase before returning Created.
  The retained live fixtures and tagged-source examples identify their
  separate provenance.
- Every worker-origin attempt input is checked against its current hosting
  channel in the fleet, including relays, facts, bounces and credential
  notices. Retired and foreign channels cannot act on a live attempt.
  A relay accepted before a disconnect still outlives that connection.
- A worker reconnect is accepted only after its previous socket's actual
  `Closed` event and the old channel's `Lost` notice. One pending
  replacement per worker remains under its original handshake deadline;
  a newer authenticated reconnect supersedes it without freeing any io
  binding early.
- Socket startup checks actual io read and room caps and enough Send
  records for the full byte grant. Category-based pending-message counts
  alone do not bound tiny ping records already handed below the machine.
- Initial token validity is anchored to engine construction time, not
  delayed account registration. Counter exhaustion refuses startup or
  revokes an account when no further generation can be issued, without reuse
  or a panic. Regained account usability queues the current grant for
  scoped live attempts as well as waking waiting starts.
- An agent start carries the worker's full endpoint descriptor table,
  as the explicit rule in `channel.md`, section 6.1 says, so the worker
  does not decode the opaque charter. `llm.md`, section 2 still describes
  a subset and needs reconciliation. Descriptors retain their endpoint
  indexes; the agent validates model references at entrance. Only LLM
  grant values travel to the agent; static git values stay in the worker.
- Inbound events and bounces carry an opaque event name. Acknowledgement
  by name is not a count, so sequence gaps are allowed. The detailed link
  schema lacks a worker-to-engine acknowledgement despite section 14
  asking for one; the separately designed long-lived agent half remains
  deferred while clarification is pending. Named bounces alone do not
  solve recovery of a delivered event lost across reconnection.
- Removing the worker world's private payload prefixes exposed a restart
  finding: one hosted attempt can receive the same inbound name with
  different bodies after the engine restarts. The host forwards both;
  the agent child's acknowledgement of an already acknowledged name can
  leave later ledger entries unacknowledged. The world retains the replay
  in its findings. The current adapters do not claim that the deferred
  long-lived acknowledgement and restart namespace policy is complete.
- The agent Finish schema lacks refusal and spend fields, while section
  14 asks for both and the domain documents call out their loss. The
  implementation retains the specified schema pending design direction;
  translation must explicitly expose this limitation instead of claiming
  those values survive the wire.
- `LongDone` uses owed output capacity. The future process/check owner
  must drive it from the actual check terminal; the domain's
  `CheckFinished` fact is best effort and can be dropped before it reaches
  protocol translation. A fact-only projection cannot establish that
  watchdog integration is complete.
- The worker domain can reject an assignment because its grant names are
  invalid, but the v1 `Invalid` wire enumeration has no Grants case. The
  adapter reports a typed unsupported conversion while a wire extension
  is awaiting design direction; it does not label this as a charter or
  repository error.

## Completion gate

Before main moves, run all four checks on the integrated branch tip:

```sh
cargo fmt --check
cargo clippy --offline --workspace --all-targets -- -D warnings
cargo nextest run --offline --workspace
cargo nextest run --offline --workspace --profile fuzzy
```

The two suites retain the 15 second and 60 second budgets in
`docs/development/workflow.md`. A check for a partial assignment does not
replace the integrated gate. Work still waiting for upstream is recorded
explicitly rather than marked complete.
