# Testing the client's domain

Provisional, 2026-10-05. How the client domain and the view are tested
below the protocol: the person, as a step machine with a tree face; the
client's own world, with a scripted engine; the joint world, with the
real engine root; what their referees watch; the faults; the budgets.
`architecture.md`, section 7 is the design, skein's
`testing-strategy.md` the strategy, `docs/design/testing.md` how temper
applies it. Step tests are in 01-domain.md, section 10 and 02-view.md,
section 10. Overview: README.md.

## 1. In one page

- **The person acts on the client,** as a person does: finds nodes in the
  view's tree by role, accessible name and text, presses and types into
  them, and waits to see what follows (`architecture.md`, 7.2). Never by
  class, by position or through the client's state.
- **One person, every tier.** The person is a step machine that asks its
  face to find and act, so the same scenario runs on the tree face now
  and, later, on the DOM face in the browser tier, unchanged.
- **The client's world comes first.** The real domain and view, a scripted
  engine that speaks the whole web of `ux/`, the shell's part played by
  the world (clock, seed, session storage, the address bar, reloads), and
  every fault of `architecture.md`, 7.1. Most of the web's tests live
  there, and none waits on the engine's routes.
- **The joint world is the check.** The same person and scenarios against
  the real engine root, translated as the protocol layer would, for the
  stories the root's routes can tell, more as they land.
- **The referee watches what the person sees and what the engine made
  durable,** never the client's state: nothing shown as done before it
  is durable, nothing made twice, nothing typed lost.
- **Every world builds and diffs every page,** and applies the patches to
  a model of the last tree, which must then equal the new one.

## 2. The person

`testing/temper-fake-person`, a step machine under the workspace's step
lints, as the fake forge's domain is, so the browser tier can run it in
its loop later.

```rust
/// What a person looks for, outermost region first: "the button named
/// Release task, in the card named T27 Memory-safety review".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Find {
    pub within: Box<[Target]>,
    pub target: Target,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Role { role: Role, name: Box<[u8]> },        // the accessible name contains these words
    Text { text: Box<[u8]> },                    // a node whose text contains these words
}

/// A scenario's person, a step at a time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Step {
    Go { address: Address },                     // typed into the address bar
    Press { find: Find },
    Type { find: Find, words: Box<[u8]> },
    Send { find: Find },                         // Enter in a composer
    See { find: Find, within: Duration },        // waits until it is there; fails past the time
    Gone { find: Find, within: Duration },
    Reload,
    Pause { span: Duration },
}

#[derive(Debug)]
pub struct Person {
    script: Box<[Step]>,
    at: u32,
    since: Option<Time>,                         // when the current step began
    asking: Option<Ask>,                         // a query its face has not answered yet
}

/// What the person does next. The tree face answers at once; the DOM face,
/// later, through the browser's accessibility tree, a round trip later.
pub fn next(person: &mut Person, face: &mut Face, now: Time) -> Next;

#[derive(PartialEq, Eq, Debug)]
pub enum Next {
    Do(Doing),
    Wait { until: Option<Time> },                // nothing to do until the tree changes or this time
    Done,
    Failed { step: u32, why: Why },              // a See past its time, a Press with nothing to press
}

#[derive(PartialEq, Eq, Debug)]
pub enum Doing {
    Press { node: NodeId },
    Type { node: NodeId, words: Box<[u8]> },
    Send { node: NodeId },
    Go { address: Address },
    Reload,
}
```

- **The tree face** (`tree.rs`) finds a `Find` in `temper_web_view::Tree`
  by role, name and text, the way an accessibility tree is searched, and
  turns a `Doing` into the `DomEvent` the shell would report: a `Type` is
  one `Input` with the whole text, as the browser reports a field's
  value.
- **A random person** (`random.rs`), for the fuzzy suite: presses any bound
  node and types any words drawn from the seed, so a sweep reaches what no
  script names.
- **The DOM face** comes with the browser tier, as a crate of its own over
  `skein-browser`, so that only the browser tier depends on it.

## 3. The client's world

`tests/web/domain`, package `temper-web-domain-world`, after the frozen
template of `docs/plans/next-domain/README.md`, 5.4.

```
tests/web/domain/
├── Cargo.toml          skein-lib, temper-world, temper-web-domain, temper-web-view, temper-fake-person;
│                       ordinary Rust, with its own [lints] table as the other worlds
├── src/lib.rs          what the world plays and checks, in its module doc
├── src/world.rs        Settings (seed, limits, faults per mille, spans), Settings::calm, Settings::random;
│                       World::new, run, stats, trace; ENDINGS
├── src/tab.rs          one tab: its domain, its view, its session storage, its address history
├── src/engine.rs       the scripted engine
├── src/scenario.rs     setting a scenario up: people, projects, tasks, what runs say and when, faults
├── src/referee.rs      Seen, Name, Stimulus; impl temper_world::Expectations
└── tests/
    ├── slice.rs        signing in; starting a chat; a chat's page; a held chat decided      W3, W4
    ├── faults.rs       drops, restarts, reloads, double presses, two tabs, stale presses     W3, W4
    ├── inbox.rs        W6
    ├── conversation.rs W7
    ├── task.rs         W8
    ├── board.rs        W9
    ├── changes.rs      W10
    ├── referee.rs      the referee fails what it must: one test per rule
    ├── memory.rs       the domain and the view at their limits, the heap's peak under their worst cases
    └── fuzzy_web.rs    random persons over random scenarios, every fault; every ending reached
```

### 3.1 What the world plays

- **The shell, for each tab** (`tab.rs`): the clock (the world's, read once
  per iteration), the seed (each tab its own, so their keys differ),
  session storage (a `Saved` kept as it was last written), the address
  history (pushed, replaced, back and forward), and the client's own
  iteration: the domain's step for each event, then the view's render.
  `SignIn` is played as the round trip would be: the world signs the
  person in at the engine and reloads the tab at `then`.
- **A reload** drops the tab's domain and view and starts new ones from
  `Start` with the address and what session storage kept, a fresh seed
  included, as a reloaded page draws one.
- **The model DOM.** Each render's patches are applied to a model of the
  last tree; it must equal the new tree, node by node, ids included. Every
  story checks the diff this way, at no cost to write.
- **The engine** (`engine.rs`), scripted, speaking the client's vocabulary
  directly, as a script in one world may (`testing-strategy.md`, section
  4). It holds what a scenario sets up: people and their sign-ins,
  projects, tasks with their phases, results and transcripts, proposals,
  escalations, questions, person tasks, changes; and it runs the
  scenario's timeline of what happens engine-side ("at 2 s T0's run
  streams a reply and commits it; at 3 s T0 proposes T1").
  - **Keyed requests** are decided once per person and key, and their
    answers kept; a decision is volatile until a drawn latency makes it
    durable, and a restart before then loses it with its key, as the real
    engine's would (`domain/people.md`, 5.1.1).
  - **Watches** start with a snapshot, then changes; a slow watcher's
    deliveries are dropped and it is told `Missed`.
  - **It checks its client:** a key reused for another ask, a decision on a
    revision the client never saw, a request while signed out, a watch
    left open by a page the client left, each fails the world.
  - **Its facts** go to the referee: what it made durable, by key; what it
    refused; what it streamed and what it committed.

### 3.2 Faults

From `architecture.md`, 7.1, drawn from the seed, each with a per mille
in `Settings`:

| Fault | Played as |
|---|---|
| a stream drops | the engine ends a watch `Dropped`: the page falls behind, then live |
| a slow watcher | deliveries dropped, the watch told `Missed`: the page reloads its snapshot |
| the engine restarts | watches dropped, answers in flight `Unreachable`, volatile decisions lost; offline for a span, then answers by key |
| a busy engine | an answer `Busy`: sent again with the same key |
| a double press | the person's press made twice within a few milliseconds |
| a reload | the tab's client restarted, its state gone but for session storage |
| two tabs | two clients for one person, deciding one thing |
| a stale press | a press on a node the person found in a tree since replaced |
| signed out | the sign-in expires; the next answer or watch says so |
| latency | every answer, read and delivery after a drawn span |

As `testing-strategy.md`, section 3 asks, the fuzzy sweep asserts that each
fault fell at least once, and that each outcome a race allows (an answer
before the watch's change, and after) appeared.

## 4. The referee

The strategy's referee (`testing-strategy.md`, section 7), in the world's
loop. It sees two things: **what the person sees**, read from each tab's
tree after each render as the person reads it (cards by their regions and
states, words and their states, the link's state, notices); and **what
the engine made durable**, from its facts. Never a domain's state.

**Safety,** on every observation:

1. **Never done before durable.** A card shows its decision, a word shows
   as sent, a page shows a new chat only after the engine made it durable.
2. **Made once.** No key is made twice; one confirmed press makes at most
   one durable decision, across double presses, reloads, restarts and the
   other tab.
3. **Decided once, told twice.** When two tabs decide one thing, one
   decision is durable and the other tab shows who decided and how.
4. **Nothing typed is lost.** What was in a composer or a reason and not
   sent is there after a reload.
5. **Provisional is replaced.** No tab shows a streamed turn beside the
   committed turn it became, nor streamed text after its run was stopped.
6. **Refusals say why.** A refused request shows its refusal on its card or
   as a notice, never nothing.
7. **The link tells the truth.** A tab shows live only while each watch it
   follows is live.

**Liveness,** as deadlines armed on the referee's own table:

1. Every confirmed press is answered on screen (decided, refused, or
   decided by another) within a span of the engine being reachable.
2. A dropped or missed watch is live again within a span of the engine
   being reachable.
3. A card decided anywhere leaves every tab within its linger and a
   delivery's span.

**Stimuli** it injects, as no fake owns them: restarts of the engine,
reloads of a tab, the sign-in expiring.

`tests/referee.rs` breaks each rule on purpose, one test each, as every
world's referee tests do.

## 5. The joint world

`tests/web/engine`, package `temper-web-engine-world`: the same person,
scenarios and referee rules, with the real engine root
(`temper-engine-domain` and its children) in place of the scripted engine.
It takes from `temper-engine-domain-world`'s library what the engine's
own world built: the paged fake store and its commits, restart cuts, the
scripted worker, and the drivers that hold a chat for out of tries.

- **It translates** as the protocol layers will, with small total
  functions in `translate.rs`: a client `Send` to the root's
  `Event::Ask`, its `Key` to the root's key, its `Ask` to
  `temper-engine-domain-people`'s; the root's `WebReply` to an `Answered`;
  the client's reads to `ReadEscalation` and `ReadResult`; `SignIn` to
  `Event::SignedIn`, keeping the sign-in number per tab as the protocol
  layer keeps a cookie. A request or watch the root has no route for is
  answered `Refused(Unknown)`, which the client shows as it would any
  refusal.
- **Its stories are the first slice** (W5): signing in; starting a chat,
  answered with its number once durable; a held chat's escalation read and
  released, or left held with a reason; two tabs deciding it; an engine
  restart before and after durability. It adds each later increment's
  stories as the root gains their routes (04-increments.md lists them).
- **It is where misreadings show.** The client and its scripted engine are
  written together; a story that passes there and fails here is a
  misreading of the engine, fixed where the engine's design says it lies.

## 6. What every world checks

The harness's checks of `testing-strategy.md`, section 6, with
`temper-world` as every temper world uses it:

- **contracts as it goes:** one terminal per `Send`, `Read` and `Open`; no
  terminal for a slot the client did not ask for; `max_out` honoured by
  every step; patches within the view's `limits.patches`;
- **invariants once it settles:** no request pending, no read in flight,
  only the frame's and the page's watches open, no card deciding, no
  notice past its deadline, each tab's model DOM equal to its tree;
- **memory:** `memory.rs` drives the client to its limits (every slab and
  window full, every text at its most bytes) and checks the counting
  allocator's peak under `temper_web_domain::worst_case` plus
  `temper_web_view::worst_case`;
- **replay:** a seed replays to the same trace (`assert_replays`);
- **transition coverage:** the handlers of each state table (keyed
  requests, watches, cards) are functions, so coverage lists the cells a
  run never reached.

## 7. Suites and budgets

- **Focused** (`slice.rs`, `faults.rs`, a file per page, `referee.rs`,
  `memory.rs`): each story on one to three seeds that show it; replay on
  one. Within the client's world's 0.5 seconds and the joint world's 0.3
  (README.md, 6.1).
- **Fuzzy** (`fuzzy_web.rs`, and the joint world's `fuzzy_slice.rs`):
  random persons over random scenarios with every fault, settled under the
  invariants, every ending of `ENDINGS` reached across the sweep. Within 4
  and 3 seconds. A failing seed is fixed and kept, as a story or a pinned
  seed (`docs/development/workflow.md`, section 3).

W3 measures the client's world before a second page's stories join it,
and sets the number of seeds each story runs on from what it finds.

## 8. Where this goes in `docs/design/testing.md`

W3 records it there: section 4.4 says people act on the client through
the person's tree face, with `tests/web/domain` and `tests/web/engine`;
section 7's layout gains:

```
testing/temper-fake-person      the person: scripts, the tree face (a DOM face later)
tests/web/domain                the client's world: the domain and the view, a scripted engine
tests/web/engine                the client with the real engine root
```

## 9. The tiers above

Not in this plan, and kept possible by it: the protocol worlds (the
client's protocol layer on the native shell, against the engine's, through
bytes), the real loop and the browser tier (`architecture.md`, 7.1 and
7.3). Each runs the same scenarios with the same referee; only the
person's face and what sits below the client change.
