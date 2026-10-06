# The client domain

Provisional, 2026-10-05. `temper-web-domain`: what a person's browser
knows, and what the web does with it (`architecture.md`, section 3). How
the crate is organised, its vocabulary, its state, its entry points, and
each of its parts: pages, objects, keyed requests, streams and the
link's state, windows, drafts, session storage. Each sketch is the shape
the code takes; each variant is marked with the increment that adds it
(04-increments.md), since nothing is added before it is used. Overview:
README.md.

## 1. In one page

- **A step machine like every temper domain** (`programming-model.md`,
  section 3): `step` for events, `fire` for its deadlines, `max_out`,
  `worst_case`. `#![no_std]`, `skein-lib` its only dependency.
- **One page at a time, its objects once.** The page at the address holds
  its own state (filters, folds, windows) and names the objects it shows
  by handle; the objects are a working set held once, so a decision
  changes every place it shows (`ux/README.md`, 5.2). Leaving a page
  closes its streams and clears its working set.
- **Requests outlive pages.** A keyed request is pending until its answer
  says it is durable, kept in session storage before it is sent, sent
  again with the same key after a drop, a busy answer or a reload, and
  shown as asked until then (`ux/README.md`, 5.3).
- **Streams decide the link's state.** A page's watches each start with a
  snapshot (`domain/engine.md`, section 11); a watch told it missed
  deliveries reopens (behind); a watch that drops, or a request that
  cannot reach the engine, makes the client offline until a reconnect
  succeeds, with backoffs the domain draws from its seed.
- **What the person is in the middle of is state.** Drafts, the open
  confirmation and its fields, folds and filters are the domain's, so the
  view keeps none and worlds see all of it.
- **Facts, not words.** The domain carries what the engine said (a
  phase, a hold's reason, amounts, wall times) as typed values; words are
  the view's.

## 2. The crate

```
crates/temper-web-domain/src/
├── lib.rs          its doc: what it keeps, what it never knows, entry points; re-exports
├── boundary.rs     Event, Request, StreamEvent, StreamEnd, Answer, ReadResult: what crosses to the shell and the protocol
├── action.rs       Action: what the person did, decoded by the view; FieldRef, Intent
├── ask.rs          Ask, Outcome, Refusal: keyed requests (domain/people.md, 5.1), the client's own vocabulary
├── address.rs      Address, Section: where the person is, typed
├── domain.rs       Domain; step, fire, max_out; the read API the view uses
├── frame.rs        what every page shows: who is signed in, the project, the counts, the link
├── link.rs         the link's state: starting, live, behind, offline; reconnect backoff
├── streams.rs      watches: opening, live, reopening, closing; heartbeats; snapshots and changes routed to their owner
├── reads.rs        snapshots and pages read, each ended by one terminal; abandoned when their page goes
├── requests.rs     keyed requests: minted, saved, sent, retried, parked, answered, restored; two in sequence
├── objects.rs      the working set: objects by key, their sources, cards' states, leaving
├── object/         one file per kind of object a card shows
│   ├── chip.rs        a task named in passing: number, title, executor, phase
│   ├── proposal.rs    who proposes what, why, what it needs, who would fund it, where it has been
│   ├── escalation.rs  the held task, why, what it had done, where it waits
│   ├── question.rs    which task asks, its words
│   ├── person.rs      a person task: what is asked, the results it depends on, its contract
│   └── result.rs      a task ended: how, its result by its contract
├── pages.rs        Page: one variant per page; opening and leaving a page
├── pages/          one file per page, its state and its handlers
│   ├── signin.rs  inbox.rs  chats.rs  task.rs  board.rs
├── task/           the task page's parts
│   ├── transcript.rs  runs, turns, the provisional turn, folds
│   ├── tree.rs        the plan: nodes, folds, counts per phase, held below
│   └── steer.rs       which requests fit the task's phase (ux/tasks.md, section 8)
├── forge.rs        the forge connector's panels and the changes page (section 9)
├── window.rs       a window onto a paged list: rows, cursors, loading, sliding
├── drafts.rs       fields and their text; the confirmation and its fields
├── notices.rs      notices with their deadlines
├── saved.rs        Saved: what session storage keeps
├── facts.rs        content-free facts
├── limits.rs       Limits, worst_case
└── tests.rs        step tests, with a module per part under tests/
```

`mod_module_files` is denied, so a directory sits beside its file
(`pages.rs` and `pages/`), as elsewhere in the workspace. W1 lands
`boundary`, `action`, `ask`, `address`, `domain`, `frame`, `link`,
`streams`, `reads`, `requests`, `drafts`, `notices`, `saved`, `facts`,
`limits` and `pages` with the sign-in and chats pages; each later
increment adds the files of its pages and objects.

## 3. Its vocabulary

### 3.1 Events and requests

```rust
/// shell, view or protocol -> the client domain
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The page loaded: where the browser is, what session storage kept, and
    /// the person's offset from UTC, for showing wall times (architecture.md,
    /// section 6). The first event; nothing else is taken before it.
    Start { address: Address, saved: Option<Saved>, offset: Offset },          // W1
    /// The browser went somewhere itself: back, forward.
    Went { address: Address },                                                  // W1
    /// The person did something, decoded by the view from the node acted on.
    Act { action: Action },                                                     // W1
    /// Terminal for `Send`: the engine's answer, or why there is none.
    Answered { request: Token, answer: Answer },                                // W1
    /// Terminal for `Read`.
    Read { read: Token, result: ReadResult },                                   // W1
    /// For `Open`: the stream is open; events follow, its snapshot first.
    Opened { stream: Token },                                                   // W1
    Streamed { stream: Token, event: StreamEvent },                             // W1
    /// Terminal for `Open`, whoever ended it.
    Ended { stream: Token, end: StreamEnd },                                    // W1
}

/// the client domain -> the shell and the protocol
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// A keyed request, sent again with the same key until it is answered
    /// durably. Ended by exactly one `Answered`.
    Send { request: Token, key: Key, ask: Ask },                                // W1
    /// A snapshot or a page of a list. Ended by exactly one `Read`.
    Read { read: Token, query: Query },                                         // W1
    /// Follow a watch. Ended by exactly one `Ended`.
    Open { stream: Token, watch: Watch },                                       // W1
    /// Stop following; its `Ended` follows.
    Close { stream: Token },                                                    // W1
    /// Show this address, as a new entry in the history or in place of the
    /// current one.
    Address { address: Address, push: bool },                                   // W1
    /// Keep this in session storage, whole, in place of what it kept.
    /// Applied before any request after it in the same step, so a request is
    /// kept before it is sent (section 6.3).
    Save { saved: Saved },                                                      // W1
    /// Go to the engine's sign-in, which comes back to `then` (domain/people.md,
    /// section 3). The client holds no token.
    SignIn { then: Address },                                                   // W1
}
```

- **No render request.** The view reads the domain after each step; the
  domain counts each change to what is shown (`Domain::shown`), and the
  view builds again only when the count moved (02-view.md, section 6).
- **Tokens are handles.** `request`, `read` and `stream` are the tokens
  of `Id<Pending>`, `Id<ReadSlot>` and `Id<Stream>`
  (`programming-model.md`, 4.2). A terminal is never stale, since a slot
  is retired only after its terminal arrived; a stale one is an assertion.

### 3.2 Actions

What the person did, as the view decodes it from the node acted on and,
for an input, the value the shell read (`architecture.md`, section 4):

```rust
#[derive(PartialEq, Eq, Debug)]
pub enum Action {
    /// A link: a chip, the navigation, going to a task by its number.
    Go { address: Address },                                                    // W1
    /// An input's value changed; the person's, not written back (section 8).
    Edit { field: FieldRef, text: Box<[u8]> },                                  // W1
    /// A form's number, decoded by the view from its digits; `None` when
    /// what was typed is not a number.
    Number { field: FieldRef, value: Option<u64> },                             // W9
    /// Enter in a composer, or a form's submit.
    Submit { form: Form },                                                      // W1
    /// A card's action pressed: opens its confirmation, which says what it
    /// will do before it is sent (ux/README.md, 5.3). `object` is the card's
    /// handle in the working set; a handle whose object has gone is refused
    /// as stale.
    Intend { intent: Intent, object: Id<Object> },                              // W4
    /// A choice in the open confirmation: an option of a person task, what
    /// becomes of a delegate on closing.
    Choose { choice: Choice },                                                  // W6
    Confirm,                                                                    // W4
    Dismiss,                                                                    // W4
    /// A fold opened or closed: a tool call, a plan's branch or level, a card.
    Fold { fold: Fold, open: bool },                                            // W7
    Filter { filter: Filter },                                                  // W6
    /// The next page of a window, older or newer.
    More { window: WindowRef, older: bool },                                    // W6
    SignIn,                                                                     // W1
}

/// What a card's button would do, before the confirmation says it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    Release,                                                                    // W4
    LeaveHeld,                                                                  // W4
    Accept, Reject, PassUp,                                                     // W6
    Answer,                                                                     // W6
    Stop, Close, Cancel,                                                        // W7
    Amend, TakeOver, AmendThenRelease,                                          // W8
    Raise, Lower,                                                               // W9
}
```

### 3.3 Keyed requests

The client's vocabulary for `domain/people.md`, 5.1, its own types: the
joint world translates them to `temper-engine-domain-people`'s, and the
protocol layer, later, to the wire's.

```rust
/// 128 bits drawn from the domain's seed; the engine makes a request once
/// per key, per person (domain/people.md, 5.1.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Key(pub [u8; 16]);

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Ask {
    StartChat { project: u32, words: Box<[u8]> },                               // W1
    /// An escalation or a proposal, at the revision the card showed.
    Decide { waiting: Waiting, revision: u64, decision: Decision },             // W4, proposals W6
    Answer { task: u64, revision: u64, answer: PersonAnswer },                  // W6
    Read { position: Position },                                                // W6
    Write { task: u64, words: Box<[u8]> },                                      // W7
    Stop { task: u64 },                                                         // W7
    Release { task: u64, revision: u64 },                                       // W7
    Close { task: u64, delegates: Box<[Disposal]> },                            // W7
    Cancel { task: u64 },                                                       // W7
    Amend { task: u64, revision: u64, amendment: Amendment },                   // W8
    TakeOver { task: u64 },                                                     // W8
    SetGoal { project: u32, goal: GoalForm },                                   // W9
    Prioritise { project: u32, goals: Box<[u64]> },                             // W9
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Answer {
    /// Durable: the engine committed it, or committed its refusal.
    Done(Outcome),
    Refused(Refusal),
    /// Not made and not kept (domain/people.md, 5.1.1): sent again, same key.
    Busy,
    /// The sign-in is gone: kept, and sent again once signed in.
    SignedOut,
    /// No answer reached the client: the engine may or may not have it.
    /// Sent again with the same key once the engine is reachable.
    Unreachable,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Started { task: u64 },                                                      // W1
    /// Decided as asked.
    Decided { choice: Choice },                                                 // W4
    /// Someone decided first (domain/people.md, 5.1): by whom, how, when.
    DecidedBefore { by: Person, choice: Choice, at: Wall },                     // W4
    Written { task: u64 },                                                      // W7
    Made,                                                                       // W7
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    Role, Limit, Ended, Unknown,                                                // W1
    /// The engine has this key for another ask. The client mints every key
    /// fresh and never reuses one, so this is a bug on one side: shown as a
    /// refusal, counted as a fact, and failed on by the client's world.
    KeyConflict,                                                                // W1
    /// Beyond the person's authority; `proposable` when someone could accept
    /// it, and the card offers to propose it instead (ux/README.md, 5.3).
    Authority { lacks: Lack, proposable: bool },                                // W4
    /// The state the person acted on has changed since (ux/inbox.md, section 5).
    Moved { revision: u64 },                                                    // W4
    Standing, NoFurther, NeedsAmend,                                            // W4
    Full,                                                                       // W7
}
```

### 3.4 Reads and watches

```rust
/// A snapshot or a page; the history and the lists the engine does not hold live.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Query {
    Chats { project: Option<u32>, live: bool, after: Option<Cursor> },          // W1
    Escalation { task: u64 },                                                   // W4
    Result { task: u64 },                                                       // W4
    Inbox { group: Group, filter: InboxFilter, after: Option<Cursor> },         // W6
    Transcript { task: u64, before: Option<Cursor> },                           // W7
    History { task: u64, subtree: bool, before: Option<Cursor> },               // W8
    Ended { task: u64, under: u64, after: Option<Cursor> },                     // W8
    Landings { project: u32, branch: u32, before: Option<Cursor> },             // W10
}

/// A live view, starting from a snapshot (domain/engine.md, section 11).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Watch {
    Person,                         // the frame: who, their projects, the counts      W1
    Task { number: u64 },           // a task's header, overview, budget, its card     W4
    Inbox,                          // the person's inbox, across projects             W6
    Run { task: u64 },              // its live run: streaming text, tool calls, turns W7
    Tree { task: u64 },             // its open subtree's phases                       W8
    Goals { project: u32 },         // the board                                       W9
    Changes { project: u32 },       // the forge's changes and queues                  W10
}

#[derive(PartialEq, Eq, Debug)]
pub enum StreamEvent {
    Snapshot(Snapshot),             // first, and again after a reopen
    Change(Change),
    /// Deliveries were dropped for a slow watcher (domain/engine.md, section
    /// 11): the page is behind, and the watch reopens for a fresh snapshot.
    Missed { count: u64 },
    /// Nothing changed; the stream is alive. Re-arms its heartbeat.
    Alive,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StreamEnd {
    Closed,                         // the client asked
    Dropped,                        // the connection went: reconnect after a backoff
    Refused(Refusal),               // the engine refused the watch: unknown, full
    SignedOut,
    Gone,                           // its subject ended and was dropped from the engine
}
```

`Snapshot` and `Change` have a variant per watch, typed all the way
down: a task's snapshot carries its chip, header, overview and the
objects its cards show; a change says which object changed and to what,
or that it left and why (`Change::Left { key, why }`, where `why` says
decided, by whom and how; withdrawn; ended). The protocol layer decodes
them in full (`programming-model.md`, section 4: nothing reaches the
domain as text to decode later).

## 4. Its state

```rust
/// What a person's browser knows (architecture.md, section 3). Opaque; the
/// view reads it through the methods of section 5.
#[derive(Debug)]
pub struct Domain {
    phase: Phase,                      // Starting, SigningIn, Ready
    rng: Rng,                          // request keys; backoffs' jitter
    wall: Wall,                        // the last step's wall time, for "waiting 4 min"
    offset: Offset,
    shown: u64,                        // counts changes to what is shown
    frame: Frame,                      // who is signed in, their projects, the project chosen, the counts
    link: Link,                        // starting, live, behind, offline; the reconnect's backoff
    page: Page,                        // the page at the address, with its own state
    generation: u32,                   // of the page: what a page asked for names it
    objects: Objects,                  // the page's working set (section 6.2)
    streams: Slab<Stream>,             // each ended by one Ended
    reads: Slab<ReadSlot>,             // each ended by one Read
    requests: Slab<Pending>,           // keyed requests until durable (section 6.3)
    intents: Map<IntentKey, Id<Pending>>,  // a second press joins the first
    confirming: Option<Confirming>,    // the one confirmation open
    drafts: Drafts,                    // fields by reference, bounded
    notices: Queue<Notice>,
    timers: Deadlines<Timer>,
    save: SaveState,                   // clean, or dirty with a deadline
    facts: Queue<Fact>,
    lost: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Timer {
    Retry(Id<Pending>),                // a busy or unreachable request, sent again
    Reopen(Id<Stream>),                // a dropped watch, opened again
    Silent(Id<Stream>),                // no event within the heartbeat: treated as dropped
    Linger(Id<Object>),                // a decided card, leaving
    Notice,                            // the oldest notice expires
    Save,                              // drafts written, coalesced
    Tick,                              // relative times shown move on: "waiting 5 min"
}
```

What the view reads is plain data (`Frame`, `Page`, `Object`, `Field`,
`Confirming`, `Notice`): pub fields, reached only through shared
borrows. `Domain` itself is opaque (`partial_pub_fields`).

## 5. Entry points

```rust
#[must_use]
pub fn max_out(limits: &Limits) -> u32;

/// Apply one event. The caller reserves `max_out` free slots in `out`.
pub fn step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut Queue<Request>) {
    domain.wall = env.wall;
    match event {
        Event::Start { address, saved, offset } => start(domain, env, address, saved, offset, out),
        Event::Went { address } => pages::go(domain, env, address, Push::No, out),
        Event::Act { action } => act(domain, env, action, out),
        Event::Answered { request, answer } => {
            requests::answered(domain, env, Id::<Pending>::from_token(request), answer, out);
        }
        Event::Read { read, result } => reads::read(domain, env, Id::<ReadSlot>::from_token(read), result, out),
        Event::Opened { stream } => streams::opened(domain, env, Id::<Stream>::from_token(stream), out),
        Event::Streamed { stream, event } => {
            streams::streamed(domain, env, Id::<Stream>::from_token(stream), event, out);
        }
        Event::Ended { stream, end } => streams::ended(domain, env, Id::<Stream>::from_token(stream), end, out),
    }
}

/// Fire at most one expired timer; the caller calls again while one is due.
pub fn fire(domain: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>);

impl Domain {
    pub fn new(limits: &Limits, seed: u64) -> Domain;
    pub fn next_deadline(&self) -> Option<Time>;
    pub fn reclaim(&mut self);                     // retired slots, at the iteration's end
    pub fn pop_fact(&mut self) -> Option<Fact>;

    // What the view reads (02-view.md, section 4).
    pub fn shown(&self) -> u64;
    pub fn wall(&self) -> Wall;
    pub fn offset(&self) -> Offset;
    pub fn link(&self) -> LinkState;
    pub fn frame(&self) -> &Frame;
    pub fn page(&self) -> &Page;
    pub fn object(&self, id: Id<Object>) -> Option<&Object>;
    pub fn field(&self, field: FieldRef) -> Option<&Field>;
    pub fn confirming(&self) -> Option<&Confirming>;
    pub fn notices(&self) -> &Queue<Notice>;
}

#[must_use]
pub fn worst_case(limits: &Limits) -> Option<u64>;
```

`act` routes an action to the part that owns it: `Go` to `pages`,
`Edit` and `Number` to `drafts`, `Intend`, `Choose`, `Confirm` and
`Dismiss` to the confirmation, `Submit` to the page's form, `Fold`,
`Filter` and `More` to the page. Every handler is a free function over
the fields it touches (`programming-model.md`, 5.1).

## 6. Its parts

### 6.1 Pages

```rust
/// The page at the address (ux/README.md, section 4), with its own state.
#[derive(Debug)]
pub enum Page {
    Starting,                                                                   // W1
    SignIn { then: Address },                                                   // W1
    /// An address the client has no page for yet, or a task number unknown.
    Missing { address: Address },                                               // W1
    Chats(chats::Chats),                                                        // W1
    /// The universal page (ux/tasks.md): a chat leads with its conversation,
    /// a goal with its plan, any other task with its activity.
    Task(task::TaskPage),                                                       // W4
    Inbox(inbox::Inbox),                                                        // W6
    Board(board::Board),                                                        // W9
    Changes(forge::Changes),                                                    // W10
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Address {
    Inbox,                                                                      // W1, home
    Chats,                                                                      // W1
    Task { number: u64, section: Option<Section> },                             // W4
    Board { project: u32 },                                                     // W9
    Changes { project: u32 },                                                   // W10
}
```

- **Going to a page** bumps the generation, closes the old page's watches,
  marks its reads abandoned (their terminals are dropped when they come),
  clears the working set, then opens the new page's watches and reads,
  and asks for the address to be shown. Pending requests, the frame and
  drafts carry over.
- **A page's state is its own:** a task page's lead, folds, filters and
  windows; the inbox's filters and two windows; the chats' filter and its
  composer. Each page file has its `open`, its handlers for its actions,
  and its handlers for its watches' snapshots and changes.
- **Home is the inbox** (`ux/README.md`, section 4). Until W6 builds it,
  `Address::Inbox` shows the chats.

### 6.2 Objects

```rust
/// How the engine names what a card shows.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ObjectKey {
    Task(u64),                                                                  // W4: chips, person tasks, results
    Escalation { task: u64 },                                                   // W4
    Proposal { task: u64, number: u32 },                                        // W6
    Question { task: u64, message: u64 },                                       // W6
    Change(u64),                                                                // W10
}

#[derive(Debug)]
pub struct Object {
    pub key: ObjectKey,
    pub revision: u64,           // the state shown; an action names it
    pub body: Body,
    pub card: Card,
    pub sources: Sources,        // which of the page's watches and windows name it
}

#[derive(Debug)]
pub enum Body {
    Chip(Chip),                                                                 // W4
    Escalation(Escalation),                                                     // W4
    Ended(TaskResult),                                                          // W4
    Proposal(Proposal),                                                         // W6
    Question(Question),                                                         // W6
    PersonTask(PersonTask),                                                     // W6
    Change(forge::Change),                                                      // W10
}

/// Where a card stands with the person (ux/inbox.md, section 5).
#[derive(Debug)]
pub enum Card {
    Open,
    /// A request about it is pending: shown as being decided, never as done.
    Deciding { request: Id<Pending> },
    /// Refused: says why, and stays.
    Refused { refusal: Refusal },
    /// Decided, withdrawn or ended, by the person or someone else: says
    /// what happened, by whom and when, then leaves at `until`.
    Leaving { why: Why, until: Time },
}

pub struct Objects {
    slab: Slab<Object>,
    index: Map<ObjectKey, Id<Object>>,
}
```

- **Sources, not reference counts.** Each object records which of the
  page's watches and windows name it, in a small bit set. A snapshot
  clears its source's bit on every object and sets it on those it names;
  an object no source names goes, unless it is deciding or leaving. One
  object can show in several places of one page (an escalation on a task
  page's overview and in its plan) and is still held once.
- **The answer and the stream, in either order.** A decided card leaves
  through whichever arrives first: the answer (`Decided`: "you accepted
  it"; `DecidedBefore`: "Pat released it at 10:14") or the watch's
  `Change::Left` with its reason. While a request about it is pending, a
  `Left` keeps the card deciding until the answer, so the person learns
  how their own request ended.
- **Revisions.** A card's action names the revision it was shown at. A
  change while its confirmation is open marks the confirmation changed
  (section 6.5), and confirming it then is refused in place, saying what
  changed, rather than sent; the engine refuses a moved revision as well
  (`domain/people.md`, 5.1.2).
- **Full is a refusal the page shows.** A snapshot naming more objects
  than the slab holds keeps what fits and the page says how many more
  there are (`architecture.md`, section 3).

### 6.3 Keyed requests

```rust
#[derive(Debug)]
pub struct Pending {
    pub key: Key,
    pub ask: Ask,                    // kept whole, to send again
    pub then: Option<Ask>,           // the second of two requests (ux/inbox.md, section 9)
    pub about: Option<ObjectKey>,    // the card it decides, which shows it as being decided
    pub state: Sending,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sending {
    InFlight { attempt: u32 },       // one Send out, its Answered to come
    Backoff { attempt: u32 },        // busy: its Retry timer armed
    Parked,                          // offline or signed out: sent when the link is back
}
```

| State | `Done`, `Refused` | `Busy` | `Unreachable` | `SignedOut` | `Retry` fires | link back |
|---|---|---|---|---|---|---|
| `InFlight` | apply; send `then`, or retire; save | `Backoff` | `Parked`; the link offline | `Parked`; signing in | stale: ignore | ignore |
| `Backoff` | impossible: nothing in flight | impossible | impossible | impossible | `InFlight`, same key | `InFlight`, same key |
| `Parked` | impossible | impossible | impossible | impossible | stale: ignore | `InFlight`, same key |

- **Minted once per intent.** Confirming an intent on an object makes a
  request with a fresh key, unless one for the same intent and object is
  pending: then the press joins it, and nothing is sent. A double click,
  a retry and a reload all send the same key (`ux/README.md`, 5.3).
- **Kept before it is sent.** The step that mints a request emits `Save`
  with the new `Saved` first, then `Send`, and the shell applies them in
  order; so a reload never loses a request the engine may have made. Each
  answer that retires a request saves again. Drafts are saved on a
  coalescing timer instead, since losing the last keystrokes loses
  nothing that was decided.
- **Restored after identity.** `Start` with what session storage kept
  opens the person watch. Once its snapshot names the saved work's owner,
  the domain opens the page and sends each parked request again with its
  original key. A different person sees a sign-in warning; the saved
  work stays parked and hidden until its owner returns.
- **Two in sequence.** "Amend, then release" and "words, then release" for a
  stopped chat are two requests (`ux/inbox.md`, section 9;
  `ux/chats.md`, section 7): the second, in `then`, is minted when the
  first is answered done, and dropped with a notice when it is refused.
- **Bounded.** A full slab refuses the intent in place ("too many requests
  in flight"); nothing is queued beyond the limit.
- **Words are a request too.** A composer's words are sent the same way,
  shown as being sent while in flight, sent once done, read once a turn
  that took them commits (`ux/chats.md`, 4.1).

### 6.4 Streams and the link

```rust
#[derive(Debug)]
pub struct Stream {
    watch: Watch,
    owner: Owner,                    // the frame, or the page of a generation
    state: Following,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Following {
    Opening,                         // Open sent; Opened or Ended to come
    Waiting,                         // open; its snapshot to come
    Live,                            // its snapshot applied; Silent armed
    Reopening,                       // Missed: Close sent; on Ended, Open at once
    Closing,                         // the client closed it: retired on Ended
    Backoff { attempt: u32 },        // dropped: Reopen armed
}

/// Shown everywhere (ux/README.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LinkState {
    Starting,                        // before the frame's first snapshot
    Live,                            // every watch of the frame and the page is live
    Behind,                          // a watch reopening for a fresh snapshot
    Offline { since: Wall },         // the engine unreachable; requests parked
}
```

- **The frame's watch is always open,** so the link's state does not
  depend on which page is shown, and its heartbeat is the client's
  liveness check.
- **Behind** is a watch told it missed deliveries: it reopens at once and
  its snapshot replaces what its page shows. Nothing the person did is
  lost, since requests are keyed (`ux/README.md`, section 7).
- **Offline** is a dropped watch, a silent one past its heartbeat, or an
  unreachable answer. Watches reopen after backoffs drawn from the seed
  (first, doubling to a most, jittered), and the first that opens brings
  the link back, which sends every parked request.
- **A restart of the engine** is a moment offline, then answers by key:
  provisional text streamed before it is gone from the page as its
  snapshot replaces it (`ux/README.md`, section 7).
- **Signed out:** a watch ended `SignedOut`, or an answer `SignedOut`,
  closes every watch, keeps every request parked and in session storage,
  and shows the sign-in page, whose button asks for `SignIn`. Keys are
  the person's across sign-ins (`domain/people.md`, 5.1.1), so the parked
  requests are sent again once they are back.

### 6.5 Drafts and the confirmation

```rust
/// A field the person types into, by what it is for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FieldRef {
    NewChat,                         // the chats page's composer                      W1
    Reason,                          // the open confirmation's reason                 W4
    Composer { task: u64 },          // a task's words                                 W7
    Amendment(AmendField),           // W8
    Goal(GoalField),                 // W9
}

#[derive(Debug)]
pub struct Field {
    pub text: Box<[u8]>,
    /// Counts the domain's own writes (a composer cleared once its words are
    /// sent, a draft restored): the view writes an input's value only when
    /// this moved, so the person's typing is never overwritten
    /// (architecture.md, section 4).
    pub written: u32,
}

/// The one confirmation open: what an intent will do, said before it is sent.
#[derive(Debug)]
pub struct Confirming {
    pub intent: Intent,
    pub object: Id<Object>,
    pub revision: u64,               // the object's when opened
    pub changed: bool,               // the object moved since: confirming is refused
    pub choices: Choices,            // an option chosen; each delegate's disposal on closing
    pub problem: Option<Problem>,    // a reason missing, a budget beyond the pool
}
```

- **Composers keep their drafts per task,** across navigation and reloads,
  bounded by the number of drafts kept; the oldest untouched goes first.
- **A confirmation's fields** are cleared when it closes, confirmed or
  dismissed.
- **What it says** comes from the object (an acceptance's funding, what a
  cancel ends, what a release will do), so the view can say it without
  asking anything.

### 6.6 Windows

```rust
/// A page's window onto a list of objects too long to hold (ux/README.md, 5.8):
/// the inbox's groups, a task's ended delegates.
#[derive(Debug)]
pub struct ObjectWindow {
    pub rows: List<Id<Object>>,
    pub older: Edge,                 // more before: the cursor to read from; or the start
    pub newer: Edge,
    pub loading: Option<Id<ReadSlot>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    End,                             // nothing more that way
    More { cursor: Cursor },         // read from here for the next page
}
```

The subset has no generic items, so each kind of row has its window
(`ObjectWindow`, `ChatWindow` for the person's chats, `TurnWindow` for a
transcript, `LineWindow` for history), each a few lines over
`skein_lib::List`, sharing `Edge` and the sliding rule in `window.rs`: a window at its limit that loads more drops
rows from its other end and keeps the cursor to load them again; a live
list appends only while its window reaches the newest end.

### 6.7 Notices and time

- **Notices** are short, with a deadline: a request answered for a card no
  longer shown; a refusal of something that has no card; the link going
  offline and coming back. A card's own outcome is on the card.
- **Relative times** ("waiting 4 min") are the view's, from the object's
  wall time and `Domain::wall`. While a page shows any, a `Tick` timer
  counts `shown` once a minute, so the view builds again.

## 7. Limits

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Limits {
    pub objects: u32,                // a page's working set
    pub requests: u32,               // keyed requests pending
    pub streams: u32,                // watches open, the frame's included
    pub reads: u32,                  // reads in flight, abandoned ones included
    pub window: u32,                 // rows per window
    pub turns: u32,                  // turns a transcript window holds
    pub tree: u32,                   // plan nodes held
    pub drafts: u32,                 // fields kept
    pub notices: u32,
    pub words: u32,                  // bytes a person may write: the engine's people.words
    pub text: u32,                   // bytes of any one text shown: a spec, a result, a turn
    pub streaming: u32,              // bytes of a provisional turn
    pub backoff: Backoff,            // first and most, for requests and watches
    pub heartbeat: Duration,         // silence past which a watch is dropped
    pub linger: Duration,            // a decided card stays this long
    pub notice: Duration,
    pub save: Duration,              // drafts' coalescing
    pub facts: u32,
}
```

`worst_case` sums the containers' own `worst_case` and the bytes each
entity may hold (`programming-model.md`, 6.3), as
`temper-engine-domain-people` does. It also bounds what the view builds
(02-view.md, section 5): the view's limits are derived from these, so a
tree always fits.

## 8. What it never knows

- **No bytes to parse:** URLs, JSON and session storage's encoding are the
  layers' below; words, specs and results are carried and never read.
- **No browser:** no DOM, no events but typed actions, no scroll, no focus.
  Following the end of a conversation as it streams is the shell's (it
  keeps a node's place on screen), not the domain's.
- **No engine internals:** tasks by number, objects by key and revision,
  outcomes and refusals as the web protocol gives them. Authority is never
  computed here: what a card offers is what the engine said the person may
  do, and the engine checks it again.

## 9. Connectors' panels

The forge's panels (`ux/forge.md`) are one module, `forge.rs`: a change's
body for its card and its panel, the changes page, its watch's snapshot
and changes. The core names it only through a closed set of seams:
`Body::Change`, `Page::Changes`, `Watch::Changes`, `Address::Changes`, and
a task page's `panels`, a closed `Panel` enum with one variant per
connector. When a second connector comes, or the module outgrows the
crate, it becomes a child domain (`programming-model.md`, 4.5), as the
engine's connectors are, and only those seams change.

## 10. Step tests

`src/tests.rs`, a module per part (`tests/requests.rs`,
`tests/streams.rs`, and so on), each driving `step` and `fire` with
hand-built events and checking the requests emitted and what the read API
shows:

- **requests:** every cell of the table in 6.3; a double press joins; a
  reload restores and resends with the same key; `Save` precedes `Send` in
  the same step; `then` sent only after done; a full slab refuses in place;
  keys differ across seeds and repeat for one seed;
- **streams and the link:** every cell of `Following`'s table; a heartbeat
  missed; a reopen after `Missed` with no backoff; backoffs' bounds;
  offline to live sends the parked requests; signed out closes and parks;
- **pages:** going to a page closes the old watches, abandons its reads and
  clears the working set; a terminal for an old generation is dropped and
  retires its slot; `Went` shows no address request;
- **objects:** a snapshot's sweep; one object in two places; the answer and
  `Left` in both orders; a revision moved under an open confirmation; a
  stale handle refused;
- **drafts:** `written` moves only on the domain's writes; drafts restored
  from session storage; the coalescing timer;
- **limits:** `worst_case` refuses invalid limits, `max_out` holds for the
  largest step (a navigation with every watch open), a full slab is a
  refusal everywhere.

Behaviour across many steps (a story, a fault) is the worlds'
(03-testing.md).
