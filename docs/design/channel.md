# temper's channel

Provisional, 2026-10-03. The wire temper speaks with itself: the engine's
link with each worker (worker-domain.md, section 2), and a worker's
channel with each agent it spawns (worker-domain.md, section 6). One
framing carries both, each with its own vocabulary. This document is the
in-depth design behind `protocol.md`, section 4. It covers:

- the frames;
- opening a channel;
- each channel's messages and the domain records they carry;
- the payloads the worker passes through;
- names and grants;
- liveness and flow control;
- the connections' state machines;
- the limits;
- where the code lives, and how it is tested.

The mechanics are those of skein's `programming-model.md` (sections 4, 7
and 8). The credentials that grants carry are `credentials.md`'s. What
the domains owe this design is in section 14, and what is open in
section 16.

## 1. In one page

- **One framing, two channels.** Every message is a frame: a fixed
  8-byte header holding its kind and its body's length, then a body of
  big-endian integers and length-prefixed bytes. Both ends are temper's,
  so there is no JSON.
- **Every length is checked before anything is set aside.** A body's
  length is checked against the largest that kind may have before its
  buffer is allocated. Each field's length is checked against what is
  left of the body, and each count against its field's limit. A failed
  check is a framing error: the channel closes, and nothing panics.
- **Opening is frozen; the rest is versioned.** Opening a channel takes
  three messages, `Open`, `Accept` and `Refuse`, plus `Ping`, and their
  layouts never change. They agree a version, authenticate a worker, and
  are followed by `Terms`, each side's largest body per kind. Only then
  does either domain hear of the channel.
- **The domains own reliability; the channel adds framing, versions,
  authentication and liveness.**
  - **The domains own:** the hello, fenced attempts, acknowledged
    answers and redialling.
  - **The channel adds:**
    - on the engine's link: pings, a silence deadline and a stall
      deadline;
    - on an agent's channel: none of those. The pipes end with the
      process, and the worker's watchdog covers an agent that lives
      without progress.
- **Payloads are typed at their ends.** A charter, an outcome, an inbound
  event, a relayed call and its answer, a fact and a snapshot each have
  one schema in `temper-channel`. The engine's and the agent's protocol
  layers encode and decode them. The worker passes them through as bytes.
- **Credentials travel beside the domains.** An assignment and the start
  of a run carry their grants' values. Each protocol layer strips the
  values as it decodes and fills them in as it encodes, so a domain only
  ever sees `(account, generation)`.
- **No credit.** Each channel's output cap is sized from the domains'
  limits. Facts go only while there is room, and a channel that stops
  draining is closed as lost.
- **Translation is pure and shared.** Each protocol crate turns wire
  messages into domain records, and back, with small total functions. The
  system worlds use the same functions without bytes, and the protocol
  worlds with them.

## 2. Where it sits

```
engine                                worker                                 agent
domain                                domain                                 domain
  ▲ engine records                      ▲ worker records (host, agent child)    ▲ agent records
temper-engine-protocol                temper-worker-protocol                 temper-agent-protocol
  ▲ wire messages                       ▲ wire messages     ▲                   ▲ wire messages
temper-channel  ◄──── the link ────►  temper-channel      temper-channel ◄──► temper-channel
  ▲ stream                              ▲ stream            ▲ stream            ▲ stream
tls (between hosts)                   tls (between hosts)   │                   │
  ▲                                     ▲                   │                   │
io socket                             io socket             io pipes  ◄────►  stdin, stdout
```

- **`temper-channel`** depends on skein-lib only. It holds the frame
  machine, the frozen opening, both vocabularies (the wire messages), the
  payload schemas, and the function that turns sizes into each kind's
  largest body.
- **Each protocol crate** holds its connections, their deadlines, its
  table of credential values, and the translation between the wire and
  its domain.
- **The link** runs over TCP, with TLS between hosts, which the engine
  terminates (skein's TLS server side, pulled for it). A worker knows the
  engine's address, the name its certificate carries, and what to trust
  it by. On one host it may
  run in plaintext: the engine's plaintext listener binds loopback only,
  and a worker sends its secret in plaintext only to a loopback address.
- **An agent's channel** is its process's pipes. The worker writes the
  agent's standard input and reads its standard output. Standard error
  is the agent's log for operators: the worker keeps its tail as the
  detail of how the agent ended, and never parses it.

## 3. Frames

### 3.1 The header

```
offset  size  field
0       2     kind       u16, big-endian
2       2     reserved   u16, zero in every version so far
4       4     length     u32, big-endian: the body's length, header excluded
8       ...   body
```

- **The header is frozen.** Every version reads it the same way, so a
  peer of any version can read `Open`, `Accept` and `Refuse`.
- **`reserved` must be zero.** A future version may give it a meaning,
  such as compression, only after both sides have agreed that version.
- **Kinds are unique across both channels and both directions:**
  - `0x0001`–`0x000f`: the frozen opening (section 4);
  - `0x0010`: `Terms`;
  - `0x01xx`: the engine's link (`0x0101`… up from the worker,
    `0x0181`… down to it);
  - `0x02xx`: an agent's channel (`0x0201`… up from the agent, `0x0281`…
    down to it).

  A kind that does not belong to this channel, in this direction, at this
  point and in the version agreed is a framing error. Since the version
  was agreed when the channel opened, an unknown kind cannot come from a
  newer peer, so it is not skipped (programming-model.md, section 8).

### 3.2 Reading

The machine reads a frame in three steps:

1. **The header:** a demand to fill 8 bytes. The kind must be one
   expected next. The length must be at most that kind's largest body,
   which is checked before anything is set aside.
2. **The body:** a buffer of exactly `length` bytes, allocated once the
   header has passed. It is filled from demands of at most
   `Limits::chunk` bytes each, so the stream below needs an intake of only
   one chunk, however large the frame.
3. **The decode:** a `lib::Reader` over the body, into the kind's plain
   struct.
   - Byte fields are copied into boxes of their exact length.
   - Bytes left over once the decode is done are a framing error.
   - The decoded message goes up only when the side above has room for
     it, and the next header is demanded only after that, so a full
     domain queue stops the reading.

### 3.3 Primitives

| Type | On the wire |
|---|---|
| `u8`, `u16`, `u32`, `u64` | fixed width, big-endian (`lib::Reader`'s order) |
| `bool` | a `u8`, 0 or 1 |
| bytes | a `u32` length, then the bytes |
| list | a `u32` count, then the items |
| option | a `u8` tag (0 none, 1 some), then the value |
| enum | a `u8` tag, in declaration order from 0, then the variant's fields |
| token | a `u64` |
| commit | 32 bytes |
| duration | a `u64` of nanoseconds |

- **No time ever crosses a channel; only durations do.** Each receiver
  turns a duration into a deadline on its own clock (`credentials.md`,
  section 7). This holds on the agent's pipes too, though both ends share
  a clock.
- **Each length is at most what is left of the body,** and each count at
  most what is left, since every item takes at least a byte. A byte field
  or list with a limit of its own (a charter's bytes, the repositories of
  a workspace) is also checked against that limit. An unknown tag is a
  framing error.
- **Text is bytes.** The channel never checks UTF-8: what the domains
  carry as bytes, the wire carries as bytes.

### 3.4 Writing

Writing a message is sized (programming-model.md, section 8):

1. Measure the body.
2. Make a `lib::Writer` of the header plus the body.
3. Write the header and the fields.
4. `finish()`, then move the frame down in one `Send`.

A body larger than that kind's largest is the sender's bug, and is
asserted. The handshake's `Terms` (section 4.3) make sure no body the
domains may produce can be larger.

## 4. Opening a channel

### 4.1 The frozen messages

These four messages, and their layouts, are the same in every version.

| Kind | Name | Body | Largest |
|---|---|---|---|
| `0x0001` | `Open` | magic `b"tmpr"`; channel `u8` (1 the link, 2 an agent's); lowest `u16`; highest `u16`; name bytes; secret bytes | 256 |
| `0x0002` | `Accept` | version `u16` | 2 |
| `0x0003` | `Refuse` | reason `u16`; text bytes, for operators only | 512 |
| `0x0004` | `Ping` | nothing | 0 |

- **`Open` is sent by the side that starts the channel:** the worker,
  both on the link (it dials) and on an agent's channel (it spawns).
  `lowest` and `highest` are the range of versions it speaks. On the link,
  `name` and `secret` are the worker's (each at most 64 bytes); on an
  agent's channel, both are empty.
- **The other side answers with `Accept` or `Refuse`.** `Accept` gives the
  highest version both speak. `Refuse` closes the channel once it has
  been flushed.
- **Refusal reasons** are frozen codes. A code the reader does not know
  reads as "other".

  | Code | Reason |
  |---|---|
  | 1 | **version**: no version both speak |
  | 2 | **unauthorized**: an unknown name or a wrong secret. One reason for both, so names cannot be probed |
  | 3 | **limits**: the other side accepts less than this side may send (4.3) |
  | 4 | **busy**: the engine's domain turned the worker away |
  | 5 | **replaced**: a newer channel of the same worker opened (4.4) |
  | 6 | **framing**: the peer broke the framing; sent best effort before closing |
- **`Ping`** is the link's liveness (section 10); an agent's channel never
  sends it.

### 4.2 The sequence

```
worker                                  engine (or agent)
Open { versions, name, secret }  ───►
                                 ◄───  Accept { version }   or   Refuse { reason }
Terms { largest bodies }         ───►
                                 ◄───  Terms { largest bodies }
(the domains hear of the channel; on the link, the worker's domain hello follows)
```

- **The engine checks an `Open`:** the magic and the channel, then the
  version range, then the worker's name and secret. The engine knows each
  worker's secret, and compares the one offered in constant time
  (`credentials.md`, section 8). It then sends `Accept` and its `Terms`
  back to back.
- **An agent checks** only the magic, the channel and the versions.
- **Neither domain hears of a channel until both `Terms` have passed:**
  - the worker's domain gets `Connected` (on the link) or `Spawned` (on
    an agent's channel) only then;
  - the engine's domain gets `Hello` only after the worker's domain hello
    has come, as the first versioned message after the `Terms`;
  - a channel refused during opening is never announced.
- **Deadlines.** On the link, opening must finish within
  `Limits::handshake` of the connection being made. The engine then waits
  `Limits::hello` for the domain hello. On an agent's channel, opening
  must finish within the spawn's own deadline (`Request::Spawn`'s
  `deadline`); a spawn whose opening fails or runs out of time ends as
  `Unspawned`, with the reason as its detail.

### 4.3 Terms

`Terms` (`0x0010`, versioned) lists, for each kind its sender receives,
the largest body it accepts: a list of `(kind u16, largest u32)`.

- **Each side checks the other's.** For every kind it may send, the
  largest it could produce, computed from its own limits, must fit within
  the largest the other side accepts. If it does not, the side sends
  `Refuse { limits }` and closes. The text names the kind, for the
  operator who gave the two sides different limits.
- **Why.** An engine and its workers each have limits of their own.
  Without this check,
  a charter larger than a worker accepts would close the channel on its
  first assignment, and the redial would bring the same assignment back,
  forever. Refused at the entrance, the mistake shows at once, and no
  work is lost.
- **On an agent's channel** the agent's limits are the ones its worker
  gives it, so the check always passes. It stays anyway, so an agent
  started with the wrong limits fails at its spawn.

### 4.4 One channel per worker

A worker has at most one channel on the engine.

- **An `Open` under a name that already has a channel** is the worker
  coming back before the engine noticed it had gone. The engine closes the
  older channel first, sending `Refuse { replaced }` best effort, and its
  domain gets `Lost` for it. Only then does the engine accept the newer
  channel.
- **The engine's domain knows workers by their channels**
  (`temper-engine-domain-fleet`). A worker's name stays in the protocol
  layer, for authentication and for operators.

### 4.5 Versions

- **One version per release for now:** v1. The range in `Open` lets a
  later release speak two versions at once. Only the codecs grow for
  that; the opening stays as it is (protocol.md, section 4).
- **v1 is not frozen until temper first ships.** Until then its messages
  may change freely. What must never change is the header and the frozen
  messages.
- **A version covers the frames and the payload schemas.** The engine
  learns a worker's version when the link opens, and encodes each run's
  payloads for it. A worker's agents speak the worker's own version,
  since they come from the same release.

## 5. The engine's link

Kinds `0x01xx`. In the tables:
- *Worker* and *Engine* name the domain records at each end.
- *run* and *attempt* are the names of section 8.
- A **call** is answered exactly once; a **notice** is not answered.

### 5.1 Up, worker to engine

| Kind | Message | Fields | Worker domain | Engine domain | Shape |
|---|---|---|---|---|---|
| `0x0101` | `Hello` | slots `u32`; workstreams list of bytes; hosting list of { run, attempt, phase `u8` } | `Request::Hello` | `Event::Hello` | first, once |
| `0x0102` | `Answer` | run; attempt; answer (5.3) | `Request::Answer` | `Event::Answer` | answers `Assign` |
| `0x0103` | `Relay` | run; attempt; call `u64`; body: a call (7.4) | `Request::Relay` | `Event::Relay` | call |
| `0x0104` | `Bounced` | run; attempt; event `u64`; bounce `u8` | `Request::Bounced` | `Event::Bounced` | notice |
| `0x0105` | `Told` | run; attempt; fact (7.5) | `Domain::pop_told` | `Event::Told` | notice |
| `0x0106` | `Rejected` | run; attempt; account `u32`; generation `u64` | owed (section 14) | owed | notice |
| `0x0107` | `Exhausted` | run; attempt; account `u32`; retry after: duration | owed | owed | notice |

### 5.2 Down, engine to worker

| Kind | Message | Fields | Engine domain | Worker domain | Shape |
|---|---|---|---|---|---|
| `0x0181` | `Assign` | run; attempt; workspace (5.3); save: option of bytes; charter (7.1); snapshot: option of bytes (7.6); grants: list of grants with their values (section 9) | `Request::Assign` | `Event::Assign` | call |
| `0x0182` | `Inbound` | run; attempt; event `u64`; body: an inbound event (7.3) | `Request::Inbound` | `Event::Inbound` | notice |
| `0x0183` | `Cancel` | run; attempt | `Request::Cancel` | `Event::Cancel` | notice |
| `0x0184` | `Relayed` | run; attempt; call `u64`; answer: a served call (7.4) | `Request::Relayed` | `Event::Relayed` | answers `Relay` |
| `0x0185` | `Acknowledge` | run; attempt | `Request::Acknowledge` | `Event::Acknowledged` | notice |
| `0x0186` | `Grant` | run; attempt; account `u32`; generation `u64`; valid: duration; token bytes; account id bytes (section 9) | owed | owed | notice |

`Request::Refuse` is not a message of its own. It is the frozen
`Refuse { busy }`, after which the channel closes; the engine's domain
then gets `Lost` for it, as for any channel it heard the hello of.

### 5.3 The fields with structure

- **A workspace:** key bytes, then a list of repositories. Each
  repository has:
  - tag `u32`: the deployment's index for it, which the worker echoes in
    what landed;
  - name bytes;
  - remote bytes;
  - start: `Base` { branch }, `Branch` { branch }, `Commit` { commit } or
    `Saved` { branch };
  - access: `ReadOnly`, or `Writable` { push branch };
  - identity: `u32`, the account of the git grant it uses.

  The engine knows each repository of the deployment by its index: its
  name, its remote and its git identity. Its protocol layer fills them
  in, as the system worlds do today.
- **An answer** is one of:
  - `Refused` { refusal }: busy, or invalid with what was invalid;
  - `Ended` { outcome (7.2), work };
  - `Parked` { snapshot: option, work };
  - `Failed` { failure, detail bytes, work }.

  A failure is the host's `Failure`, the whole tree of it, encoded as
  nested enums. Work is what landed, a list of { tag `u32`, commit }, plus
  an option of the save's landings.
- **The engine's protocol layer translates an answer** into the engine's
  records (`temper-engine-domain`'s `Answer`):
  - **Refusals:** `Busy` and `Invalid`.
  - **Failures are classed:** an unprepared workspace that may succeed
    later, and a cancel, are `Transient`; one that names what the forge
    lacks, or refuses, is `Permanent`; the run's own failures are `Run`,
    and the agent's are `Agent`.
  - **An outcome that does not decode** is `Failed { Agent }`, since the
    agent said what no engine reads.
  - **What is dropped:** the detail goes to operators' logs, not to the
    domain. The save's landings are dropped, since the engine finds the
    saved-work branch on the forge.

  This is the translation the system worlds use today, made the protocol
  layer's own.

## 6. An agent's channel

Kinds `0x02xx`. One process carries one run, so the channel names
neither: a call is named by the run.

### 6.1 Down, worker to agent

| Kind | Message | Fields | Worker domain (`channel::Down`) | Agent domain | Shape |
|---|---|---|---|---|---|
| `0x0281` | `Start` | charter (7.1); snapshot: option (7.6); repositories: list of { name bytes, writable `bool` }; endpoints: list of endpoint descriptors (llm.md), each with its account; grants: list of grants with their values | `Start` | `Event::Start` | call; first, once |
| `0x0282` | `Event` | event `u64`; body: an inbound event (7.3) | `Event` | owed: inbound events | notice |
| `0x0283` | `Answer` | call `u64`; reply: `Relayed` { answer (7.4) }, `Pushed` { push: the push's result (below) }, `Unavailable`, `Busy`, `Withdrawn` or `TooLarge` | `Answer` | `Event::Pushed`, `Event::HostCancelled`; relayed: owed | answers `Call` |
| `0x0284` | `Cancel` | nothing | `Cancel` | `Event::Cancel`, once admitted | notice |
| `0x0285` | `Grant` | account `u32`; generation `u64`; valid: duration; token bytes; account id bytes (section 9) | owed | owed | notice |

- **A push's result** is the worker's domain's, field for field: landed,
  nothing to push, stale, or failed with its typed reason, the failed
  repository's index, and the tail of git's diagnostic (at most 512
  bytes) with the count of bytes dropped before it (agent-domain.md,
  4.4).

- **The worker's protocol layer completes the start.**
  - **The repositories** are where the run's checkout sits: each a
    directory of the workspace, which is the agent's working directory.
    They come from the worker's domain (section 14).
  - **The endpoints** are every endpoint in the worker's endpoint table
    (llm.md), so the worker never reads the charter to choose among them.
  - **The grants' values** come from its table, by the names its domain
    gives.
- **The agent's protocol layer turns a start into the domain's
  `Event::Start`.** It decodes the charter into the agent's
  `run::Charter`, and opens a root for each repository through io, which
  become the checkout's roots. It strips the grants into its table and
  keeps the endpoint descriptors for its LLM connections (llm.md).
  `Request::Admitted` gives it the run's name, kept to address a cancel.
  A cancel that crossed a refusal finds no run, and is dropped.

### 6.2 Up, agent to worker

| Kind | Message | Fields | Agent domain | Worker domain (`channel::Up`) | Shape |
|---|---|---|---|---|---|
| `0x0201` | `Call` | call `u64`; ask: `Push` { message bytes } or `Relay` { body: a call (7.4) } | `Request::Push`; relays: owed | `Call` | call |
| `0x0202` | `Withdraw` | call `u64` | `Request::CancelHost` | `Withdraw` | notice |
| `0x0203` | `Fact` | fact (7.5) | its facts, drained while there is room | `Fact` | notice |
| `0x0204` | `Long` | span: duration | `Request::Checking`, its deadline made a span | `Long` | notice |
| `0x0205` | `LongDone` | nothing | the fact that the checks finished | `LongDone` | notice |
| `0x0206` | `Waiting` | heard `u64`: the last event read, by name | owed | `Waiting` | notice |
| `0x0207` | `Finish` | `Ended` { outcome (7.2) }, `Parked` { snapshot: option }, or `Failed` { failure `u8` } | `Request::Answer` | `Finish` | answers `Start`; last |
| `0x0208` | `Rejected` | account `u32`; generation `u64` | owed | owed | notice |
| `0x0209` | `Exhausted` | account `u32`; retry after: duration | owed | owed | notice |

- **Order.** The agent's protocol layer sends a step's facts before its
  requests, so the end of one check goes up before the next one's `Long`,
  and the run's last facts go before its `Finish`. Nothing goes up after
  `Finish` (worker-domain.md, section 6).
- **Mapping the worker's channel.** The worker's domain reads one message
  at a time (`Request::Read`), so the machine demands the next frame only
  while a read is waiting.
  - A frame that decodes is `Event::Received`.
  - A framing error is `Event::Malformed`; the agent broke the rules.
  - The end of the stream is `Event::Hangup`.
- **Sending.** `Request::Send` ends with `Event::Sent` once the frame is
  queued within the output cap, and waits while it is not. A closed
  channel ends it with `Event::Unsent`. The agent child domain sends one
  message at a time, so this is all the credit it needs: an agent that
  stops reading stalls its own sends, and the watchdog decides.

## 7. Payloads

Opaque to the worker, typed at their ends. Each is the body of a byte
field, under its own limit, and is decoded by the protocol layer that
reads it. A payload that does not decode is not a framing error: the
frame was fine. What happens to it is its reader's translation, as below.

### 7.1 A charter

Encoded by the engine's protocol layer from the engine's `Charter`, and
decoded by the agent's into `run::Charter`. Its schema is the engine's
type as it stands:
- why it is due;
- the brief's sections, each a kind and text, or a reason the text is
  missing;
- the plan's guidance;
- the grants;
- what it may finish with;
- its budget, with the time as a duration;
- the models, each an endpoint, a model name and a most-tokens;
- what its trace keeps of each kind of report.

The agent's protocol layer renders the brief as the text the LLM reads,
under headings. It gives the run its repositories from the start message,
not from the charter. A charter that does not decode is the agent's
refusal of the run (section 14).

### 7.2 An outcome

Encoded by the agent's protocol layer from the run's declared outcome,
and decoded by the engine's into its `Outcome`:
- a change, by its message;
- a verdict with its text and children;
- a report;
- a plan, steps or tasks;
- a reply;
- finished;
- a release;
- an escalation.

The engine's undecodable outcome is the agent's failure (5.3).

### 7.3 An inbound event

The engine's `Inbound`:
- news of a comment, by its id;
- news of reviews, or of a pull request's state and head;
- an item finished or held, by its item;
- a person's decision.

It travels with the event's name, `u64`, which the engine issues (section
14). It is decoded by the agent's protocol layer once its domain hears
inbound events (agent-domain.md, section 10).

### 7.4 A relayed call and its answer

The engine's `Call` and `Served`:
- **a call:** a forge read, a recall, a note, a comment or an escalation;
- **its answer:** what was read, the entries recalled, what was noted,
  the comment posted, or why it was not served.

The schema of a read's answer is the part of the forge's answers that a
run reads. For now it mirrors the engine's `Served` as it stands, field
for field; it changes with the agent's half of relaying
(agent-domain.md, section 10), which v1, not frozen until temper first
ships (4.5), allows at no cost.

When the engine's protocol layer cannot decode a call, it answers the
call itself, at once, as `Unserved { Invalid }`, without the domain. It
is refused at the entrance, like a busy call.

### 7.5 A fact

`kind u8` (text, progress, a call, a tool, usage: the engine's views'
`Kind`) and content bytes, under the fact limit.

- **The agent's protocol layer projects** the run's and the sessions'
  facts into it (agent-domain.md, section 7).
- **The engine's protocol layer decodes** it into `Event::Told`, and
  drops one that does not decode: facts are best effort.

### 7.6 A snapshot

What a parked run hands over, and what it resumes from:
- **The agent's own.** Only the agent's protocol layer reads it. The
  engine keeps it in its store, and the worker passes it through.
- **Versioned on its own.** It outlives the channel that carried it: an
  engine may hand a snapshot made by one release to an agent of the next.
  So it starts with a version of its own (protocol.md, section 3), and an
  agent reads the versions its release still supports.
- **Not built yet.** Parking is the agent's to build (agent-domain.md,
  section 10).

## 8. Names

- **On the link, a run is named by its item, and an attempt by its item
  and its count.** These are the packings the system worlds already use:
  - **A run's token:** the repository's index in the high 32 bits, the
    item's number in the low 32.
  - **An attempt's token:** the repository in the top 8 bits, the
    number in the next 24, and the count in the low 32. So every attempt
    of every item has a name of its own, and its item can be read back
    from it.
- **The engine's protocol layer packs and unpacks them.** The worker's
  domain uses them as the tokens they are. An item or a count beyond the
  packing (a repository index of 256 or more, a number of 2^24 or more, a
  count of 2^32 or more) cannot be named. A deployment of more than 256
  repositories is refused at the engine's startup; the rest is section
  16.
- **A relayed call** is named by the worker (`call`). The engine echoes
  that name in `Relayed`, and never reads it.
- **On an agent's channel** the run is implicit, and a call is named by
  the run (`Up::Call`'s `call`).
- **An inbound event** is named by the engine (section 14), and the name
  travels down to the agent, which can name it back.

## 9. Grants

Their meaning is `credentials.md`'s (sections 4 and 7). Here is how the
channels carry them.

- **The assignment carries its attempt's first grants,** values included,
  in `Assign`: one per LLM account its charter's endpoints use, and the
  git grant.
  - **Why inline.** The worker's git needs its grant before it can
    prepare the workspace; inline, the grant cannot arrive behind the
    assignment that needs it.
  - **What the engine sends later** goes as `Grant` messages: after each
    refresh, and after every hello for the attempts it keeps.
- **A value is two byte fields:** the bearer token, and the provider's
  account id, empty unless the provider is ChatGPT (`credentials.md`,
  section 4). Each is at most `Sizes::token_bytes`. Apart, the id never
  has to be cut out of the token.
- **Each protocol layer keeps values in its table:**
  - **On decode,** it moves each value into its table, by account, and
    the domain gets the name `(account, generation)` and how long the
    token is valid.
  - **On encode,** it fills the value in from its table. A grant whose
    generation is older than the two the table holds is dropped from the
    message as it is encoded: a newer one follows.
  - **`valid`** is written as the time left on the writer's clock, so the
    time a grant waited at the worker is taken off what the agent is told.
- **The worker splits an attempt's grants.** It keeps the git grant for
  its own git operations, and passes the LLM grants to the agent: in
  `Start`, and in `Grant` messages afterwards.
- **`Rejected` and `Exhausted`** go up, agent to worker to engine, fenced
  by attempt like every other message. The worker passes them on. One
  that names an attempt the worker does not host is dropped.
- **Secrets on the link.** Grants and the worker's secret cross the link
  only over TLS, or in plaintext over loopback (section 2).

## 10. Liveness and flow control

- **Pings, on the link only.** A side that has sent nothing for
  `Limits::ping` sends a `Ping`. A link that has received no frame for
  `Limits::silence` (at least three pings) is lost. Pings never reach a
  domain. A channel proves itself to the worker's domain
  (worker-domain.md, section 2) by what the engine's domain says, not by
  pings.
- **A stall deadline, on the link only.** A link whose output has been
  queued without a chunk draining for `Limits::stall` is lost: its peer
  has stopped reading. Only the peer's draining counts as progress, never
  this side's own writes (programming-model.md, section 7). A worker's
  stalled agent channel is the watchdog's to catch (6.2).
- **The output cap** of each channel is sized from the domains' limits.
  It is the most the domains can have queued on it at once, so a channel
  never refuses a message its domain sends (protocol.md, section 4).
  Engine to worker, per worker:

  ```
  slots × (largest Assign + inbox × largest Inbound + run_calls × largest Relayed
           + Cancel + Acknowledge + accounts × largest Grant)
  ```

  Worker to engine:

  ```
  Hello + slots × largest Answer + stalled × largest Relay + bounces × Bounced
  + slots × run_calls × (Rejected + Exhausted) + reserve for facts
  ```

  - **The inbox term** bounds bursts. An item's inbox keeps what it
    relayed until the run answers. A notice replaced after a run was
    given it goes again, at the pace of decisions, which a peer that is
    reading drains.
  - **On an agent's channel,** the agent child domain's outbox bounds
    what goes down (`limits::outbox`). Up, the agent's own limits bound
    it: its calls in flight, its one `Finish` and its facts.
- **Facts go only while there is room.** The worker's protocol layer
  takes a told fact (`Domain::pop_told`) only while the link's queued
  output leaves the reserve for everything else free. The agent's
  protocol layer does the same with its facts toward the worker. Facts
  that do not fit are dropped and counted by the domain that holds them.
- **A channel that still fills its cap is closed as lost.** With caps
  sized as above, only a peer that stopped reading can do that, and the
  stall deadline closes it in any case.
- **Relays outlive channels.** The engine may answer a relayed call on a
  later channel than the one it came on (engine-domain.md, section 8). So
  the worker's protocol layer keeps every relay in flight in one table
  across channels, bounded by the host's calls (slots × run_calls).
  - A relay ends with `Event::Relayed` when its answer comes, on any
    channel.
  - It ends with `Event::RelayCancelled` once `Request::CancelRelay` has
    asked; an answer arriving after that is dropped.
  - The worker's domain sends `Relay` only while the link is open. It
    holds relays itself while the link is down (`Limits::stalled`).

### 10.1 Both directions at once

Both channels are full duplex: what one side sends does not follow from
what it reads. A side waiting for its peer's next frame must still be
able to send. skein's stream contract (lib.md, section 7) answers one
demand at a time, with `Bytes` or with `Room`, and a demand cannot be
restated while it is outstanding. So a side whose read is outstanding
cannot ask for room.

- **What skein owes:** room asked for while a read is outstanding, each
  answered once (protocol.md, section 10). The frame machine then asks
  for room whenever it has a frame to send, whatever its read.
- **Until then, the machine holds a grant of room.**
  - With every demand it states, it asks for all the room it lacks, up to
    the output cap.
  - It keeps what `Room` gives as its grant, and sends within it.
  - A frame that does not fit waits in the machine. What waits there and
    what is queued below stay within the cap together, so the bounds of
    this section hold.
  - Each answer to a read lets the machine ask again. On the link the
    peer's pings bound the wait to `Limits::ping`.
  - On an agent's channel, each message from the agent lets the worker
    ask again. Between two of them the worker sends little: the agent
    child domain sends one message at a time, an inbound event wakes the
    agent, and a waiting agent hears only grants, one per refresh, which
    its run's wall time bounds. A grant of a whole cap outlasts that.
- **The machine never asks for room it does not need.** A demand with
  room that is free at once would be answered at once, and asked again,
  forever.

## 11. Connections

Each end's connection is a state machine whose state says which deadline
runs. The protocol layer arms that deadline, and the machines keep none
(programming-model.md, section 9). Every state also takes these, and the
tables leave them out:
- the stream below failing or ending, a framing error, or the deadline
  passing: each closes the connection;
- a domain request for a connection that is gone: dropped. The domains
  have already heard, or will hear, that it is lost, and they resend
  what they must after the next hello.

### 11.1 The engine's end of a link

```
state        event                                   next         does / emits
accepted     tls done, or plaintext on loopback      opening      arm handshake
opening      Open: magic, channel, version, name,    terms        older channel of the name closed
             secret all good                                      (replaced); Accept; Terms
             Open: anything else                     closing      Refuse (version, unauthorized)
terms        Terms that fit                          hello        arm hello
             Terms that do not                       closing      Refuse (limits)
hello        Hello                                   open         Event::Hello { channel }
             any other kind                          closing      (framing)
open         a frame                                 open         its event; or, for a relay that
                                                                  does not decode, Relayed (invalid)
             a request                               open         its frame, grants' values filled
             Request::Refuse                         closing      Refuse (busy)
             silence, stall                          closing
closing      flushed and closed                      closed       Event::Lost { channel }, if open
                                                                  was reached
```

### 11.2 The worker's end of a link

```
state        event                                   next         does / emits
idle         Request::Dial                           connecting   io connect; arm handshake
connecting   connected                               opening      tls, then Open
opening      Accept, then Terms that fit             open         Terms; Event::Connected
             Refuse, Terms that do not fit           closing      (Refuse (limits) for the second)
open         a request                               open         its frame; pop_told while room
             a frame                                 open         its event; Assign and Grant
                                                                  values to the table; Relayed ends
                                                                  its relay
             silence, stall                          closing
closing      closed                                  idle         Event::Lost (once per Dial)
```

The domain dials again on its own backoff (worker-domain.md, section 2).
The protocol layer never redials.

### 11.3 The worker's end of an agent's channel

```
state        event                                   next         does / emits
spawning     io spawned the process                  opening      Open, on its stdin
opening      Accept, then Terms that fit             ready        Terms; Event::Spawned
             Refuse, Terms that do not, deadline     closing      Event::Unspawned { detail }
ready        Request::Send                           ready        its frame, grants' values filled;
                                                                  Sent once queued
             Request::Read                           ready        the next frame: Received,
                                                                  Malformed or Hangup
closing      the process's pipes closed              closed       (Exited and Reaped are io's)
```

Signals, waits and reaping stay io's, as the agent child domain asks for
them. The channel is done once its stdout is read to the end
(worker-domain.md, section 6).

### 11.4 The agent's end

```
state        event                                   next         does / emits
waiting      Open on stdin                           terms        Accept; Terms
terms        Terms that fit                          starting
starting     Start                                   rooting      grants to the table; a root per
                                                                  repository asked of io
rooting      every root opened                       running      Event::Start
             a charter that does not decode, or a    closing      Finish { Failed }: the agent's
             root that fails                                      refusal of the run (section 14)
running      a frame                                 running      its event
             a request                               running      its frame; facts while room
             stdin ended                             running      Event::Cancel, once admitted
             stdout broken                           closed       the process exits
```

The worker closing its end means the run must stop, so the agent treats
it as a cancel and winds down. If it does not, its tree is killed. The
agent's process exits after its `Finish` is flushed.

## 12. Limits and the worst case

`temper-channel`'s `Limits`, which each protocol crate embeds:

| Limit | What | Starting value |
|---|---|---|
| `chunk` | the most bytes of a body read at once | 64 KiB |
| `name_bytes`, `secret_bytes` | a worker's name and secret | 64 |
| `handshake` | from connect to open (the link) | 10 s |
| `hello` | from `Accept` to the domain hello (the engine) | 10 s |
| `ping` | idle time before a ping (the link) | 15 s |
| `silence` | hearing nothing before the link is lost | 45 s |
| `stall` | output not draining before the link is lost | 60 s |

- **Sizes** is a plain struct of numbers, each taken from the domains'
  limits: the largest charter, snapshot, inbound event, relayed call and
  answer, outcome, fact, detail and name; the repositories of a
  workspace; slots and workstreams; grants per attempt; token bytes.
  `temper-channel` turns it into each kind's largest body, and into the
  output caps of section 10. It does not depend on the domains; each
  protocol crate fills Sizes from its own domain's limits.
- **The worst case per connection:**
  - the intake below: a chunk plus a header;
  - one body being read: the largest body;
  - one decoded message waiting for room: about the same;
  - the output cap;
  - the machine's state.
- **Per process:**
  - **the engine:** `workers` connections, plus the ones still opening,
    which are capped and accepted one at a time (io.md, section 3);
  - **a worker:** one link, plus its agents' channels;
  - **an agent:** one channel.
- **The credential tables** are sized from the accounts the deployment
  has (`credentials.md`, section 4).

## 13. Where the code lives

- **`temper-channel`:**
  - the frame machine, a stream machine on lib's vocabulary (lib.md,
    section 7), with its handshake states;
  - both channels' wire messages;
  - the payload schemas, with their encoders and decoders;
  - Sizes, and each kind's largest body;
  - the primitives.

  It knows nothing of any domain, and nothing about connections beyond a
  stream.
- **`temper-engine-protocol`:** connections, the listener, opening (the
  secrets check, one channel per worker), the names' packing, the
  credential table, and the translation between the engine's records and
  the link's messages, with the charter and outcome payloads.
- **`temper-worker-protocol`:** the link's dialling, the relay table, the
  agent channels over io's pipes, the credential table, and the
  translation, which passes payloads through untouched.
- **`temper-agent-protocol`:** its one channel, rooting the checkout, the
  credential table, the translation of the start into a charter and of
  answers and facts back, and the LLM connections (llm.md).
- **Translation is a module of small total functions** in each protocol
  crate, between wire messages and domain records, with no I/O and no
  bytes beyond the payload codecs. The system worlds call these functions
  in place of the wiring they hold today:
  - `tests/engine/domain/src/names.rs` and the charter and outcome parts
    of `codec.rs`;
  - `tests/worker/domain/src/protocol.rs`;
  - `tests/agent/domain/src/protocol.rs` and `channel.rs`.

  That wiring retires, and the worlds keep their scenarios.

## 14. What the domains owe

- **The engine:**
  - **typed models in the charter:** each an endpoint, a model and a
    most-tokens, not opaque bytes copied in. The schema is then typed,
    and the engine knows which accounts to grant;
  - **inbound events named:** `Request::Inbound` carries the name of the
    inbox entry it relays, and `Event::Bounced` echoes it. Together with
    the agent's `Waiting { heard }` naming the last event read, this
    settles engine-domain.md, section 15 (bounced events, and events
    relayed on a channel then lost): the run says what it read, by name;
  - **what landed by tag:** `Work.landed` takes the tag the assignment
    gave each repository (the deployment's index), which the worker
    echoes. Answers then need no state in the protocol layer, and survive
    the engine's restarts;
  - **grants, `Rejected` and `Exhausted`,** as `credentials.md`, section
    9.
- **The worker:**
  - **`host::Repository` gains a tag,** echoed in `host::Landed` in place
    of the repository's place in the assignment. Its identity becomes the
    git grant's account;
  - **grant names:** `host::Assignment` carries its grants' names;
    `channel::Down::Start` carries the checkout's repositories (name,
    writable) and the attempt's LLM grant names; `Down::Grant`,
    `Up::Rejected` and `Up::Exhausted` are added;
  - **events named:** `Event::Inbound` and `channel::Down::Event` carry
    the event's name, and `Request::Bounced` names it.
- **The agent:**
  - **its half of the channel** (agent-domain.md, section 10): inbound
    events by name, waiting with the last event heard, parking with a
    snapshot and resuming from one, relayed calls and their answers, a
    refusal of its own (a charter it cannot take, which today fails as
    policy), and what a run spent;
  - **grants by name,** with `Rejected` and `Exhausted`
    (`credentials.md`);
  - **no policy in the translation.** The token budget's split across
    input, output and cache, which the system world's translation does
    today, moves into the agent's domain. The domain takes the engine's
    budget as it is, and the protocol layer decides nothing;
  - **facts with content,** for the fact payload's kinds, which the
    engine's views keep as each run's capture policy says.

## 15. Testing

As testing.md applies skein's strategy.

- **Step tests,** in `temper-channel`:
  - each primitive;
  - each message and payload, encoded then decoded;
  - each way a frame can be malformed (a kind out of place, a length over
    its largest, a count beyond what is left, an unknown tag, trailing
    bytes, a nonzero reserved field), refused without a panic;
  - Sizes against hand-computed bodies.
- **The machine world** (testing-strategy.md, 2.4) runs the frame machine
  between:
  - **below,** a stream that delivers peer bytes cut at random, grants
    room late, and ends or fails mid-frame;
  - **above,** a user that demands slowly and closes in every state.

  The peer bytes are:
  - messages generated valid from the seed, up to every limit;
  - the same messages mutated;
  - **transcripts:** golden frames kept beside the tests. The frozen
    messages are kept for every version ever shipped, and every kind of
    the current version, each with what it decodes to. A change to the
    encoding fails them.

  Its fuzz target decodes arbitrary bytes.
- **Protocol worlds** (testing.md, 2.2), each joining two stacks with
  in-memory streams cut and joined at random, in plaintext:
  - **the link:** the engine's protocol layer and domain against the
    worker's, running the whole worker's world's scenarios through bytes.
    Its faults:
    - connections cut mid-frame;
    - peers that stop reading (the stall deadline) or go quiet (silence);
    - garbage frames;
    - a wrong secret, a version with no overlap, terms that do not fit;
    - a worker that dials again while its old channel looks alive
      (replaced).

    The domain world's "copies of frames" become the resends that
    follow a redial, since TCP never repeats a frame;
  - **an agent's channel:** the worker's protocol layer and domain
    against the agent's, running the agent's world's scenarios. Its
    faults:
    - an agent that writes garbage (`Malformed`);
    - one that stops reading (its sends stall; the watchdog);
    - one that hangs up mid-frame;
    - one of the wrong version at its spawn;
  - **the agent's top-level world through both channels.**
- **The system worlds** stay at the domain tier, using the translation
  functions of section 13.

## 16. Open questions

- **The names' packing** limits a repository index to below 256, an
  item's number to below 2^24, and an attempt's count to below 2^32. If a
  forge's numbers outgrow that, an attempt could take two words on the
  wire, and the worker's domain a pair of tokens.
- **Facts dropped, told.** The worker counts the facts it drops. A
  `Told` could carry how many were dropped before it, as the web's
  deliveries do (engine-domain.md, section 11), so views show gaps.
- **The liveness timings** are starting values, to be tuned against real
  networks.
- **Large payloads.** A snapshot travels in one frame, so a worker's
  output cap holds a slot's worth of them. If snapshots grow to many
  megabytes, a message could be split across frames, or compressed (the
  header's reserved field leaves room). Neither is needed yet.
- **Both directions at once.** The grant of room in 10.1 retires once
  skein's streams take room asked for while a read is outstanding.
- **A worker's name in the engine's domain.** The fleet knows workers
  only by their channels. Views and operators may want the name: it
  would cross as a field of the hello, which the domain then carries.
