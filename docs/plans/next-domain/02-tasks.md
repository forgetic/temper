# Step 02: tasks

Provisional, 2026-10-04. The tasks child domain,
`temper-engine-domain-tasks`, and its world, `tests/engine/tasks`: the
engine's hub (`tasks.md`). Tasks in a tree, made in batches, one
lifecycle for every executor, durable inboxes and wake policies,
subscriptions and references, amendments and cancels down the tree,
proposals and escalations up it, funders' numbers, the recurring
procedure. It is built beside the legacy `work` and `plan`, which it
replaces at the cutover. Overview and conventions: README.md.

## 1. What it takes from the legacy code

Ported by copying and adapting, never by making the old code serve both
(README.md, 3.1):

| From | What | Into |
|---|---|---|
| `temper-legacy-engine-domain-work`, `tracked.rs` | the six failure classes and their per-class `Retry` (tries, backoff doubling to a ceiling, half jittered from a seeded `Rng`); attempts that only grow; a refusal before anything ran counting no try | `failures.rs` |
| the same | an answer acknowledged once the parent made it durable, so a copy after that is answered again and dropped | `run.rs` (with turns) |
| `temper-legacy-engine-domain-plan`, `check.rs` | a plan's checks: no cycle, a bounded size | `batch.rs`, now over one batch |
| the same, `wake.rs` | wakes and their batching by count and age | `wake.rs`, as policies of data |
| the same | a step done only once the steps it added are | `closing.rs`: a task ends after its delegates (`tasks.md`, 5.6) |

What does not come: records, mangled records, hold codes as small
integers (a hold's reason is a typed `Hold` here), items, the inbox
derived from comments, plans as typed graphs, envelopes and growth
(delegation within authority, step 01, and proposals here).

## 2. The crate

```
crates/temper-engine-domain-tasks/src/
├── lib.rs          its doc: what it keeps, what it never knows (tasks.md, section 2); re-exports
├── boundary.rs     Event, Request, Stored, Key, and the values they carry
├── domain.rs       Domain: the slabs of live tasks, stubs, proposals; step, fire
├── task.rs         one task's phase machine (tasks.md, section 5): a handler per cell
├── batch.rs        a batch checked whole: well formed, acyclic, within the limits
├── run.rs          an agent task's phases: idle, due, preparing, claimed, running, backing off
├── failures.rs     classes, tries, backoff
├── closing.rs      closing in order: run, delegates deepest first, effects, releases
├── inbox.rs        admission (room kept, merged, refused), taking, relaying
├── wake.rs         wake policies over kinds and news classes; batching; timers
├── refs.rs         subscriptions to tasks and timers; references and introductions
├── proposals.rs    proposals and escalations: where each waits, deciding, stalls
├── funders.rs      the numbers per task, per person's pool, per project's period
├── recurring.rs    the core procedure (tasks.md, section 9)
├── stored.rs       what is saved, and restoring from it
├── facts.rs
├── limits.rs       tasks.md, section 10, every one
└── tests.rs        step tests: each phase's cells, each admission rule
```

### 2.1 What a task is, in it

```rust
/// A live task (tasks.md, section 3). Its number is the engine's, never reused.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) struct Task {
    number: u64,
    project: u32,
    requester: Party,
    proposer: Option<Party>,
    executor: Executor,
    spec: Spec,               // words and typed parameters, bounded; the root's to read
    contract: Contract,
    authority: Authority,     // tasks' own type for it: carried, never judged
    numbers: Numbers,         // against its authority, as authority's answers say
    funder: Funder,
    tracked: Option<Priority>,
    dependencies: Set<u64>,   // live siblings it starts after
    delegates: Set<u64>,
    references: Set<u64>,
    subscriptions: Set<Subscription>,
    policy: WakePolicy,
    holds: Box<[Resource]>,   // write holds, as opaque references the root gives
    attempt: u64,
    tries: Tries,
    phase: Phase,
}

/// Who a task is from, or for: a task, a person, the deployment.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Party {
    Task(u64),
    Person(u64),
    Deployment { project: u32 },
}

/// Who carries a task out; the root's references, opaque here.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Executor {
    Agent { charter: u32 },
    Procedure { connector: u16, code: u32 },
    Person { whom: Whom },
}

/// tasks.md, 5.1: the phases every task has, whatever its executor.
#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Phase {
    Waiting,
    Active(Active),
    Closing { stage: Stage, how: Ending },
    /// Stopped for a decision, from where it was; a release returns it there.
    Held { was: Was, why: Hold },
    Ended(Ending),
}

#[derive(PartialEq, Eq, Hash, Debug)]
pub(crate) enum Active {
    /// An agent's task (tasks.md, 5.2).
    Idle,
    Due,
    Preparing,
    Claimed { attempt: u64 },
    Running { attempt: u64, read: u64 },
    BackingOff { until: Time },
    /// A procedure's task: stepped by its owner (5.3).
    Stepping,
    /// A person's task: in an inbox (5.4).
    Asking { since: Wall },
}
```

### 2.2 Its vocabulary

A sketch; the names are the implementer's to settle against `tasks.md`.

```rust
/// parent -> tasks
#[derive(PartialEq, Eq, Debug)]
pub enum Event {
    /// Make a batch authority allowed, its creator's numbers as authority's
    /// answer leaves them (authority.md, 8.1). Answered by one `Made` or `Refused`.
    Make { reply_to: ReplyTo, creator: Party, batch: Box<[New]>, numbers: Numbers },
    /// The executor of `task` did something: a run's turn took its inbox up
    /// to `read` and spent `spent`; a run ended (finished, parked, failed by
    /// class); a procedure stepped; a person answered.
    Turned { task: u64, attempt: u64, turn: u32, read: Option<u64>, spent: u64 },
    Activation { task: u64, attempt: u64, end: End },
    /// A message admitted at its sender's entrance (tasks.md, 7.2), or a
    /// connector's news, classified for this task (7.3).
    Message { to: u64, from: Party, message: Message },
    Amend { reply_to: ReplyTo, task: u64, by: Party, amendment: Amendment },
    Cancel { reply_to: ReplyTo, task: u64, by: Party, why: Box<[u8]> },
    Release { reply_to: ReplyTo, task: u64, by: Party },
    Move { reply_to: ReplyTo, task: u64, to: u64 },
    /// What holds a task from outside it: drift, an effect failed for good,
    /// a person's stop, a run refused as unstartable (tasks.md, 5.5).
    Hold { task: u64, why: Hold },
    Propose { reply_to: ReplyTo, from: u64, action: Action, reason: Box<[u8]>, theirs: bool },
    Decide { reply_to: ReplyTo, waiting: Waiting, by: Party, decision: Decision },
    /// Authority's answer to `Route`: whether `holder` covers it.
    Covers { waiting: Waiting, holder: Party, covers: bool },
    /// A claim was committed and placed, or refused before anything ran.
    Claimed { task: u64, attempt: u64 },
    Unclaimed { task: u64, attempt: u64, refusal: Refusal },
    /// Its connectors settled what `task` asked for, and released what it held.
    Settled { task: u64 },
    Restore { record: Stored },
    Restored,
    Loaded { owner: Token, rows: Box<[Stored]>, more: bool },
}

/// tasks -> parent
#[derive(PartialEq, Eq, Debug)]
pub enum Request {
    Made { reply_to: ReplyTo, tasks: Box<[u64]> },
    Refused { reply_to: ReplyTo, refusal: Refusal },
    Done { reply_to: ReplyTo },
    /// Engage the executor of `task`, for `why`: a run due, a step, a person
    /// asked. The root prepares and claims, steps, or tells the person.
    Activate { task: u64, executor: Executor, why: Why },
    /// End the live run of `task`: a cancel, a narrowing it cannot honour.
    Stop { task: u64, attempt: u64 },
    /// Relay to the live run of `task` the message its policy lets through.
    Relay { task: u64, attempt: u64, message: u64 },
    /// Where should this wait? Ask authority whether `holder` covers it.
    Route { waiting: Waiting, holder: Party, below: u32 },
    /// What waits for a person changed: a question, a proposal, an
    /// escalation, a person task, a result (people.md, section 6).
    Waiting { person: Whom, entry: Entry, present: bool },
    /// The closing of `task` reached its effects and resources: its
    /// connectors settle and release, then answer `Settled`.
    Close { task: u64, ending: Ending },
    /// A task ended, its result to its requester (a message, already saved).
    Ended { task: u64, requester: Party, ending: Ending },
    Save { record: Stored },
    Erase { key: Key },
    Load { owner: Token, range: Range },
}
```

- **Every check but authority's is here** (`tasks.md`, section 4): a
  batch is well formed, acyclic among its own tasks, and within the
  limits (live tasks, per tree, depth, delegates, batch size); failing,
  it is refused whole, naming the task and why. Authority's check is the
  root's, before `Make`.
- **`Activate` never starts a run.** The root prepares (brief,
  transcript), asks authority (`authority.md`, 8.3), commits the claim and
  places it; it tells the hub `Claimed` or `Unclaimed`. The hub only knows
  a run is wanted, and how it ended.
- **Proposals route by questions.** The hub knows the tree; authority
  knows who covers what. The hub walks up the tree, skipping procedures,
  asking `Route` for each holder in turn, and the first `Covers { covers:
  true }` is where it waits (`tasks.md`, section 8).

### 2.3 What it saves

| `Stored` | Key range | Saved when |
|---|---|---|
| a task, every field of 2.1 | live tasks; ended tasks | made, and at every change of phase, tries, attempt, numbers |
| a message in its inbox | by task, by sequence | admitted; erased once taken |
| a proposal | pending; decided | made, routed, decided |
| an entry of a task's history | by task, by sequence | made, amended, held, released, cancelled, moved |
| a funder's numbers | persons' pools, projects' periods | carved, settled, reset |
| a stub of an ended task | by number | ended while a live task names it |

A task that ends is erased from the live range and saved to the ended
one, in the commit that ends it, with its result.

## 3. The world

`tests/engine/tasks`, package `temper-engine-tasks-world`, shaped as
README.md, 5.4. The world is the hub's parent and plays:

- **authority's answers**, from the seed: allow, propose or refuse a
  batch, an amendment, a proposal; cover or not;
- **executors:** agent runs (turns that take the inbox, then finish with
  a result of the task's contract, park, or fail by class); procedures
  that step with a decision (delegates, a result, a hold); people who
  answer, late or never;
- **connectors' news,** classified wakes, kept or dropped, on topics the
  tasks subscribed to; effects that settle late, fail for good;
- **people** who create, amend, cancel, release, move, decide;
- **the store:** it commits what the hub saved at the end of each step,
  and the engine restarts at moments the referee chooses, the hub rebuilt
  from what was durable.

Its stories, from `tasks.md`, section 11, each a focused test: a batch
made whole or refused whole; a plan of spikes, a choice and changes, run
in dependency order; a dependency on an introduced sibling accepted, a
dependency closing a wait cycle across subtrees refused, then a
subscription to the sibling and its ended result used as an input; a
negative verdict starting dependents, a failure holding them; a delegate
held past its tries, escalated two levels to a person, released and
finished; a coordinator woken once by a burst; a cancel closing three
levels with runs live and effects in flight, deepest first; an amendment
reaching a live run; a proposal routed past a procedure to a person and
accepted, one stalling and passing up; a chat's goal accepted as its
person's, outliving the chat; an inbox filling, news merged, words
refused, a result still taken; a recurring task across a reset, and with
the engine down for several periods, making one batch.

Its referee (`tasks.md`, section 11): a task starts only after its
dependencies are done; no task waits on itself through dependencies and
delegates; batches whole or not at all; one run at most per task; every
result reaches its requester once, after the task closed; a cancel ends
everything below it, deepest first; no committed message lost, none
refused once committed; every proposal and escalation waits where a
holder can decide it; spend counted once, up the funding chain; nothing
past a limit held. Restarts at drawn moments, with the referee checking
that what was durable is what the hub resumed.

Its fuzzy sweep: random trees, random executors and faults, every ending
(done, failed, cancelled, held and released, proposal accepted, rejected,
stalled, withdrawn) reached across the seeds.

## 4. Increments

1. **02a tasks, batches, lifecycle.** `task.rs`, `batch.rs`, `run.rs`,
   `failures.rs`, `closing.rs` (run and delegates; effects and releases as
   `Close` and `Settled`); `Stored` for tasks; the world with authority
   always allowing, agent executors only, no messages but results. Stories:
   batches, dependency order, failures and holds, a cancel down three
   levels.
2. **02b inboxes and wakes.** `inbox.rs`, `wake.rs`, `refs.rs`: admission,
   taking by a turn, relaying to a live run, policies and batching,
   subscriptions to tasks and timers, references and introductions.
3. **02c amending, cancelling, releasing, moving.**
4. **02d proposals and escalations,** routed by `Route` and `Covers`,
   stalls, withdrawal.
5. **02e funders and the recurring procedure.** `funders.rs`,
   `recurring.rs`; procedure and person executors in the world.
6. **02f restart and memory.** Restores at drawn moments in the fuzzy
   sweep; the memory test at every limit.

02a and 02b are what step 06's skeleton needs; the rest can land while
06 begins.

## 5. Done when

- every story of section 3 is a focused test, and every rule of the
  referee fails a test written to break it (`tests/referee.rs`);
- the fuzzy sweep reaches every ending, within its allotment;
- the crate depends on `skein-lib` alone; nothing of the legacy crates
  changed.
