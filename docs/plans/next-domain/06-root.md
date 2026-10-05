# Step 06: the root

Provisional, 2026-10-04. The engine's new root domain,
`temper-engine-domain`, and its world, `tests/engine/domain` (`engine.md`):
routing between the child domains of steps 01 to 04 and the capabilities
kept from today, one commit per decision with every output held until it
is durable, loads, restart, agents' runs from their tasks, the engine's
tools. It is built beside the legacy root, story by story, starting with a
walking skeleton, and replaces it at the cutover. The brief and the views
are extended here, for what the root needs of them. Overview and
conventions: README.md.

## 1. What it takes from the legacy root

From `temper-legacy-engine-domain`, by copying and adapting:

- **routing and translation:** a parent owning its children's states,
  completing hand-offs within one step, small total functions between
  sibling vocabularies (`route.rs`, `translate.rs`), facts gathered into one
  bounded queue;
- **the brief's gathering:** sections asked for in order, read within one
  deadline, a required one missing sending the run back (`jobs.rs`);
- **runs:** the assignment built from a charter, the workspace and the
  credential grants, pushed again to live attempts as accounts refresh;
  relayed calls answered once; the fleet adopting claims at a restart and
  hearing when the loading is done (`runs.rs`, `serve.rs`, `credentials.rs`);
- **the views' wiring:** watches, deliveries one in flight, traces.

What does not come: the item table, records composed and split, nonces,
side writes and their retries, the restart contract read back from the
forge, hold codes, `people.rs` (step 03 replaces it), the wiki's notes.

## 2. The crate

```
crates/temper-engine-domain/src/
├── lib.rs          the tree (engine.md, section 3), what the root keeps (section 4), re-exports
├── boundary.rs     Event, Request: the store, workers, people, connectors, accounts
├── store.rs        Record, Key, Write, Range: the children's records wrapped, and the root's own
├── decision.rs     one decision: writes gathered; commits in flight; outputs held until durable
├── loads.rs        loads in flight, paged, and what waits on each
├── start.rs        restart: load what is live, adopt, read afresh, settle the outbox, decide
├── runs.rs         a task due to a run: check, prepare, claim, place; turns; answers; graces
├── calls.rs        the engine's tools: checked, decided once by name, committed, answered
├── route.rs        what arrives, through which children (engine.md, section 4's table)
├── translate.rs    small total functions between the children's vocabularies
├── connectors.rs   the closed set: a `match` on each connector, the forge the only one
├── config.rs       charters, connectors, rules, procedures' settings, limits; projects' seeds
├── domain.rs       Domain, step, fire, resume, max_out
├── facts.rs
├── limits.rs       its own, and the sum of its children's worst cases
└── tests.rs
```

Its children: `tasks`, `authority`, `people`, `fleet`, `accounts`,
`brief`, `views`, and the forge connector's top. `notes` joins in
step 08.

## 3. Its vocabulary

```rust
/// protocol -> domain
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The store (domain/engine.md, 5.1 and 5.3): the commit `number`, and
    /// every one before it, is durable; or it failed, and the engine stops.
    Committed { number: u64 },
    Uncommitted { number: u64 },
    /// Terminal for `Load`: a page of rows, and the key to go on from.
    Loaded { owner: Token, rows: Box<[Record]>, next: Option<Key> },
    Unloaded { owner: Token },
    /// Workers, over their channels (domain/worker.md, section 2), runs named
    /// by their task and attempt.
    Hello { channel: Token, hello: Hello },
    Lost { channel: Token },
    Turn { channel: Token, task: u64, attempt: u64, turn: Turn },
    Answer { channel: Token, task: u64, attempt: u64, answer: Answer },
    Relay { channel: Token, task: u64, attempt: u64, call: Token, name: u64, body: Call },
    Bounced { channel: Token, task: u64, attempt: u64, name: u64, bounce: Bounce },
    Told { channel: Token, task: u64, attempt: u64, kind: Kind, content: Box<[u8]> },
    /// People, through the web (domain/people.md, section 11).
    SignedIn { reply_to: ReplyTo, sign_in: u64, identity: Identity },
    Ask { reply_to: ReplyTo, sign_in: u64, key: [u8; 16], ask: Ask },
    Connected { sign_in: u64, connection: Token },
    Disconnected { connection: Token },
    Unwatch { watcher: Token },
    Delivered { watcher: Token, done: bool },
    /// The forge's protocol, passed to its connector.
    Forge { call: Token, cost: u32, result: Result<forge::api::Answer, forge::api::Error> },
    ForgeHint { hint: forge::api::Hint },
    /// LLM accounts, as today (credentials.md, section 5).
    Refreshed { account: u32, generation: u64, valid: Duration },
    RefreshFailed { account: u32, generation: u64, failure: accounts::Failure },
    Rejected { account: u32, generation: u64 },
    Exhausted { account: u32, retry_after: Duration },
}

/// domain -> protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// To the store: apply `writes` whole, after every commit numbered
    /// before it. Ended by one `Committed` or `Uncommitted`.
    Commit { number: u64, writes: Box<[Write]> },
    /// To the store: up to `most` rows of `range` from `after`. Ended by one
    /// `Loaded` or `Unloaded`.
    Load { owner: Token, range: Range, after: Option<Key>, most: u32 },
    /// A commit failed: the shell stops the engine (core.md, section 4).
    Stop,
    Assign { channel: Token, assignment: Assignment },
    Inbound { channel: Token, task: u64, attempt: u64, name: u64, message: Message },
    Cancel { channel: Token, task: u64, attempt: u64 },
    Relayed { channel: Token, task: u64, attempt: u64, call: Token, served: Served },
    Acknowledge { channel: Token, task: u64, attempt: u64 },
    AcknowledgeTurn { channel: Token, task: u64, attempt: u64, turn: u32 },
    Refuse { channel: Token },
    Grant { channel: Token, task: u64, attempt: u64, grant: accounts::Grant },
    Reply { to: ReplyTo, reply: Reply },
    Push { connection: Token, change: people::Change },
    Deliver { watcher: Token, missed: u64, chunks: Box<[views::Chunk]> },
    Ended { watcher: Token, end: views::End },
    Forge { call: Token, forge: u16, repository: u32, op: forge::api::Op },
    Account { request: accounts::Request },
}
```

The worker-facing records keep today's shapes but for their names (a
task's number where an item was) and what the design adds (turns, their
acknowledgements, transcripts, messages and calls of the second payload
version); step 07 writes their translation.

## 4. Decisions, commits and loads

```rust
// decision.rs (sketch)

/// What one entry point decides (engine.md, 5.1): every record the children
/// it routed through saved or erased. Opened as the entry point starts,
/// closed before it returns.
pub(crate) struct Decision {
    writes: List<Write>,
}

/// The commits in flight, and the outputs waiting for them (engine.md, 5.2).
pub(crate) struct Commits {
    /// The last commit numbered, and the last known durable: every commit
    /// up to it is.
    made: u64,
    durable: u64,
    /// Outputs in the order they were made, each released once the commit
    /// it was tagged with is durable.
    held: Queue<Held>,
}

pub(crate) struct Held {
    after: u64,
    request: Request,
}
```

- **Closing a decision.** Writes gathered: one `Commit { number: made + 1
  }`. Every outward request the step made is tagged with `made` as it then
  is, the last commit whose state it may rest on, and queued in `held`; one
  tagged with a commit already durable is released at once.
- **Releasing.** On `Committed { number }`, `durable` moves, and the held
  requests it covers become ready, drained through the root's ready list
  at the start of its next steps within `MAX_OUT`
  (programming-model.md, section 2), never all in the step that heard it.
- **Backpressure.** Past the commits-in-flight limit, a decision that
  would write is not taken: a person's request is answered busy, a run's
  call is answered busy (and asked again), a turn or an answer is answered
  busy (the worker sends it again), a connector keeps its events and
  outcomes (`engine.md`, 5.1). The store's answers are always taken.
- **`Uncommitted`** stops the engine: `Request::Stop`, and nothing held
  is released.
- **Loads** carry an owner token; what waits on one is a state that says
  so (a brief gathering, a run preparing to resume, a person's page);
  their bytes are a limit, and a load past it is cut and says how much.

```rust
// store.rs (sketch): the children's records, wrapped, and the root's own

pub enum Record {
    /// The deployment's id, made at its first start, and its numbers.
    Deployment(Deployment),
    /// The calls committed since a live task's last turn, by attempt and name, with their answers.
    Call(CallRecord),
    /// A transcript's turn: its bytes, its spend, what it read.
    Turn(TurnRecord),
    Tasks(tasks::Stored),
    People(people::Stored),
    Forge(forge::Stored),
    Traces(views::Stored),
}

pub enum Key {
    Deployment,
    Call { task: u64, attempt: u64, name: u64 },
    Turn { task: u64, attempt: u64, turn: u32 },
    Tasks(tasks::Key),
    People(people::Key),
    Forge(forge::Key),
    Traces(views::Key),
}
```

## 5. Restart

`engine.md`, section 6, as a state of the root's own:

```rust
pub(crate) enum Start {
    /// The live ranges, one after another, each child restored from its rows.
    Loading { range: u32, after: Option<Key> },
    /// Claims handed to the fleet; workers say what they host.
    Adopting,
    /// Each connector reads afresh what its system owns of the live work.
    Reading,
    /// Every outbox entry not committed as made is looked for by its key.
    Settling,
    /// Every procedure has stepped once; agent tasks due get runs.
    Running,
}
```

Nothing new is assigned before `Adopting` has handed every claim to the
fleet; nothing decides before `Running`. Workers send again the turns and
answers they were not acknowledged for; the root commits any it does not
have, and acknowledges each.

## 6. Runs and the engine's tools

- **From a task to a run** (`engine.md`, 7.1): `tasks` asks `Activate`;
  the root asks authority (`authority.md`, 8.3), prepares (the brief, and
  the transcript if the charter resumes it), commits the claim with its
  attempt number and the writer slots its workspace writes (the
  connector's `Claim`), and places it through the fleet once the claim is
  durable.
- **Turns** (7.2) are committed with their spend (charged through
  authority's numbers) and the messages they took, then acknowledged; a
  copy of one already committed is acknowledged again and dropped.
- **Answers** (7.4): finished, the result checked against the contract
  and the state now, the task closing; parked, idle; failed, by class.
- **Charters, tools and messages are the root's own types.** smith's
  charter, its host tools and its messages appear only in the engine's
  protocol layer (07a; `agent.md`, sections 4 and 5), so the root's run
  and tool routes do not wait for 05s.
- **Calls** (7.3) are named by attempt and name, checked, decided once,
  committed with their answer, and answered after the commit; one seen
  before is answered from its record:

| Tool | Route through the children | Commits |
|---|---|---|
| `delegate` | authority (`check_batch`) → tasks (`Make`) → the connectors for the resources the batch names | yes |
| `message` | authority (the reference) → tasks (`Message`) | yes |
| `amend`, `cancel`, `release` | authority (a widening fits?) → tasks | yes |
| `decide` | tasks (`Decide`) → authority (`covers`) → tasks | yes |
| `propose` | authority (`needs`) → tasks (`Propose`) | yes |
| `subscribe`, `unsubscribe` | tasks (tasks and timers), or a connector (`Subscribe`) | yes |
| `effect` | authority (`check_effect`, with the connector's facts) → the connector saves the outbox entry | yes |
| a connector's read | authority (`check_call`) → the connector (`Read`) | no |
| `note`, `recall` | answered unavailable until step 08 | no |

## 7. The capabilities, extended

- **The brief** (`temper-engine-domain-brief`) gains the sections of
  `engine.md`, section 9, as new `Kind`s and `Source`s beside today's:
  the task and its lineage, its inbox since its last run, its inputs' and
  dependencies' results, its delegates and their states, its earlier
  attempts, the calls committed since its last turn, the proposals waiting
  for it, a transcript's tail; and from connectors, a conflict with what
  landed since. Today's `Item`, `Comments`, `Dependencies` and `Plan`
  sources stay for the legacy root until step 08. Its world gains the new
  kinds' cuts.
- **The views** (`temper-engine-domain-views`) are already named by
  tokens: the new root watches a task's tree as today's `Subject::Item`,
  a project's goals as `Subject::Board`, and a run's turns as they commit
  as a new fact `Kind`. The names are changed in step 08.
- **Accounts** do not change.
- **Notes** are not a child yet (README.md, 2.2).

## 8. The world

`tests/engine/domain`, package `temper-engine-domain-world`, the root
with every child beneath it (`engine.md`, section 15), against:

- **a fake store** (`src/store.rs`): records ordered by key, commits
  applied in order and answered after a latency, some failing (the engine
  stops), some applied and their answers lost; loads paged by range; it
  outlives the engine's restarts;
- **scripted workers** (`src/workers.rs`, `src/script.rs`), at the root's
  domain face: they say hello with their graces, host runs on charters of
  the second version, play each run's script (turns kept until
  acknowledged, tool calls, waits, parks, pushes through the fake forge's
  git naming the heads they expect, answers kept until acknowledged), lose
  their channels and come back, vanish, or freeze past their grace and
  resume with a push in hand;
- **scripted people on the web** (`src/people.rs`): they sign in, start
  chats, set goals, write, answer, decide, stop and release, watch;
- **the fake forge,** through the forge connector's calls as step 04's
  world translates them, used here as a library as today's engine world
  uses the forge world's translation;
- **restarts,** injected by the referee at drawn moments and at the cuts
  of `engine.md`, section 15: before a commit is durable; after it is,
  before its answer arrives; after an outbox entry went out, before its
  outcome; after its outcome, before it was committed; after a turn or an
  answer is committed, before its acknowledgement arrives; after a push
  landed, before the worker reports it.

Its referee holds the engine to `core.md`, section 10, seen from outside:
authority holds; keyed effects made once across restarts; nothing a
worker, a person or the forge saw rested on a commit that was lost;
dependencies first, no task waiting on itself; one run per task; one
writer per resource on workers that keep their deadlines, and no push
over an unexpected head on any; merges land exactly the heads decided;
results and people's words arrive; nothing written over; every story ends
within a bound; every call decided once; budgets never counted twice.

Its stories are `engine.md`, section 15's, but the note written, corrected
and recalled (step 08). The system worlds keep the legacy root until
step 07.

## 9. Increments

1. **06a the walking skeleton.** The root with `tasks` (02a, 02b, and
   the finite funding/atomic charge seam of 02e),
   `authority` (01a to 01c), `people` (03a), the fleet (05a), the brief and
   accounts; the fake store, one scripted worker, one person. One story:
   a person signs in and starts a chat; the task is made, due, its brief
   gathered (the task's section only), its claim committed, its run
   assigned; two turns committed and acknowledged; it finishes; its answer
   committed and acknowledged; its result reaches the person. One restart,
   after a commit is durable and before its answer reaches the engine,
   making nothing twice. This is where the design's gaps show first: each
   one found is settled in `domain/engine.md` in the same branch.
   Its brief seam adds `Source::Task { task }`, `Kind::Task` and a separate
   task budget; task text precedes requester lineage, losing farthest
   ancestors first. The existing brief world's seeds retain their source
   distribution; dedicated task worlds add replay, facts and randomized
   bounds/terminal-race coverage. This seam alone is not the walking skeleton.
   The root's journal is the next seam: a deployment header owns fresh
   numbers, every writing decision saves that header in its one commit,
   deliveries carry the last commit even when their decision wrote nothing,
   cumulative store answers only make deliveries ready, and the ready pass
   releases one at a time. Pressure is checked before routing a child; a
   failed commit stops all subsequent releases. A first start commits its
   deployment identity; a restored header is already durable. The initial
   root world has an atomic ordered fake store, an independent commit and
   delivery referee with negative cases, deterministic lag/replay scenarios,
   a lost completion recovery cut and maximum held-result memory checks.
   Its small randomized sweep varies writing/read decisions, fresh number
   gaps, storage latency and ready draining. These journal tests cover the
   barrier cells; child record wrappers, loads and the complete walking
   story are still required before 06a is complete.
   Integration exposed a prerequisite missing from the original sketch:
   02a/b carry number snapshots but do not keep the external funding ledger.
   The charged story must use 02e's finite person pool/project period,
   atomic carving and charged turn/answer admission. A separate precharge
   followed by a refusing turn would commit an expense for an unaccepted
   event. Begin brief, journal and load work beside 02c; bring the funding
   seam forward before running the complete story. The root never keeps a
   second copy of the child's funding ledger.
   The next foundation adds fenced one-terminal loads for the root-owned
   deployment and turn ranges: bounded row/order/cursor validation,
   whole-prefix byte cuts with exact omission reports, abandonment retaining
   IO capacity until the terminal, and key-range paging in the independent
   fake store. Child ranges and actual startup/brief routing still join in
   the walking story; its dispatch must wait for the commits a load reads.
2. **06b runs.** Parking and resuming from a transcript, or fresh past the
   resume limit with the tail in the brief; messages relayed only once
   committed; cancels; failures by class, backoff, holds; saved work;
   graces; a worker vanishing, the next attempt resuming at the last
   committed turn and told of the calls committed after it; a worker
   frozen past its grace.
3. **06c the engine's tools** (section 6), calls by name and decided once,
   busy under backpressure; `tasks` 02c to 02e and `authority` 01d in place.
4. **06d people,** complete (03b, 03c): inboxes, person tasks, proposals
   reaching people, stops and releases, watches through the views.
5. **06e the forge connector** (step 04): adoption at runtime; resources,
   holds and slots at claims; effects through the outbox; change tasks to
   landing, the queue, updates and resolutions from a merge in progress;
   goals' issues and landings as news; drift held and explained; the
   connector's brief sections and reads.
6. **06f restarts at every cut, the fuzzy sweep, memory at the worst
   case,** and the budgets checked with both engines' worlds in the
   suites.

## 10. Done when

- every story of `engine.md`, section 15, but the note's, is a focused
  test, and every cut has a restart story;
- the referee's every rule fails a test written to break it;
- the fuzzy sweep reaches every ending, within its allotment, with the
  legacy worlds still running;
- the table of step 07, section 2, maps every legacy story to a story
  here, to one in a child's world, or to a reason it goes.
