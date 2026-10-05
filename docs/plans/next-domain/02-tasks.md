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
    numbers: Numbers,         // its current allotment, as authority's answers say
    funder: Funder,           // actual funding link, including the original period
    allotment: u64,           // durable generation, closed exactly once
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

A sketch of the complete target; later increments add their variants
when they implement the behavior. Sections 2.4 and 2.5 describe the implemented 02a and 02b contracts.

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
    Activation { reply_to: ReplyTo, task: u64, attempt: u64, end: End },
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
| an allotment closure | by task and generation | ended or replaced in a move |
| a run's cumulative committed spend | by task and attempt | each charged turn and answer |
| a stub of an ended task | by number | ended while a live task names it |

A task that ends is erased from the live range and saved to the ended
one, in the commit that ends it, with its result.

Moves preserve every live allotment's unspent amount by normalizing each
actual funding component bottom-up, validating replacement reservations,
and committing their closures, reopened generations and history together
(authority.md, section 7). The hub checks external incoming funding links
across the moved requester subtree: a delegate may have been funded by an
accepted proposal's holder rather than its requester. Allocations funded
by old tasks that must end are separately transferred, or the move is
refused; original pool and period identities otherwise remain. The same
actual funder keeps its counters unchanged. A budget widening cannot mix
sources under one `Funder`: a different source must re-fund the whole
remaining allotment and increase, subject to authority, or be refused.
An in-flight run's answer charges only its cumulative whole less that
run's previously committed expense, even across allotment replacement.
History and generation records prevent a replay from closing an old
allotment or charging an old turn twice.

### 2.4 The implemented 02a contract

02a implements agent executors, batches and their dependencies, per-class
failure/backoff, holds, cancellation and ordered closing. It carries its
own authority, numbers and funder values; authority and funding decisions
are the root's. Procedure/person executors, turns, inboxes, references,
proposals and funder arithmetic arrive in their later increments.

`Activate` asks the root for work. Reply-bearing `Prepare` and `Claim`
advance due → preparing → claimed; `Started` records placement. The root
issues fresh task and attempt numbers and commits a claim before assigning
the run. A task's attempt must strictly grow; external stale or invalid
claims are refused with typed state/attempt reasons. `Activation` answers
with `Acknowledged { accepted: New | Already }` or a typed refusal. A
finish with live delegates is a call before the run exits: a refusal
leaves its attempt live so the executor can correct the finish. Invalid
results instead consume an invalid-result try. No attempt/try is spent on
preparation failure; assignment refusal consumes the claim's number and
pauses without spending a try. Only agent execution is supported, while
carried authority delegation kinds include charters, procedures and roles.

Dependency admission initially accepts members of this batch and the
creator's existing live direct delegates. Introduced references are 02b.
02b follows the normative reference rule in `tasks.md`, section 4, and
accepts legal introduced dependencies; the old contrary story is corrected
in section 3. Ended tasks are inputs, not new
dependencies. The root may load a trustworthy ended summary through
bounded, reply-bearing `RememberStub`, in the same decision as `Make`.
`ForgetStub` refuses while a live dependency or input names the stub;
02b extends that guard to references. Loaded failure/cancellation summaries
are valid inputs too. Remembered summaries occupy the configured stub
limit until the root forgets them, and every live task reserves a future
stub slot before batch mutation. Lifetime subtree-made counts preserve
per-tree limits when completed delegates leave the live set.

Held records preserve waiting, active or closing state and any closing
result. Closing refuses new batches even while held, so a new delegate
cannot invalidate settlement. A held live run is stopped, and release refuses while its terminal
is outstanding. Incoming terminal/settlement updates the saved prior state
without lifting the hold; release resets tries, restores that state and
reassesses dependencies. Cancellation lifts holds throughout its subtree,
waits for own runs, then delegates, then the root's `Close`/`Settled` gate.
A cancelled task retains an optional completed result describing what it
had done. Effect/resource settlement belongs to that root gate in 02a.

`Live`, `Ended` and `Stub` records use their own typed keys, with immediate
`Save`/`Erase` in the same decision as each reply or outward request
(`engine.md`, 5.6). `Ended` asks the root to commit exactly one requester
result message with the ended record; it is durable mail, never a volatile
notification reconstructed from history. The root keeps its run/call
acknowledgements after task/stub eviction. Startup-only `Restore` and
`Restored` accept rows in any order, validate links and the dependency DAG,
reproject stored wall backoffs once, and request adoption of committed
claims before new work. Due/preparing work is rebuilt; closing gates are
reissued idempotently. General paging remains 02f.

### 2.5 Implemented 02b contract

Messages have root-issued IDs. `Send` carries only implemented user words,
questions and answers; `Peek` returns whole oldest messages within a byte
budget and reports what waits. `Claim.readable` names the actual brief's
messages. Reply-bearing `Turn` commits a sequential attempt-local turn and
its read-through over durable immutable offers. A turn takes only offered
IDs, preserving policy-kept older messages. Merging replaces a hint with a
fresh ID while retaining its oldest arrival and occurrence count. Live
relays and replies escape only after their offers, receipts and task/message
records commit. Accepted terminal answers clear offers, retaining unread
messages; ending archives those messages.

Batch admission reserves each delegate's terminal result capacity before
mutation. `results_due` persists the credit until `DeliverResult` converts it
into an inbox row; the root completes every `Ended` → `DeliverResult` in
that same decision. Open questions reserve answer inbox and receipt capacity.
Bounded exact-call receipts admit retries without duplicate messages; a
changed call gives `KeyConflict`, pressure saves no receipt, and
root-managed `ForgetReceipt` follows retirement of root call history.
The root keeps a logical call key stable across pressure retries. It treats
child `Busy`, `Inbox` and `NotReady` as retryable pressure, caching only the
final outcome. If other deliveries commit meanwhile, retry dispatch uses a
fresh message candidate, preserving increasing committed message IDs; it
must never reuse a refused low candidate beneath a later read-through.
Replaying an accepted candidate still uses that candidate's saved receipt.
Cancellation uses its existing durable control phase and never competes for
ordinary inbox room. Future amendments and proposal decisions add their
message variants only when their behavior lands.

`Introduce` grants reciprocal bounded explicit references only when its
introducer sees both live tasks in the same project. A requester reserves
an explicit reference for each admitted delegate,
so its visibility survives that delegate ending until it forgets the
reference. Pending result credits and subscriptions prevent forgetting it;
reference pressure is retryable `Busy` after room is released. Dependencies
may use the creator's introduced live references. The admission and restore
checks
include existing dependencies and parent-to-delegate waits, not merely the
batch-local graph. Ended references, unread results/notices and dependencies
keep stubs; subscriptions prevent removing their explicit reference.

`Subscribe` supports referenced task state/result, opaque connector topics,
and once/periodic timers. `Notify`, `Observe` and `Timer` are real root
callbacks completed before commit; `Observe` loads an ended referenced task's
kept result. `Topic` registers/removes actual connector interest. News can
be lowered from wakes to kept/dropped by policy. The closed policy controls
each supported kind, batches permitted kinds by count or oldest age, and
always wakes for person words. Monotonic deadlines project wall times once
while live and reproject on restart. Reached batching thresholds persist
with messages even while held, surviving clock correction and restart.
Holds preserve inboxes and interests;
subscriptions end with their owner. `Restore` rejects malformed rows,
missing links, overcommitted capacity and unfinished callback state.

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
in dependency order; a legal dependency on an introduced sibling accepted,
a cross-subtree dependency/delegation wait cycle refused atomically, then a
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
   levels. Focused restart cuts and saturated-memory checks already cover
   these records and gates; 02f broadens the complete hub's coverage.
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

### 5.1 The 02c accounting seam

02c adds checked `Control`, `Amend`, `Move` and cumulative `Charge` events.
`Authorization` is authenticated evidence from the root: tasks checks actual
tree standing, while project-role and escalation standing and authority
comparisons remain authority's checks, translated by the root. An amendment
carries all affected descendant authority snapshots; the root guarantees that
this list includes every delegation ceiling changed by the amendment and
marks the live runs whose grants cannot honour the new authority. Dependencies
can only be removed while waiting. Each affected executor receives a durable,
merged amendment message in a separate reserved control slot, preserving the
ordinary inbox's existing capacity. Offers reserve two separate immutable
control slots per task as well; ordinary offer saturation cannot delay an
admitted amendment. A third unread live amendment refuses `Busy` before any
mutation until a read or terminal frees a control slot. Immutable prior offers
stay readable. Durable narrowing reissues `Stop` after restart, including a
cut after its commit but before the first `Stop` delivery.

Moves validate the complete future requester wait graph, project, depth,
lifetime tree bounds, references, terminal result credit and tracked goals.
A tracked subtree cannot become a task's delegate. The old requester keeps
its explicit reference; the moved root also retains its old requester as an
explicit reference. Actual funding links are checked separately. Each incoming
allocation from an old task funder is considered explicitly; old ancestors
which cease to be ancestors cannot remain its funders. Stable external pool
and period identities are retained for other allocations.

Every actual funding component replaced by a move is normalized from its
leaves. `Transfer` names exact old/new funding links. `Balance` names finite,
authentic before/after funding snapshots, already authority checked by the
root. Tasks verifies task-funder snapshots and exact arithmetic, including
separate incoming allocations funded directly by an old ancestor. It refuses
missing transfers, cycles, missing reservation evidence, overruns that cannot
preserve promised unspent, and insufficient new funds before changing any row.
A same-actual-funder transfer preserves its counters. All changed generations,
`Stored::Closure`, replacement counters, `Stored::Funding` external snapshots
and immutable `Stored::History` rows commit together. A task cannot close while
it funds live allocations, even outside its requester subtree.

`TaskRecord::run_spent` is attempt-local cumulative committed spend, reset only
by a fresh claim. `Charge` adds only the whole minus that prior cumulative
amount, preserves it through replacement, refuses a decreasing whole, and
holds a charged overrun. It is a primitive for the atomic admission in 02e:
**the root must not charge a turn or terminal before that call is accepted.**
02e must add combined charged turn/terminal events so a refused read, turn,
finish or attempt cannot precharge the ledger.

02c reserves and settles actual live task funders concretely. External funding
at ordinary `Make` still has an explicit root precondition: authority checked
and reserved the actual named pool/period before `Make`, in its same decision.
External balances for moves/amendments are emitted as `Stored::Funding`, but
02c does not hold a complete person-pool/project-period ledger, initial pool
carving, period resets or the pool's funding link to its original project
period. Its unique closure row gives the root the original identity, generation,
budget and spend to settle externally. Those finite ledgers and atomic charge
admissions must land in **02e before the root's charged walking story**, rather
than being invented as independent funding state in the root. The original
02a/02b prerequisite suffices for its routing skeleton, not its accounting story.
