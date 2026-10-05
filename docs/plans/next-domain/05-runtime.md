# Step 05: the runtime extended

Provisional, 2026-10-04. What the worker, the agent, the fleet and the
channel gain for the next design (`worker.md`, section 11; `agent.md`,
section 11; the design's README, section 5, on `channel.md` and
`llm.md`): turns kept until acknowledged, transcripts, spend priced in
the deployment's unit, merges in progress, expected heads, messages and
waiting, the engine's tools, result contracts, a second payload version.
All of it extends what is built (README.md, 3.2): the legacy engine never
asks for any of it, so it, and the system worlds that run it, keep
passing unchanged. What goes (snapshots, the first payloads, base
branches made by the worker) goes in step 08. Overview and conventions:
README.md.

## 1. Why this is extension, not replacement

- **Runs are already named by tokens** in the fleet, the worker and the
  channel (`run: u64, attempt: u64` on the wire). Only the engine's
  protocol packs an item into them (`temper-engine-protocol`, `names.rs`),
  and that goes with the legacy root at the cutover. The fleet keys an
  attempt by its run and attempt together, and the worker's host by its
  run, so a run named by its task number and an attempt by its attempt
  number need no change here.
- **What the design adds is triggered by the engine.** A worker sends
  turns, and an agent tells them, waits, parks, calls the engine's tools,
  only for a run whose charter is of the second payload version. The
  legacy engine speaks the first, so none of it reaches it.
- **The second payload version beside the first** is what the channel
  was built for: "a version covers the frames and the payload schemas",
  and the range in `Open` lets a release speak two at once
  (`channel.md`, 4.5). The engine learns a worker's version at the hello;
  workers and agents speak both until step 08.

## 2. By crate

### 2.1 The fleet, `temper-engine-domain-fleet`

| Adds | Shape | The legacy root |
|---|---|---|
| turns, kept by the worker until acknowledged, fenced like answers | `Event::Turn { channel, run, attempt, turn, body }`; `Event::TurnKept { run, attempt, turn }`; `Request::Turned { .. }`, `Request::AcknowledgeTurn { channel, run, attempt, turn }` | an ignore arm for `Turned`: a worker could send one |
| the worker's declared graces at its hello, refused unless strictly shorter than the engine's (`engine.md`, section 8) | `Hello { graces: Option<Duration>, .. }`; `None` checks nothing | builds `graces: None` |
| committed turn prefix restored with adoption | `Adopt { kept: u32, .. }`, zero before the first turn | builds `kept: 0` |
| bounded turn admission and commitment pressure | `Limits::turns`; `Event::TurnBusy { run, attempt, turn }`; `Request::TurnBusy { channel, run, attempt, turn }` | turn limit zero; ignore request arms |

```rust
// fleet, boundary.rs: added
pub enum Event {
    // ...
    /// From a worker: the `turn`th turn of the run's attempt, which it keeps
    /// until acknowledged (domain/worker.md, section 8). Passed up once from
    /// the live attempt; a copy of one the parent has kept is acknowledged
    /// again; one from a fenced attempt is acknowledged and dropped.
    Turn { channel: Token, run: Token, attempt: Token, turn: u32, body: Token },
    /// The parent committed the turn: acknowledge it.
    TurnKept { run: Token, attempt: Token, turn: u32 },
}
```

`graces` is the worker's declared **total stop bound**, not the separate
component deadlines: its contact grace plus the longer of cancel's grace
and a push's deadline, then a save's commit and push (engine.md, section
8). A declaration equal to the engine's grace is refused as well: the
engine's grace must be strictly longer. The component deadlines remain
the worker's to configure and sum.

Turns start at one. `Adopt::kept` restores the durable contiguous prefix
atomically with the claim, before a stray's held turns are released.
Commitments reach the fleet in order; a turn already in that prefix is
acknowledged again. A duplicate pending commitment is dropped without
acknowledgement. Stray turns wait, bounded by `Limits::turns`, for the
parent to adopt them. A full admission sends `TurnBusy` and drops that
copy's body; the worker keeps its original and retries after a backoff.
The parent can also answer an admitted `Turned` with `TurnBusy` when its
commitment cannot proceed, releasing the admission for that retry.
Every body token is handed to the parent once, by `Turned` or `Drop`;
a handed admission retains only its names. Adoption and cleanup release
one held body per resume, so acknowledgement or busy plus drop needs
only two output slots and the fleet's existing `max_out` bound suffices.

### 2.2 The worker

`temper-worker-domain-checkout`:

- **a merge in progress** as a starting point: `Start::Merge { branch,
  base }`, the base merged into the branch with the conflicts left in the
  tree, their files listed for the agent's start (`worker.md`, 4.1);
- git: `Op::Merge { at, theirs }`, ending `Merged` or `Conflicted {
  files }`; `Op::Commit` gains `merging: Option<Commit>`, the second
  parent, and is never skipped as unchanged when it has one; `Op::Push`
  gains `expected: Option<Commit>`, the head the branch must be at, `None`
  as today's fast-forward (`worker.md`, section 5).

`temper-worker-domain-host` and `temper-worker-domain`:

- `Assignment` gains `transcript: Option<Box<[u8]>>`, beside `snapshot`,
  carrying the transcript and the calls committed after its last turn,
  opaque, to the agent's start;
- turns: from the agent child up the link, kept until acknowledged and
  sent again after every hello, on the same footing as answers; a run with
  too many unacknowledged turns waits (`worker.md`, section 8);
- spend in each turn and in the answer;
- a change's title and body kept apart: the agent's push asks with both,
  where today it joins them into one message the worker commits as the
  title (the checkout's `Op::Commit` already takes them apart);
- the worker's graces and push deadline at its hello;
- pushing mid-run, to the run's own branch, under its writer slot (a
  chat's small fix, `core.md`, 6.3), beside pushing at `finish`.

`temper-worker-domain-agent`: the agent's turns and its parking passed up;
the conflicted files in the agent's start.

### 2.3 The agent

`temper-agent-domain-session`:

- **turns told:** each turn, as it ends, encoded with a version and its
  names resolved (no ticket meaningful only inside the session), the
  providers' opaque blocks kept verbatim with the dialect and endpoint
  they came from;
- **opening from a transcript;** one it cannot use is a typed refusal,
  which the run reports as transient (`engine.md`, 7.2);
- **pricing:** each completion priced from the charter's prices, in the
  deployment's unit, its sub-agents' included. The token caps per kind
  stay, as limits.

`temper-agent-domain-tools` and `temper-agent-domain-run`:

- **messages** delivered to the LLM, named; the last one read reported
  with each turn;
- **`wait`**, holding the slot, and parking past the charter's threshold
  (a coordinator's is zero), every turn told first;
- **the engine's tools** (`engine.md`, 7.3) and the connectors' reads,
  offered by the charter's tool families, relayed, their answers decoded
  and given back; a call withdrawn, lost or answered busy asked again
  with the same name after a backoff, while the run lives;
- **the result contract:** `OutcomeSpec` gains `report:
  Option<ReportSpec>` and `failure: bool` beside `change` and `verdicts`;
  `finish` checks a report and a declared failure as it checks the others;
- **a merge in progress:** its conflicted files in the run's start, and a
  push refused for a marker left, naming the file;
- **the budget** in the deployment's unit with the models' prices, beside
  today's token budget; a run stops each session's next completion once
  it has spent it (`authority.md`, section 7).

### 2.4 The channel, `temper-channel`

- **The link, extended in place by kinds.** New message kinds beside
  today's: a turn up (`run`, `attempt`, `turn`, `spent`, `read`, the
  turn's payload) and its acknowledgement down; the graces and push
  deadline in the hello; for each branch a run may push to, the head it is
  expected at. A peer that does not know a kind refuses it with a status
  and skips its body (programming-model.md, section 8); none is sent to a
  peer of the first version.
- **The payloads, a second version beside the first.** `payload.rs`
  becomes `payload/v1.rs` (a move) and `payload/v2.rs` is new, chosen by
  the version agreed at `Open`:

```rust
// temper-channel/src/payload/v2.rs (sketch): the payloads of the second version

/// A charter (channel.md, 7.1, as the design's README, section 5, revises it).
pub struct Charter {
    pub instructions: Box<[u8]>,
    pub brief: Box<[Section]>,
    /// The tool families the task's authority gives, the engine's among them.
    pub tools: Tools,
    pub contract: Contract,
    /// What the run may spend, in the deployment's unit, and its models' prices.
    pub budget: Budget,
    pub models: Box<[Model]>,
    /// How long an idle run waits before it parks.
    pub waiting: Duration,
}

pub enum Contract {
    Report { most: u32 },
    Verdict { verdicts: Box<[VerdictRule]> },
    Change { checks: bool },
    // every contract may also end in a failure, with its reason
}

/// What a run hears (tasks.md, 7.1), carried whole and named.
pub enum Message {
    Result { from: u64, ending: Ending, result: Box<[u8]> },
    Question { from: u64, words: Box<[u8]> },
    Answer { words: Box<[u8]> },
    Decision { proposal: u64, accepted: bool, reason: Box<[u8]> },
    Amendment { amendment: Box<[u8]> },
    Words { from: Party, words: Box<[u8]> },
    News { topic: Box<[u8]>, class: Class, news: Box<[u8]> },
    Notice { notice: Box<[u8]> },
    Timer { at: u64 },
    Waiting { proposal: u64, summary: Box<[u8]> },
}

/// A relayed call: one of the engine's tools, or a connector's read.
pub enum Call {
    Delegate { batch: Box<[NewTask]> },
    Message { to: u64, words: Box<[u8]>, question: bool },
    Amend { task: u64, amendment: Box<[u8]> },
    Cancel { task: u64, reason: Box<[u8]> },
    Release { task: u64 },
    Decide { waiting: u64, decision: Decision },
    Propose { action: Box<[u8]>, reason: Box<[u8]> },
    Subscribe { topic: Topic },
    Unsubscribe { topic: Topic },
    Effect { connector: u16, effect: Box<[u8]> },
    Note { scope: u32, name: Box<[u8]>, revision: Option<u64>, change: NoteChange },
    Recall { scope: u32, query: Recall },
    Read { connector: u16, read: Box<[u8]> },
}

/// The run's outcome: a change, a verdict, a report or a failure.
pub enum Outcome {
    Change { title: Box<[u8]>, body: Box<[u8]> },
    Verdict { verdict: u32, fields: Box<[u8]>, follow_ups: Box<[NewTask]> },
    Report { text: Box<[u8]> },
    Failure { reason: Box<[u8]> },
}

/// A turn: the session's own vocabulary (llm.md), versioned, its spend, and
/// the last message it read.
pub struct Turn {
    pub version: u16,
    pub body: Box<[u8]>,
    pub spent: u64,
    pub read: Option<u64>,
}
```

The byte fields inside (`effect`, `read`, `amendment`) are themselves
typed at their ends by the connector's or the tasks' schema in the same
module, never left for the domain to parse (programming-model.md,
section 4); they are bytes here only because the worker passes them
through. Their concrete fields are settled by `channel.md`, section 16 and
`temper-channel/src/payload/v2.rs`: amendments, actions, authority and
forge reads/effects/results are closed typed values at the endpoints.
The byte-shaped members in the sketch above describe worker pass-through,
not a domain parser.

### 2.5 The protocol layers

- `temper-worker-protocol`: the new link kinds; payloads still passed
  through as bytes.
- `temper-agent-protocol`: version 2's charter, messages, calls and
  outcomes translated to and from the agent's domain; the tools' schemas,
  decoding and rendering for every engine tool, each connector's reads and
  `wait`; `finish` with the result contract (`llm.md`, as the design's
  README, section 5, revises it).
- `temper-engine-protocol`: nothing in this step. Its translation of the
  second version is written in step 07, against the new root.

### 2.6 The fake checkout, `tests/fake-checkout`

Merges that conflict, conflicted files and markers, commits with two
parents, pushes refused when the branch is not at the expected head.

## 3. Tests

Each crate's worlds gain scenarios; none loses one:

| World | New stories |
|---|---|
| `tests/engine/fleet` | turns fenced, kept and acknowledged across a lost channel and a restart; a worker refused for graces longer than the engine's |
| `tests/worker/checkout` | a merge in progress prepared, its conflicts listed; a merge committed with two parents; a push refused for a moved head |
| `tests/worker/host` | turns kept until acknowledged and sent again after a hello; a run waiting on unacknowledged turns; a transcript passed to the start; a mid-run push |
| `tests/worker/agent` | turns and parking passed up; conflicted files in the start |
| `tests/agent/session` | turns told with names resolved; a session opened from a transcript, and one refused; completions priced |
| `tests/agent/tools`, `tests/agent/run` | messages read and reported; `wait` and parking; each engine tool relayed and answered; a call asked again by name; a report and a failure as results; a merge in progress resolved, a marker refused |
| `tests/channel` | golden frames for every new kind and every version 2 payload; the machine world with both versions; a peer of the first version refusing a new kind |
| `tests/agent/protocol` | version 2 charters, calls and outcomes through bytes |

The system worlds (`tests/agent/domain`, `tests/worker/domain`) do not
change: they run the legacy engine, on the first version, and must keep
passing with the same counts.

## 4. Increments

1. **05a the fleet's turns and graces.** Needed first, by step 06's
   skeleton.
2. **05b the channel:** the link's new kinds; `payload/v1.rs` moved,
   `payload/v2.rs` written, with golden frames. Settled layouts and the
   typed inner schemas are in `channel.md`, section 16. This increment
   adds configured 1–2 negotiation and bounded unknown-kind status/skip;
   legacy callers retain explicit v1 imports and mechanical exhaustive
   match arms. It does not implement runtime domain behavior or v2
   protocol translations (05c–05g).
3. **05c the checkout and the fake checkout:** merges, two parents,
   expected heads. This is split into **05c1**, the fake git foundation,
   and **05c2**, the checkout state machine and parent boundary. 05c is
   complete only after both. 05c1 adds fetched two-parent graphs, a local
   three-way merge with markers, explicit resolved two-parent commits
   (including unchanged trees), and expected-head pushes. The world's
   existing callers keep ordinary one-parent/fast-forward defaults.
   05c2 prepares `Start::Merge`, forwards bounded repository/path
   conflicts, retains the second parent until committed, and carries
   expected heads through successful and ambiguous pushes.
4. **05d the host and the worker's root:** transcripts, turns, spend,
   merges in progress, mid-run pushes, graces.
5. **05e the session:** turns told, opening from a transcript, pricing.
6. **05f the run and the tools:** messages, waiting and parking, the
   engine's tools, contracts, merges in progress, the budget in the unit.
7. **05g the agent's and the worker's protocol translations** of the
   second version.

05c to 05f are independent of each other once 05b has fixed the
payloads' shapes; 05g follows them.

## 5. Done when

- every item of `worker.md`, section 11 ("New") and `agent.md`, section
  11 has a story in its crate's world;
- the system worlds pass unchanged, on the first version;
- nothing of the first version, snapshots or base-branch creation has
  been removed (step 08 does that).
