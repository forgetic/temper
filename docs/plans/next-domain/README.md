# Migrating to the next domain design

Provisional, 2026-10-04. A plan for taking temper's code from the domain
as built (`docs/design/engine-domain.md`, `worker-domain.md`,
`agent-domain.md`) to the domain design (`docs/design/domain/`), in
steps that each land on main with every check of
`docs/development/workflow.md` passing. This document is the overview:
what I think of the migration and why it is shaped as it is, the
conventions every step follows, and the steps in order. Each step has a
document of its own beside this one.

**Goal, revised 2026-10-05.** Migrate temper's domain layer to the new
design, including the temper/smith split which extracts agent-specific
domain code into `~/src/rust/smith/`. The goal is completed when both
temper and smith domain logic is in place and follows the new design.

Completion includes this plan's remaining increments, cutover and domain
contractions, smith's domain contracts and world stories, and temper's
integration with smith. Copying the existing agent into smith alone does
not complete the goal. Both repositories follow skein's foundation
documents, independent review and their own exact-tip gates. Features
explicitly deferred by the designs remain deferred; the lower-layer
follow-on plans retain their existing scope. Implementation resumed
on 2026-10-05 at the user’s request, with this revised joint goal.

**Citations.** A bare file name names a document of the next design
(`tasks.md`, `forge.md`), as the design's own README does: in
`docs/design/domain/` (moved from `docs/design/next/domain/` in step 00);
today's
documents are named by path (`docs/design/forge.md`, the forge's protocol
layer) or as "the current `engine-domain.md`"; skein's foundation
documents by their file names (`programming-model.md`,
`testing-strategy.md`); smith's design documents, in smith's repository,
with smith's name (smith's `run.md`).

## 1. In one page

- **Most of it is built beside, not changed in place.** `tasks`,
  `authority`, `people` and the forge connector are new child domains,
  each with its world; the engine's root is rebuilt beside the old one.
  None of it edits the code it will replace, so no test of that code
  moves.
- **The old engine stays green, frozen, until one cutover.** Its five
  crates and four worlds move aside under `legacy` names in step 00 and
  change no more. The system worlds keep running it until the new root
  has caught up (step 06); then one cutover (step 07) switches them over
  and deletes it.
- **What the design only adds to is extended in place.** The fleet, the
  worker, the channel, the brief, the views, the fake forge and the fake
  checkout gain variants and fields beside what they have. The old
  engine never sends the new ones, so its tests do not move.
- **The agent leaves for smith** (step 05s): a kit for flexible LLM
  agents in its own repository, which temper takes as it takes skein.
  smith starts from a copy of temper's agent and is made generic there;
  temper's agent is frozen as legacy until the cutover switches the
  system worlds to smith's.
- **What goes, goes after the cutover** (step 08): snapshots, the
  channel's first payloads, the wiki, labels as an index, base branches
  the worker makes, the old names in views and briefs. Deleting is the
  last kind of step, and the safest.
- **The store and the web are boundaries the new root has from its first
  day,** in its world, through fakes. Their protocol and io layers come
  after the cutover, since no temper service runs yet (2.4).
- **A walking skeleton early.** The new root runs one chat end to end (a
  person's request, a run, a turn committed, an answer, a result) as
  soon as `tasks`, `authority` and `people` can, before the rest of it.
  The design is provisional, and a skeleton is where its gaps show first.

```
00 groundwork ──┬──► 01 authority ─────────┐
                ├──► 02 tasks ─────────────┤
                ├──► 03 people ────────────┼──► 06 root ──► 07 cutover ──► 08 after
                ├──► 04 forge connector ───┤      ▲             ▲
                ├──► 05 runtime ───────────┴──────┘             │
                │    (fleet, worker, channel; the session)      │
                └──► 05s smith ─────────────────────────────────┘
                     (the agent, in smith's repository)
```

01 to 05 run in parallel once 00 is merged; 06 starts with its skeleton
once 01, 02, 03 and the first increment of 05 are in, and takes 04 and
the rest of 05 as they arrive. 05s replaces 05f and 05g; the cutover
needs it.

## 2. My assessment

### 2.1 How big it is

| Part (lines of code / of its worlds) | What the design does to it | Technique |
|---|---|---|
| root `temper-engine-domain` (11.4k / 6.9k) | rewritten: the item table, records composed into comments, the forge-backed restart go | built beside; cutover |
| `work` (2.7k / 2.9k) | becomes `tasks`, several times larger | built beside, porting its lifecycle |
| `plan` (4.5k / 3.4k) | splits between `tasks` and the forge's change procedure | built beside, porting its checks, wakes and change mechanics |
| `rules` (2.0k, step tests) | becomes `authority` | built beside, porting its answers and its landing sweep |
| `forge` (8.0k / 5.0k) | becomes the connector's `client`, without records, wiki or labels | built beside: what stays copied, then trimmed |
| new: `tasks`, `authority`, `people`, forge top, `change`, `issues` | — | built beside |
| `fleet` (3.2k / 2.4k) | kept: turns acknowledged, graces declared | extended |
| `brief`, `views`, `accounts` (2.3k, 2.2k, 0.5k) | kept: new sections, subjects named by task | extended; renamed after |
| `notes` (2.5k / 1.8k) | from the wiki to the store | converted after the cutover |
| worker, four crates (13.5k / 16.9k) | changed little: turns, transcripts, merges in progress, expected heads | extended |
| agent, four crates (15.3k / 19.1k), its providers, OAuth and fake LLM | moves to smith, made generic there: host tools, messages, waiting, result contracts, delivery, spend, turns | built beside, in smith's repository, from a copy; temper's frozen |
| `temper-channel` (12.1k, of it 5.7k tests) | link extended; payloads redone (2.7k) | second payload version beside the first |
| `temper-engine-protocol` (4.9k) | translation redone (1.7k); connections, credentials, OAuth kept | converted at the cutover |

Roughly 28k lines of code and 18k of worlds are replaced; the shared
crates extended come to about 50k, most of them touched lightly; the new
code (`tasks`, `authority`, `people`, the forge connector, the new root
and their worlds) will likely be as large as what it replaces, part of it
ported. It is a rewrite of the engine's domain, and an extension of
everything else.

### 2.2 What I recommend, and where it departs from "beside everywhere"

Building beside and adding without modifying fits most of this: the new
child domains, the forge connector, the root itself, and every extension
of the worker, the agent and the channel. Gradually converging the old
code fits only the shared crates, where the design adds and later takes
away.

It does not fit the engine's root, `work`, `plan`, `rules` and the
forge's records. Their core is exactly what the design removes: the item
table, records composed and split, the restart contract read back from
comments, hold codes, plans as typed graphs. Converging them step by step
would mean designing intermediate states nobody will ship (an item's
record in a store; a plan half made of tasks; a forge both database and
connector), each needing its own tests, each thrown away. The old engine
world's stories are told in terms the design drops (labels taken off,
records garbled, people acting on the forge), so keeping them green
through a hybrid would test behaviour that is going. So the old root is
strangled rather than converged: frozen beside the new one, kept green
while that is cheap, switched off in one cutover, deleted. The step-by-step
convergence you asked for happens where it pays: in the fleet, the
worker, the channel, the brief and the views, which extend first and
contract after (section 3).

The agent was to extend in place too, and its session did (05e). It now
leaves for smith, which speaks only the second version and smith's own
channel, while the system worlds need temper's agent on the first until
the cutover. So the agent is built beside as well, in smith's
repository, from a copy, and temper's is frozen (05s-smith.md,
section 1).

Keeping the old engine running is not free. It bites in four places, and
each step says how it is handled:

1. **The channel's payloads.** The old root speaks today's payloads
   (charters with plans and envelopes, outcomes with steps and tasks,
   snapshots); the new speaks the design's. The channel already agrees a
   version at the hello and says a version covers the payload schemas
   (`channel.md`, 4.5), so the new vocabulary is a second version beside
   the first, and the first goes after the cutover. That is two codecs for
   a while (step 05).
2. **Notes.** The old root keeps them in the wiki, the new one in the
   store; one crate cannot serve both without carrying two backings. The
   new root goes without notes (its `note` and `recall` answered
   unavailable), and notes move to the store after the cutover, when one
   root uses them (step 08).
3. **The test budgets.** Both engines' worlds run until the cutover, and
   the default suite was at 9.7 of its 15 seconds, the fuzzy one at 26.5
   of its 60, when last recorded (at `3ccdd2d`).
   Each new world gets an allotment (5.5), and the legacy worlds' fuzzy
   sweeps may be trimmed, since the code under them no longer changes.
   The budgets are not raised.
4. **The system worlds.** `tests/agent/domain` and `tests/worker/domain`
   run the real old root through the engine's protocol translation.
   They are the only end-to-end checks of engine, worker and agents, and
   they keep running it, unchanged, until step 07 moves them.

**The alternative I would consider** is retiring the old engine in step
00: deleting it and its worlds outright. That saves the second payload
version, the legacy rules and the overlap in the budgets. It costs the
two system worlds until step 06 has rebuilt enough to carry them, which
is the long part of the migration, while the worker and the agent are
changing under them. I recommend keeping the old engine, with an escape
hatch: if keeping it ever needs the old code bent rather than merely
left alone, step 07 is done early instead, with fewer stories carried
over, and what is not yet carried over is listed as owed.

### 2.3 Risks, taken first

- **Facts about Forgejo** (`forge.md`, section 20): the pull request
  update and how it answers a conflict, whether its push starts CI,
  listing a pull request's files and a comparison's, reading an Actions
  job's log, creating a branch at a commit, reading branch protection
  with temper's permission. The change procedure's lazy updates and
  resolutions rest on the first two. They are checked against Forgejo 15
  with the conformance tool in step 00, before step 04 builds on them.
  These are historical v15 observations. The target is Forgejo v16.0.5;
  its supported API supplies job logs (`domain/forge.md`, 20.1).
- **The durable-state convention:** how a child domain's changes become
  part of the root's one commit. Every new child depends on it, so it is
  fixed in step 00 (section 5.2), not discovered in step 06.
- **The design is provisional.** Every step will find gaps. A step
  records what it settles in the design document it changes, in the same
  branch, as the protocol build did, so the documents stay the brief.
- **The root's size.** The old root is the largest crate in the engine.
  The new one keeps less state (children own theirs) but routes more:
  commits, loads, restart, runs, nine tools, connectors. Step 06 splits it
  into increments that each tell some of its stories.

### 2.4 Against the order in the design's README

Its section 5 says the web and the store go before records in comments
can go. In the domain that order is not needed: the new root has a store
and a web from its first day, as boundaries its world fakes, and records
in comments go with the old root at the cutover, never having been
written by a protocol layer (their drafts are deleted in step 00).
Nothing runs temper outside the worlds yet, so nothing is lost meanwhile.
The store's and the web's protocol and io layers follow the cutover,
each from a revision of `docs/design/protocol.md` (step 08).

## 3. Techniques

### 3.1 Build beside

A new crate, with the name the design gives it, its own world, and no
edit to the code it replaces. What it takes from the old code (a
lifecycle, a check, a budget's accounting) is ported by copying and
adapting, with the commit saying where it came from; the old code is
never made to serve both.

### 3.2 Extend

A shared crate gains what the design adds, beside what it has:

- **a new variant** of an event or a request, which only the new root or
  the new payloads produce;
- **a new field** whose absence (`None`, an empty slice) is today's
  behaviour, so every existing caller passes it and behaves as before;
- **a new module or type** beside the old one, chosen by the caller.

Exhaustive matches make every new variant a build error at each site
that matches on it, and every new field one at each site that builds the
value. In kept code, the new arm is written in full. In legacy code, an
extension may make two edits and no others: the new arm, an ignore when
a peer could cause the cell, or `unreachable!` naming why when only
temper's own code could and the legacy engine never does
(programming-model.md, 5.4); and the new field's default where legacy
code builds the value. Anything more is a sign the extension should be a
new type instead.

### 3.3 Contract

After the cutover, a shared crate loses what only the old engine used:
variants, fields, modules, a payload version, a fake's wiki. Each
contraction is a deletion, with renames where a name was kept only for
the overlap (views' `Item`, the channel's version number). It runs the
whole gate like any change.

### 3.4 Legacy

From step 00 to step 07 the legacy crates and worlds:

- **change only** by the two edits of 3.2, by renames that a shared
  crate's rename forces, and by the trimming of their fuzzy sweeps'
  seeds when a budget needs room;
- **are not fixed** for behaviour the design drops; a legacy failure that
  a shared crate's extension causes is a bug in the extension;
- **are named as such:** `legacy` in their crate names and paths, so no
  reader, agent or graph query mistakes them for the design.

## 4. Names

### 4.1 Moved aside in step 00

| Today | Legacy name and path |
|---|---|
| `temper-engine-domain` | `temper-legacy-engine-domain`, `crates/temper-legacy-engine-domain` |
| `temper-engine-domain-work` | `temper-legacy-engine-domain-work` |
| `temper-engine-domain-plan` | `temper-legacy-engine-domain-plan` |
| `temper-engine-domain-rules` | `temper-legacy-engine-domain-rules` |
| `temper-engine-domain-forge` | `temper-legacy-engine-domain-forge` |
| `tests/engine/domain` (`temper-engine-domain-world`) | `tests/legacy/engine/domain` (`temper-legacy-engine-domain-world`) |
| `tests/engine/work` | `tests/legacy/engine/work` (`temper-legacy-engine-work-world`) |
| `tests/engine/plan` | `tests/legacy/engine/plan` (`temper-legacy-engine-plan-world`) |
| `tests/engine/forge` | `tests/legacy/engine/forge` (`temper-legacy-engine-forge-world`) |

Step 05s3 moves temper's agent aside the same way: its crates, its
providers and its fake LLM, and its worlds but the system world
(05s-smith.md, section 3).

Two of these names the design gives to new crates (the root, and the
forge connector's top) and two of the paths to new worlds (the engine's
and the forge connector's); the other five move so that everything
frozen is in one place. The move is mechanical (package names, paths,
`use` lines) and changes no behaviour.

**Why move the old rather than name the new temporarily:** the new code
is then written once, under the names the design and its citations use,
and the cutover ends in a deletion rather than a rename of new code.
If you would rather the old code were not touched at all, the other way
works too: the new root and forge top take temporary names
(`temper-next-engine-domain`, say) and step 07 renames them.

### 4.2 New

| Crate | World | Step |
|---|---|---|
| `temper-engine-domain-authority` | none: step tests (`authority.md`, section 11) | 01 |
| `temper-engine-domain-tasks` | `tests/engine/tasks`, `temper-engine-tasks-world` | 02 |
| `temper-engine-domain-people` | `tests/engine/people`, `temper-engine-people-world` | 03 |
| `temper-engine-domain-forge` (the connector's top) | `tests/engine/forge`, `temper-engine-forge-world` | 04 |
| `temper-engine-domain-forge-client` | the connector's world | 04 |
| `temper-engine-domain-forge-change` | step tests, and the connector's world | 04 |
| `temper-engine-domain-forge-issues` | step tests, and the connector's world | 04 |
| `temper-engine-domain` (the root) | `tests/engine/domain`, `temper-engine-domain-world` | 06 |

### 4.3 Documents

Step 00 moves `docs/design/next/domain/` to `docs/design/domain/`, its
final place, and gives today's `engine-domain.md`, `worker-domain.md`
and `agent-domain.md` a line saying what they now describe (the legacy
engine, and the worker and agent as built). New code cites the domain
documents by that path from its first line: `//! (domain/tasks.md,
section 4)`, so `domain/forge.md` (the connector) and `forge.md` (the
protocol layer, as code cites it today) are never confused. Citations in
kept code are repointed as each crate is touched, and the rest at step 08.

smith's design is in smith's repository, `docs/design/domain/`, its
first commit; temper's `agent.md` says what temper takes from smith and
fills in, and the design's README, 6.5, where the previous `agent.md`'s
sections went, for the citations that name them.

## 5. Conventions every step follows

### 5.1 A child domain's shape

As every child domain today (`temper-engine-domain-accounts` is the
smallest example): a `#![no_std]` step crate with the workspace's lints;
`boundary.rs` for its vocabulary, `domain.rs` for its state and entry
points, `limits.rs` for its limits and worst case, `facts.rs` for its
content-free facts, `tests.rs` for its step tests.

```rust
//! The tasks child domain of the temper engine's domain layer
//! (programming-model.md, 4.5; domain/tasks.md): the engine's hub. ...
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod boundary;
mod domain;
mod facts;
mod limits;
#[cfg(test)]
mod tests;

pub use boundary::{Event, Key, Request, Stored};
pub use domain::{Domain, fire, max_out, step};
pub use facts::Fact;
pub use limits::{Limits, worst_case};
```

Each entry point has the shape of programming-model.md, section 3:
`step(domain: &mut Domain, env: &Env<Limits>, event: Event, out: &mut
Queue<Request>)`, `fire` for its own deadlines, `MAX_OUT` or `max_out`
declared, `worst_case(limits) -> Option<u64>` counted from its
containers. Pure policy (`authority`, `change`, `issues`) is free
functions over values, called in place by its parent, as `plan` and
`rules` are today.

### 5.2 Durable state

The design says each decision is one commit of records written or
removed (`engine.md`, 5.1), and that each child domain writes its own
records (5.4). How a child's records reach the root's commit is not
said. This is the convention, fixed in step 00 and recorded in
`domain/engine.md`, section 5:

```rust
// In each child domain that keeps durable state: its boundary.rs.

/// A record this child domain keeps in the store (domain/engine.md, 5.4),
/// in its own terms. The root carries it in its commit; the store's
/// protocol layer encodes it, with a version per shape.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Stored {
    Task { number: u64, task: TaskRecord },
    Message { task: u64, sequence: u64, message: MessageRecord },
    // ...
}

/// Where one of its records lives, by its own key. Keys order the records,
/// and what is live sits in key ranges of its own (domain/engine.md, 5.5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Key {
    Task(u64),
    Message { task: u64, sequence: u64 },
    // ...
}

pub enum Request {
    // ...
    /// Part of the decision being made: keep `record` under its key.
    Save { record: Stored },
    /// Part of the decision being made: remove what `key` holds.
    Erase { key: Key },
    /// Load what `range` holds, a page at a time. Ended by exactly one
    /// `Loaded`.
    Load { owner: Token, range: Range },
}

pub enum Event {
    // ...
    /// One of its records, as the store kept it, at a restart.
    Restore { record: Stored },
    /// Every record of its live ranges is restored: it decides from here.
    Restored,
    /// Terminal for `Load`: a page of rows, and whether more follow.
    Loaded { owner: Token, rows: Box<[Stored]>, more: bool },
}
```

- **A child never waits for a commit.** `Save` and `Erase` have no
  terminal event: they are part of the decision its parent is making.
  Its state changes as it decides (`engine.md`, 5.1), and the next
  decision sees it.
- **A child never holds an output back.** What it sends outward (an
  assignment, an answer, an effect to make) it emits as it decides; the
  root holds it until the commit it follows from is durable (5.3).
- **Its world plays the store** as its parent: it keeps what was saved,
  commits it at the end of each step, and restarts the child from what
  was durable by feeding `Restore` rows, then `Restored`. So a child's
  restart is tested in its own world, before the root exists.

### 5.3 Outputs that wait for a commit

The root closes each decision before its entry point returns
(`engine.md`, 4): the writes its children made become one `Commit`,
numbered, and every outward request the step produced is held until that
number, or the last one made if the step made none, is durable
(`engine.md`, 5.2). Held requests are released in order, through the
root's ready list, within its `MAX_OUT`, never all at once in the step
that hears the store. Step 06 builds this; the children only follow 5.2.

### 5.4 Worlds

A child domain's world follows `tests/legacy/engine/work`, the frozen template:

```
tests/engine/tasks/
├── Cargo.toml              temper-engine-tasks-world: skein-lib, the domain crate, temper-world;
│                           ordinary Rust, its own [lints] table as the work world's
├── src/lib.rs              what the world plays and checks, in its module doc; re-exports
├── src/world.rs            Settings (seed, limits, per mille faults, spans), Settings::calm,
│                           Settings::random, World::new, run, stats, trace, ENDINGS
├── src/referee.rs          Seen, Name, Stimulus; impl temper_world::Expectations
└── tests/
    ├── simulation.rs       the stories, each on a few seeds; replay (assert_replays)
    ├── referee.rs          the referee fails what it must: one test per safety rule
    ├── memory.rs           the domain at its limits, its heap's peak under its worst case
    └── fuzzy_simulation.rs random worlds settled; every ending reached across the sweep
```

- **The world is the parent.** It scripts the root and the neighbours
  through the parent (authority's answers, executors, connectors' news,
  the store), from the seed, and checks the contracts as it goes: one
  terminal per request, one reply per `ReplyTo`.
- **The referee watches from outside** (testing-strategy.md, section 7):
  what the scripted neighbours saw, and the facts the domain emits, never
  its state. Safety on every observation, liveness as deadlines.
- **Restarts** are the referee's stimuli, as in today's work world, from
  what the world committed (5.2).
- **Pure policy** is tested by step tests in its crate, with sweeps of
  generated values against an independent statement of the rule, as
  `rules` is today.

### 5.5 Budgets

Measured on 2026-10-04 at `95e9bfd`, before the mechanical legacy rename,
with no other build running. The enforced profiles passed: **1,772 focused
tests in 5.758 seconds** (15-second cap), and **26 fuzzy tests in 18.223
seconds**, with one ignored finding (60-second cap). The serial `measure`
runs took 19.348 seconds focused and 41.076 seconds fuzzy. These are test
execution times; compilation is outside the budgets.

The following totals sum nextest's per-test durations from the serial
runs (`--profile measure -j 1`, adding `--ignore-default-filter -E
'binary(/^fuzzy_/)'` for fuzzy). Durations are rounded to milliseconds,
so short tests and process overhead account for the difference from wall
time. Names here are the measured pre-migration names; the four legacy
worlds receive their renamed packages in 00c without changing their tests.

| World package | Focused seconds | Fuzzy seconds |
|---|---:|---:|
| `temper-agent-domain-world` | 1.576 | 15.548 |
| `temper-agent-protocol-world` | 0.174 | 0.027 |
| `temper-agent-run-world` | 0.501 | 0.289 |
| `temper-agent-session-world` | 2.017 | 0.625 |
| `temper-agent-tools-world` | 0.633 | 2.717 |
| `temper-channel-world` | 0.068 | 0.107 |
| `temper-engine-accounts-world` | 0.037 | 0.013 |
| `temper-engine-brief-world` | 0.190 | 0.722 |
| `temper-engine-domain-world` | 1.417 | 6.475 |
| `temper-engine-fleet-world` | 0.259 | 0.526 |
| `temper-engine-forge-world` | 1.286 | 2.625 |
| `temper-engine-notes-world` | 0.110 | 0.189 |
| `temper-engine-plan-world` | 0.152 | 1.458 |
| `temper-engine-protocol-world` | 0.150 | 0.109 |
| `temper-engine-views-world` | 0.317 | 0.255 |
| `temper-engine-work-world` | 0.274 | 0.455 |
| `temper-fake-forge-protocol-world` | 0.014 | 0.000 |
| `temper-fake-forge-tests` | 0.004 | 0.000 |
| `temper-fake-llm-tests` | 0.012 | 0.000 |
| `temper-forge-forgejo-world` | 0.084 | 0.000 |
| `temper-worker-agent-world` | 0.116 | 0.446 |
| `temper-worker-checkout-world` | 0.457 | 0.763 |
| `temper-worker-domain-world` | 3.979 | 7.106 |
| `temper-worker-host-world` | 0.074 | 0.617 |
| `temper-worker-protocol-world` | 0.125 | 0.000 |
| `temper-world` | 0.059 | 0.000 |

Other crates' unit tests total 5.123 seconds focused and 0.000
seconds fuzzy in the serial runs.

The initial allotments below preserve room for shared runtime extensions
and process overhead. They apply to the sum of a new world's tests when
measured alone with the serial profile; stories use one to three seeds.
Authority has step tests only.

| New tests | Focused seconds | Fuzzy seconds |
|---|---:|---:|
| authority step tests | 0.2 | — |
| tasks world | 0.5 | 4 |
| people world | 0.5 | 4 |
| forge connector world | 0.5 | 4 |
| new root world | 0.5 | 4 |

The agent's worlds are copied into smith (05s2) and measured against
smith's own budgets; temper's legacy copies keep their shares above until
07d removes them.

These allotments add at most 2.2 focused and 16 fuzzy seconds of serial
work to the overlap. They are targets, not proof that concurrent suites
fit: every code increment still runs both enforced profiles in full.
When a step's gate finds a budget broken, the step makes its own tests
cheaper first, then trims the legacy worlds' fuzzy seed counts (3.4),
and says so in its commit. The budgets in `.config/nextest.toml` are not
raised.

### 5.6 Increments and the gate

Each step is a series of increments. Each increment is a branch, rebased
on main, that passes the four checks of `docs/development/workflow.md`
before it merges with `--ff-only`, and leaves main whole: new crates are
members of the workspace with their tests, extensions are used by their
own tests at least, nothing is half wired. An increment that changes
only Markdown skips the checks, as the workflow says.

## 6. The steps

| Step | What | Depends on | Old code touched |
|---|---|---|---|
| 00 [groundwork](00-groundwork.md) | Forgejo facts checked; protocol drafts deleted; legacy moved aside; documents moved; durable-state convention recorded; budgets measured | — | renames only; dead drafts deleted |
| 01 [authority](01-authority.md) | `temper-engine-domain-authority` | 00 | none |
| 02 [tasks](02-tasks.md) | `temper-engine-domain-tasks`, `tests/engine/tasks` | 00 | none |
| 03 [people](03-people.md) | `temper-engine-domain-people`, `tests/engine/people` | 00 | none |
| 04 [forge connector](04-forge-connector.md) | the connector's four crates, `tests/engine/forge`; the fake forge grown | 00 | the fake forge, extended |
| 05 [runtime](05-runtime.md) | fleet, worker and channel extended; the session (05e); payload version 2 | 00 | extended; legacy match arms |
| 05s [smith](05s-smith.md) | the agent copied into smith's repository and made generic there; temper's moved aside; the channel's agent hop to smith | 05e; README.md, section 8's gates | renames only |
| 06 [root](06-root.md) | `temper-engine-domain`, `tests/engine/domain`; brief and views extended | 01, 02, 03, 05a; then 04, 05 | extended |
| 07 [cutover](07-cutover.md) | engine protocol converted, writing smith's charters; system worlds moved, on smith's agent; legacy deleted | 06, 05s | the engine protocol; the system worlds; legacy deleted |
| 08 [after](08-after.md) | notes into the store; contractions; what comes below the domain | 07 | contracted |

## 7. Where each owed change lands

What the design's README, section 5, says the other documents owe, and
the step that does it:

| Owed by | Change | Step |
|---|---|---|
| `protocol.md` | the store as a boundary: commits, loads, sized records, secret records | 06 (domain face), 08 (protocol plan) |
| | the web first | 06 (domain face), 08 (protocol plan) |
| | runs named by task | 06, 07 |
| | forge comments shrink to keys; transcripts for snapshots | 04, 05, 08 |
| `channel.md` | turns up, kept until acknowledged; spend; graces at the hello | 05 |
| | acknowledgements of turns; transcripts in assignments; merges in progress; expected heads | 05 |
| | messages carried whole and named | 05 |
| | relayed calls: engine tools and connectors' reads | 05 |
| | outcomes: change, verdict, report, failure | 05 |
| | the charter: instructions, tools, result contract, budget and prices, waiting time | 05 |
| | names by task; no repository packing | 05 (wire), 07 (engine translation) |
| | snapshots go; a versioned turn payload | 05 (beside), 08 (gone) |
| | caps and sizes; its section 14 rewritten | 05 |
| smith | the agent's domain, protocol, providers and channel hop to smith's repository; temper's half in the engine's protocol layer | 05s; 07a |
| `credentials.md` | the refresh token in the store's secret records; the web's OAuth client; credentials per adopted repository | 08 (follow-on plan) |
| `llm.md` | to smith's protocol design: `finish` with the result contract; turns encoded with their names; completions priced; host tools' schemas passed through | 05s5 |
| | temper's half: schemas, decoding and rendering for every engine tool and connectors' reads | 07a |
| `docs/design/forge.md` | records, outcome blocks, the wiki, nonces, the person in a marker go | 00 (drafts), 08 (the rest) |
| | the new calls | 04 (fake), 08 (Forgejo protocol plan) |
| | repositories adopted at runtime, on several forges | 04 (domain), 08 (protocol plan) |
| `docs/design/testing.md` | the engine's world and its fake store; new worlds; the fake forge and checkout grown; restarts at cuts; the referee of `core.md`, section 10 | 02 to 06; rewritten at 07 |
| `docs/design/performance.md` | turns' bytes; transcripts; the working set of tasks | 05, 06; rewritten at 07 |
| `docs/development/protocol-implementation.md` | the paused forge increment's drafts deleted; settled decisions | 00 |
| citations | about two hundred, repointed | as each crate moves; the rest at 08 |

Step 08 writes the separate plans for work below the domain, as
08-after.md, section 3, specifies. The protocol-plan entries above name
those plans and the design revisions they require, not implementation of
the lower layers during this migration. Its notes and contractions are
implemented before those plans are written.

## 8. Current order and independent review

Finish the full charged 06a walking story before adding depth in 02d–e,
04b–f or the agent's runtime (05f–g, now 05s). Only the finite funding and atomic admission slice of 02e
needed by that story comes forward. Already-started deeper work stays on
its branch. Once the story passes, audit every tasks event and request
against an actual root route; merge or remove unused variants and put root
mechanics in the root unless `domain/tasks.md` assigns them to the hub.
Record the resulting contracts in `domain/engine.md` and `domain/tasks.md`
in the same branch. Resume paused increments in the plan's order only
after the walking story, tasks audit, documentation backfill and five
style checks have passed. The agent's runtime then resumes as step 05s,
in smith's repository: 05f and 05g are ported into it, not merged.

Every increment keeps the existing independent review and four-check gate.
Review rejects undocumented new public items. Its checklist includes:

- Module docs name the child's kept state, what it never knows, entry
  points and contracts. Public boundary types, variants and fields document
  their sender, contract, terminal event and applicable bounds.
- Re-export names explicitly; no `pub use module::*`.
- Name the tasks result `Outcome` or `TaskResult`, preserving the standard
  `Result` name for fallible operations.
- Use descriptive parameters such as `domain` and `limits`.
- Separate items with blank lines, matching the existing crates.
- Cite `domain/<file>.md` and its section throughout new code.

Backfill authority, tasks, people and the root in one doc-comments-only
increment, through the gate. Companion code-style changes stay separately
reviewable in the same pass. Before 05g, confirm payload and wire golden
fixtures have a documented code regeneration command and drift tests.
