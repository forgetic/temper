# Step 03: people

Provisional, 2026-10-04. The people child domain,
`temper-engine-domain-people`, and its world, `tests/engine/people`:
people as parties (`people.md`). Signing in through the forge, roles per
project, requests checked and answered once durable, keyed so that one
sent twice is made once, inboxes derived from the tasks, person tasks
taken and handed back, and the web's live connections. It is new: today
people's requests are the legacy root's `people.rs`, acting on the forge.
Overview and conventions: README.md.

## 1. What it takes from the legacy code

Little. From `temper-legacy-engine-domain`'s `people.rs` and boundary:
the shape of a person's call (`Ask`, answered by exactly one `Reply`
through a `ReplyTo`), keys carried by the web so a request sent twice is
made once, and watches handed to the views. Everything else is new: the
legacy engine checks a person's permission read from the forge per
request, opens sessions as issues and writes messages as comments, none
of which survives (`people.md`, section 13).

## 2. The crate

```
crates/temper-engine-domain-people/src/
├── lib.rs          its doc: what it keeps, what it never knows of tasks (people.md, section 2)
├── boundary.rs     Event, Request, Stored, Key; Identity, Role, Ask, Reply, Entry
├── domain.rs       Domain: people, sign-ins, roles, requests in flight, inboxes, connections
├── signin.rs       sign-ins made, expired, ended; a first owner from configuration
├── roles.rs        roles per project; seeding at adoption; changes by owners
├── requests.rs     keyed requests: admitted, routed, answered once; the same key answered from its record
├── inbox.rs        entries per person and per role, bounded; read positions; a role's entry leaving every inbox
├── person.rs       person tasks: taken, handed back, answered; two people deciding one thing
├── facts.rs
├── limits.rs
└── tests.rs
```

### 2.1 Its vocabulary

```rust
/// parent -> people
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// The web's protocol layer exchanged a forge's authorisation code and
    /// read who the person is; their sign-in's tokens it keeps itself, in
    /// the store's secret records (people.md, section 3).
    SignedIn { reply_to: ReplyTo, sign_in: u64, identity: Identity },
    SignOut { reply_to: ReplyTo, sign_in: u64 },
    /// A person's request, with the key the web page made for it.
    Ask { reply_to: ReplyTo, sign_in: u64, key: [u8; 16], ask: Ask },
    /// The root's outcome for a request it routed to authority and the tasks.
    Decided { request: Token, outcome: Outcome },
    /// What waits for whom changed, as the tasks say (people.md, section 6).
    Waiting { whom: Whom, entry: Entry, present: bool },
    /// A project's roles, and their changes; a repository adopted, with its
    /// collaborators' permissions, to seed roles from (people.md, section 4).
    Roles { project: u32, holdings: Box<[Holding]> },
    Adopted { project: u32, collaborators: Box<[Collaborator]> },
    /// The web's live connection of a sign-in opened or closed.
    Connected { sign_in: u64, connection: Token },
    Disconnected { connection: Token },
    Restore { record: Stored },
    Restored,
    Loaded { owner: Token, rows: Box<[Stored]>, more: bool },
}

/// people -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    /// A request the person's role allows on its face: the root asks
    /// authority (authority.md, 8.4), then the tasks, and answers `Decided`.
    Route { request: Token, person: u64, project: u32, role: Role, ask: Ask },
    /// The one answer to an `Ask` or a sign-in; held by the root until the
    /// commit it follows from is durable (domain/engine.md, 5.2).
    Reply { to: ReplyTo, reply: Reply },
    /// To a person's open connections, through the views: their inbox changed.
    Push { connection: Token, change: Change },
    Save { record: Stored },
    Erase { key: Key },
    Load { owner: Token, range: Range },
}

/// What a person asks (people.md, 5.1), typed all the way down: the web's
/// protocol layer decodes it (programming-model.md, section 4). Tasks are
/// named by number; words are bytes the domain carries and never reads.
#[derive(PartialEq, Eq, Debug)]
pub enum Ask {
    StartChat { project: u32, words: Box<[u8]> },
    SetGoal { project: u32, spec: Box<[u8]>, charter: u32, budget: u64, priority: u32 },
    Write { task: u64, words: Box<[u8]> },
    Answer { task: u64, answer: PersonAnswer },
    Decide { waiting: Waiting, decision: Decision },
    Amend { task: u64, amendment: Amendment },
    Cancel { task: u64 },
    Release { task: u64 },
    TakeOver { task: u64 },
    Stop { task: u64 },
    Prioritise { goals: Box<[u64]> },
    Adopt { project: u32, repository: Repository, role: RepositoryRole },
    Policy { project: u32, policy: PolicyChange },
    Note { scope: Scope, name: Box<[u8]>, change: NoteChange },
    Watch { subject: Subject },
}
```

- **It decides what is the person's to ask,** from their role alone
  (`people.md`, 5.1): a member may start a chat, an observer may not.
  What gives authority (a goal's budget, an acceptance) is authority's to
  check, through the root. A request refused at either says why.
- **Keys:** a request's key, with its answer, is saved for a retention
  (`engine.md`, 5.4), so the same key after a restart is answered from
  its record, never made twice.
- **Inboxes are derived, not kept,** but for each person's read position
  over results and replies (`people.md`, section 6).

### 2.2 What it saves

| `Stored` | Saved when |
|---|---|
| a person: forge, user id, login, name | first signed in |
| a sign-in: its number, its person, its expiry (the tokens are the protocol layer's) | made, ended |
| a project's roles | seeded, changed |
| a request's key and its answer, for a retention | answered |
| a person's read position | they read |

## 3. The world

`tests/engine/people`, package `temper-engine-people-world`. The world
is the parent: it routes requests as the root would, answering
`Decided` from a scripted authority and tasks; it feeds `Waiting` entries
as scripted tasks ask questions, propose and hold; it plays the web's
protocol layer, signing people in and opening and closing their
connections; it commits what was saved and restarts the domain from what
was durable.

Its stories (`people.md`, section 12, at this child's scale): a member
starts a chat; an observer is refused; a member's goal past their
allotment is routed as a proposal to maintainers, and one accepts; two
maintainers decide one proposal, the second told it was decided, by whom,
and how; a person task addressed to a role, taken by one, handed back,
answered by another; a request sent twice, across a restart, made once;
a sign-in expiring; a repository adopted, its collaborators seeded into
roles, one already holding a role keeping it.

Its referee: every request answered once, and only after the commit it
follows from; nothing acted on beyond the person's role; what waits for a
role leaves every inbox once one person acts; a person's words reach their
task's inbox, or the task has ended.

## 4. Increments

1. **03a signing in, roles, requests.** Enough for step 06's skeleton:
   a person signed in starts a chat, routed and answered.
2. **03b inboxes and person tasks.**
3. **03c adoption's seed, policy changes, retention of keys, memory.**

## 5. Done when

- every request of `people.md`, 5.1 is in `Ask`, routed or refused, with
  a test each;
- the stories and the referee of section 3 pass, and the fuzzy sweep
  reaches every ending within its allotment;
- the crate depends on `skein-lib` alone.
