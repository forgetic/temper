# The protocol layer

Provisional, 2026-10-03. How temper's components turn bytes into domain
entities and back: the boundaries each one has, what their protocol
layers share, temper's own wire protocol, and the rules for the foreign
ones. The mechanics are those of skein's
`docs/foundation/programming-model.md` (sections 4, 7 and 8); the
entities this layer carries are those of `agent-domain.md`,
`worker-domain.md` and `engine-domain.md`, whose "Below the domain"
sections say what each domain is owed. skein's machines (TLS, HTTP,
server-sent events, JSON) are designed in skein's `docs/design`. This
document is the overview: each boundary is designed in depth before it is
built, in its section here until it outgrows it. What is open is listed
in section 11.

## 1. In one page

- **Translation, and nothing else.** The protocol layer turns bytes into
  the domains' entities and back, and decides nothing about the domain
  (programming-model.md, section 4). The domain decides deadlines,
  retries and what a failure means; the protocol layer runs the call, its
  timer and its one attempt.
- **One protocol crate per component:** the engine's, the worker's and the
  agent's, and one for each fake that grows into a service. They stack
  skein's machines and a few crates of temper's own.
- **Two kinds of boundary.** temper's own channels, between the engine and
  a worker and between a worker and an agent, share one sized wire format
  of temper's design. Foreign protocols (LLM providers, the forge, people's
  browsers, git) are stacks of skein's machines with temper's decoders on
  top.
- **The channels are thin.** The domains already own what makes them
  reliable: the hello, fenced attempts, acknowledged answers, redialling.
  The wire adds framing, a version, authentication and a liveness
  deadline.
- **Payloads are typed at their ends.** What the worker passes through as
  bytes (a charter, an outcome, a relayed call and its answer, a fact) is
  encoded by one end's protocol layer and decoded by the other's, from one
  schema.
- **The engine holds every credential:** forge tokens, workers' secrets and
  LLM accounts, which it alone refreshes. LLM access tokens go down to the
  agents that use them, through their workers. A domain names a
  credential; only protocol layers carry its value.
- **The forge's load follows its changes.** The calls a pass makes grow
  with what changed on the forge, never with its history, its labels or
  the items it holds, and the worlds count them.
- **Tested at every tier:** machines and decoders against transcripts of
  real peers; protocol worlds joining each component's stack to its
  peers' fakes, which grow protocol layers of their own; and the facts the
  forge client relies on checked against a real Forgejo.

## 2. The map

```
starts   with           transport               wire                         kind
worker   engine         TCP, TLS off the host   temper's channel             own
worker   agent          the agent's pipes       temper's channel             own
agent    LLM provider   TCP, TLS                HTTP, SSE, JSON              client
engine   OAuth server   TCP, TLS                HTTP, JSON                   client
engine   forge          TCP, TLS                HTTP, JSON                   client
forge    engine         TCP, TLS off the host   HTTP, JSON: webhooks         server
people   engine         TCP, TLS                HTTP, SSE, JSON: the web     server
worker   git            a process's pipes       arguments in, text out       process
agent    MCP server     a process's pipes       JSON-RPC, a line each        process
engine   store          files                   temper's records             own
```

Formats also travel inside other payloads: the engine's record, the
outcomes it posts and notes' pages inside forge comments and wiki pages
(engine-domain.md, sections 4 and 10); charters, outcomes and relayed
calls inside channel messages (section 4).

## 3. What every protocol layer shares

- **Crates:**

  ```
  crate                    depends on                              holds
  temper-channel           skein-lib                               temper's channel: frames and payload schemas
  temper-llm-anthropic     skein-lib, skein-json                   a provider's documents, both sides
  temper-llm-openai        skein-lib, skein-json                   the same, for ChatGPT
  temper-oauth             skein-lib, skein-json                   token refresh, both sides
  temper-forge-forgejo     skein-lib, skein-json                   Forgejo's documents and webhooks, both sides
  temper-<c>-protocol      skein-lib, skein-io, the above, domain  a component's connections and translation
  ```

  A crate that holds a foreign protocol's documents has both sides, so
  the fake facing temper uses the side temper does not. Two sides written
  together can agree on one misreading, so each is also fed transcripts of
  the real peer (testing-strategy.md, 4.1).
- **Connections are the protocol layer's own entities**
  (programming-model.md, 4.1): a socket or a pipe below, a connection
  here, a channel, a run or a call above. A connection holds its stack, a
  state per machine, and its deadlines, and binds to the entity above by
  echoed tokens (4.2).
- **One exchange in flight per connection,** or one stream, with an output
  cap and a room check before the next is parsed (section 7): a peer that
  stops reading stops being read from.
- **Refusals at the entrance.** A connection with no slot is rejected; a
  call with no room is refused before anything is set aside for it.
- **Deadlines.** The domain gives each call its deadline; each connection
  arms its own handshake, idle and progress deadlines from its limits.
  Machines keep none (programming-model.md, section 9).
- **No retries below the domain.** A call goes out once and ends once,
  with the failure the domain's vocabulary defines: for the forge,
  unavailable only when the call provably never left, and timed out
  otherwise (engine-domain.md, section 13). A retry below the domain hides
  its cost from the budget, and adds load just when the peer is
  struggling.
- **Limits and the worst case.** Each protocol crate exports its `Limits`
  and `worst_case`: its connections, intake and output caps, the largest
  message of each kind, and its decoders' stack depths.
- **What outlives a process carries a version:** the engine's blocks in
  forge comments and wiki pages, the store's records and the credentials
  file. A new engine reads what the one before it wrote.
- **Configuration is data,** read by the shell at startup: peers'
  addresses (names are resolved then: skein's io connects to addresses),
  certificates and the credentials file (section 5).

## 4. temper's channel

One wire format carries both channels, each with its own vocabulary: the
engine's link with a worker (worker-domain.md, section 2) and a worker's
with each agent it spawns (worker-domain.md, section 6).

- **Sized frames** (programming-model.md, section 8): a fixed header
  holding the message's kind and its body's length, which is checked
  against that kind's limit before anything is set aside; a body of
  fixed-width integers and length-prefixed bytes, read with a `Reader`
  into a plain struct and written with a `Writer` of its exact length.
  Both ends are temper's, so there is no JSON on the channels.
- **The hello opens each channel.** The first message each way says the
  protocol version; on the engine's link, the worker's also carries its
  name and secret, and the domain's hello (worker-domain.md, section 2),
  in one message. A version or a secret the other end does not accept is
  refused with a small fixed-size refusal, and the connection closes
  before either domain hears of it.
- **The version is agreed at the hello.** The hello's layout never
  changes, so any two releases can read each other's. Each side says the
  range of versions it speaks, and the channel uses the highest both
  speak, or is refused if there is none. Every frame and payload schema
  is then read and written under that version.
  - **For now, one version per release.** The engine, its workers and
    their agents upgrade together.
  - **Rolling upgrades later.** A release that speaks two versions adds
    codecs; the handshake stays as it is.
  - **An agent speaks its worker's version.** The engine learns that
    version at the hello and encodes the run's payloads for it.
  - **Unknown kinds are framing errors.** Since the version is agreed at
    the hello, a kind it does not define cannot come from a newer peer.
- **Liveness, on the engine's link only.** A side that has sent nothing
  for an interval sends a ping; a link that has heard nothing for longer
  is lost, and the domains' graces begin. On an agent's channel, a process
  that dies closes its pipes, and the worker's watchdog (worker-domain.md,
  section 6) covers one that lives without progress.
- **Authenticated and private off the host.** The engine's link carries
  the forge's work and LLM access tokens, so between hosts it runs over
  TLS, which the engine terminates, and each worker presents its own
  secret, compared in constant time. On one host it may run in plaintext
  over loopback. An agent's channel is its pipes, which only its worker
  holds, and needs neither.
- **A slow reader never holds up the rest.** Each channel's output is
  capped, and the protocol layer never makes the domain wait on one
  channel: that would stall every other peer behind it. Three things keep
  the cap from being reached:
  - **Most messages are bounded by the domains' own limits.** These are
    assignments, cancels, acknowledgements, and relayed calls with their
    answers. Their counts follow from slots and calls in flight, so each
    channel's cap is sized from those limits.
  - **Inbound events are bounded in bursts.** An item's inbox keeps what
    it relayed until the run answers, so a burst to one run is at most an
    inbox. A notice replaced after a run was given it goes again. Those
    repeats come only as fast as decisions are made, and a peer that is
    reading drains them.
  - **Facts are pulled.** They are the one stream with no such bound, so
    they wait in a bounded queue of the domain's until the protocol layer
    has room for them. Past that queue, they are dropped and counted
    (worker-domain.md, section 2).
  - **A channel that stops draining is closed.** If its output has not
    drained a chunk within its progress deadline, or still fills its cap,
    its peer has stopped reading. The domains then see the channel as
    lost, which they already survive.

  So there is no credit for the domains to track. Credit would add state
  and events to three domains, for traffic that moves at the pace of the
  forge and people, and its only gain would be over a peer the deadline
  closes anyway. What a run was relayed on a channel that is lost,
  however it is lost, is the engine's open question of inbound events
  (engine-domain.md, section 15).
- **Names.** A run goes by its item, and an attempt by its item and its
  count, as the system worlds already name them (testing.md, 4.5); the
  protocol layers pack them into the domains' tokens.
- **Payload schemas, defined once.** A charter, an outcome, an inbound
  event, a relayed call and its answer, a fact and a snapshot each have
  one schema in `temper-channel`. The engine's protocol layer encodes
  what it assigns and decodes what runs answer; the agent's does the
  reverse; the worker reads only what it acts on and passes the rest
  through as bytes.

## 5. Credentials

- **The engine holds every credential:** its forge tokens, each worker's
  secret, and the LLM accounts. An LLM account is an OAuth grant: a
  refresh token, and the access token it last bought with its expiry.
- **One process refreshes an account.** A provider may rotate the refresh
  token each time it is used, so two processes refreshing one account
  would spend it twice. The engine refreshes, and writes each rotated
  token to its credentials file, atomically, before the access token it
  bought is handed out, so a crash between the two loses neither.
- **Accounts are the engine's domain's.** The domain decides when to
  refresh (a margin before expiry), which account a run uses, and what a
  failed refresh or an exhausted account means: the runs that need it
  wait, and a person is told. The OAuth server is a peer like any other.
- **Grants go down the channels.** An assignment carries a grant for each
  provider its charter names: the account, which of its access tokens,
  and when that token expires. The run asks for a fresh grant before its
  own expires, or once a call fails as unauthorized, as a host call its
  worker relays to the engine. A refresh token never leaves the engine.
- **Names through the domains, values beside them.** A domain names a
  credential, by its account and the access token's generation, and never
  holds its value, so no trace, snapshot or log of a domain can leak one
  (agent-domain.md, section 2). Each protocol layer keeps the values it
  carries in a small table under those names: the engine's fills a
  grant's value in as it encodes the message, the worker's holds it until
  the run's channel takes it, and the agent's puts it in the requests it
  sends. An entry goes once its token has expired or its name is
  released.
- **Logging in** comes later, through the web. For now an account starts
  from a grant made by the provider's own login and imported once into
  the credentials file.

## 6. LLM providers

- **Two providers from the start, both over OAuth:** Anthropic's Messages
  API, and ChatGPT's, OpenAI's Responses API as ChatGPT serves it to an
  account (the account's id is in the access token).
- **The stack,** from the socket up: TLS, the HTTP client, server-sent
  events, JSON tokens, the provider's decoder, and the session's
  conversation vocabulary (agent-domain.md, section 5). A request is
  measured, then written once into a body of its exact length.
- **Decoded on the way in, all of it.** A completion streams; the decoder
  builds its text and tool calls as events arrive, and decodes each tool
  call's input into the session's typed calls, within the session's
  limits. Nothing goes up as JSON text.
- **Failures, classified** by status and by the provider's error type into
  the session's failures: rate limited with when to try again,
  overloaded, unauthorized (renew the grant), the account exhausted (the
  account is spent, not the run), refused, malformed, and cut short (a
  stream that ends without its last event). Whether to try again is the
  session's decision.
- **A provider's identity in one place.** Anthropic's OAuth route admits
  only requests shaped like its own command-line client's (its headers
  and its system prompt), and has changed that shape before; each
  provider's crate keeps its request shape in one place, and its
  transcripts are captured again when the shape changes.
- **Connections** carry one exchange at a time and are kept between turns.
  The idle deadline between events is the connection's; a call's deadline
  is the session's.

## 7. The forge

Forgejo first: its REST API, over HTTP and JSON, and its webhooks to an
HTTP server in the engine. A webhook's signature (HMAC-SHA256) is checked
before anything else in it is read.

**SHA-256 and HMAC come from RustCrypto's `sha2` and `hmac`.** This is a
second outside dependency in step code, besides TLS
(programming-model.md, section 3). Unlike TLS, it is pure Rust, `no_std`
and deterministic, so it costs nothing in the replaying tiers.

**GitHub comes later, and the design leaves room for its strengths.**
The forge child domain asks for what it needs, not how to get it.
Each provider's protocol layer answers in the cheapest way its API allows,
for example:

- conditional requests, keeping each resource's validator in a bounded
  table, since GitHub does not count an unchanged answer against its rate
  limit;
- several reads in one query;
- CI from GitHub's own checks.

The protocol layer reports the provider's rate limit as it stands, so the
domain's budget follows whatever the provider counts.

**What the domain does about load** (engine-domain.md, section 12): one
listing per repository of what changed since the last pass, never one
per label and never one of the history; only live items held; CI read
from the commit statuses of the heads it holds, never from a history of
runs; webhooks as hints merged into one earlier pass; a request budget
with a share for each kind of call. So the cost of keeping up follows the
rate of change.

**What the protocol layer adds:**

- **Every listing paged,** at the largest page the forge allows, which it
  reads from the forge (`/settings/api`) at startup: a listing without
  both a page and its size is never sent, since Forgejo's Actions
  listings ignore the size without the page and answer with everything.
- **Listings decoded into summaries.** An item's body is skipped as it
  streams past; a read fetches one item, or one page of what it asks
  for, never a whole thread to search.
- **No retries** (section 3). A refusal for the rate goes up with its
  reset, and the domain's budget stops every call until then.
- **A few persistent connections,** one exchange each, under a cap on
  calls in flight.
- **Webhooks answered first,** before the hint goes up; the domain merges
  hints, so a burst of them, the engine's own writes' among them, costs
  one earlier pass.
- **The engine's own blocks:** its record, the outcomes it posts and
  notes' pages, encoded into comments and wiki pages and decoded back.
  The codecs the worlds use today (testing.md, 4.5) become the protocol
  layer's.

**Counted.** The fake forge counts the calls each pass makes, and the
engine's world checks them: an idle repository costs a fixed number of
calls per pass, whatever it holds, and a busy one a number that follows
what changed.

**Checked against a real Forgejo** before the client relies on them: that
listings honour least-recently-updated order; that Actions report each
job as a commit status on the head it ran on; which changes move an
item's updated time (dependencies, especially); how pages are capped. A
small conformance check, against a throwaway Forgejo, holds the fake
forge to these facts, as skein's conformance suite holds its simulator to
the kernel.

## 8. Later

- **People:** the engine's web, an HTTP server with server-sent events for
  watches, and signing in through the forge, the engine as its OAuth
  client.
- **Git:** each typed operation one invocation in a contained process
  (worker-domain.md, section 8), its output read in git's machine
  formats (NUL-separated, porcelain) by small scanning machines;
  credentials given per invocation, never in a URL or a file.
- **MCP:** JSON-RPC over a server's pipes, one document per line.
- **The store:** its records in temper's own sized format, written in
  order (engine-domain.md, section 13).

## 9. Testing

As testing.md applies skein's strategy:

- **Machines and decoders alone,** in machine worlds, against transcripts
  captured from the real peers (each provider's streams and errors,
  Forgejo's answers and webhooks) and fuzzed.
- **Protocol worlds** (testing.md, 2.2): the engine and a worker through
  temper's channel; a worker and its agents; an agent and the fake LLM
  provider; the engine and the fake forge. The system worlds' scenarios
  run again through bytes.
- **The fakes grow server sides:** the fake LLM provider serves both
  providers and an OAuth server that rotates refresh tokens; the fake
  forge serves Forgejo's API, sends webhooks and counts calls.
- **The real Forgejo check** (section 7), outside the two suites.

## 10. Building it

In order, each boundary designed in depth first:

1. **temper's channel:** `temper-channel`, then the engine's, the
   worker's and the agent's protocol layers for it, and protocol worlds
   running the system worlds' scenarios through bytes. It needs nothing
   new from skein.
2. **LLM providers and credentials:** accounts in the engine's domain and
   grants on the channels (below); both providers; the fake LLM
   provider's protocol layer; and from skein, its first pull: the TLS
   client, the HTTP client, server-sent events and JSON.
3. **The forge:** Forgejo's client and webhooks, the fake forge's protocol
   layer, the call counts and the real Forgejo check.
4. **People, git, MCP and the store.**

What the domains owe this layer, found while designing it:

- **The engine:** LLM accounts, with their refresh, their failures and
  their exhaustion; a grant in each assignment; renewing a grant as a
  relayed call.
- **The agent:** grants held by name, renewed before they expire and
  after an unauthorized call; an exhausted account as a failure of its
  own.
- **The worker:** grants relayed by name, with the rest of a run's calls.

## 11. Open questions

None yet beyond the domains' own (section 4 names the one it meets).
