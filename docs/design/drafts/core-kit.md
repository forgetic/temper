# temper's core as a kit

Draft, 2026-10-06. This records the direction agreed in a design discussion
on 2026-10-06. temper's core (`domain/core.md`) becomes a kit that
applications build around, and temper is its first application. Until the
draft is adopted, `docs/design/domain/` describes what is built, and this
document changes nothing there. The mechanics are those of skein's
`docs/foundation/programming-model.md`. Section 12 lists what is still open.

## 1. In one page

- **A kit, not a framework.** These parts become crates that applications
  build around:
  - the generic engine of `domain/core.md`: the five primitives and the
    promises of its section 10;
  - the store's records for that engine;
  - the worker that hosts smith's agents;
  - a client over the primitives.

  skein is the same kind of kit for io and protocols, and smith for agents.
  Applications call the kit; the kit never calls them.
- **The application owns the root.** skein's model is step machines nested
  inside one another, and the top one is always the application's. It
  composes the kit's machines with its own connectors and domain logic,
  and translates between them (programming-model.md, 4.5). Extension is
  composition: no traits, no generic items, no callbacks.
- **The kit gives a mid-level core.** The core is the parent of tasks,
  authority, people, fleet, accounts, brief, notes and views, and keeps
  what today's root keeps of them. The root's boilerplate grows with the
  application's connectors, not with the kit's internals.
- **Boilerplate is accepted; what it carries is not.** The root routes and
  translates, and never decides or orders. The kit's promises must not
  depend on every application writing its root correctly.
- **Rails keep that true** (section 5):
  - the commit barrier is a generic container in skein-lib, and the root's
    only way out;
  - the core orders restart and effects as a script;
  - every part declares its worst cases, and the barrier asserts them;
  - a conformance world crashes the application's domain at every commit;
  - a reference root is the template every application starts from.
- **Connectors meet the core through one vocabulary:** the one that
  `domain/connectors.md` section 2 tabulates, which becomes the kit's API.
  The core does not hold what it does not act on. It names that by task
  number and connector, and the application's children keep it.
- **The store, the worker and the client split the same way.** The kit
  has its own records, its worker host and its client domain. The
  application adds its own record families, kinds of workspace and kinds
  of object, and composes them in its protocol layers and roots.
- **The forge is cut out of the core first, inside temper's
  workspace.** The boundary is checked by the crate graph and by a
  second application that is not about software. The kit moves to its
  own repository once the boundary stops moving.

## 2. Why a kit

- **The core is already generic by design.** `domain/core.md` describes an
  engine on five primitives whose tasks, inboxes and authority know no
  connector (1 and 3.5). Other applications want the same engine with
  other connectors. Some are temper's own later pipeline, such as test
  environments and deployments (`domain/connectors.md`, section 14).
  Others are products that are not about software at all.
- **The precedent is skein and smith.** skein is the io and protocol kit,
  pull-driven, with temper as its first user. smith is the agent kit,
  which has already left temper (`domain/agent.md`).
- **What the kit offers** is what is hard to get right: work that outlives
  processes, authority that only narrows, effects made once, nothing
  leaving before its commit, and bounded memory. The kit's referee checks
  these promises in the application's own worlds (5.6).
- **The kit favours flexibility and orthogonality over convenience.** An
  application writes some boring code to wire the kit in. That is fine
  as long as the code is easy and predictable, and as long as getting it
  wrong cannot break a promise silently. Agents write such code well.

## 3. The application owns the root

```
application root    routing between its children, translation, its store, channel and web vocabularies
├── core            the kit: tasks, authority, people, fleet, accounts, brief, notes, views, and their routing
├── forge           a connector (temper's)
├── …               the application's other connectors
└── …               the application's own child domains, if it has any
```

- **Control.** The application chooses:
  - its connectors;
  - what it adds around the core;
  - its limits;
  - its entrances (the store, the workers' channel, the web, each
    system's protocol).

  The core is the top of its own subtree, as a connector is.
- **Composition only.** Step code may not define traits or generic items,
  and may not use `dyn` or closures (programming-model.md, 10.3). So the
  root matches on what the core and each connector ask for, and calls
  the one each request goes to. Matches are exhaustive with no `_` arm.
  A new variant in the kit therefore breaks every application's build in
  one place: a change in the kit reaches an application on purpose,
  through a new pin.
- **What the core never knows:** any connector's types, the application's
  records, and any wire format.
- **An escape hatch.** The core's children stay public crates. An
  application may compose them itself, without the core. If it does, it
  owns the invariants the core keeps, and the conformance world (5.6)
  still applies to it.

## 4. The core

- **What it holds:** everything today's root (`temper-engine-domain`)
  holds that is not about a connector:
  - the routing among its children (`domain/engine.md`, section 4's
    table, without the connectors' rows);
  - the deployment's numbers;
  - the runs in flight, and the calls committed for each;
  - the brief's gathering;
  - the restart script (5.4).
- **Its vocabulary toward the root:**
  - the store: its own records, saved, erased, loaded and restored
    (`domain/engine.md`, 5.6);
  - the workers' channel, the engine's half;
  - people's requests and their replies;
  - LLM accounts' refreshes;
  - the connectors' vocabulary (section 6).
- **Connectors are numbered.** The core knows from its configuration that
  connector `k` exists, and that connector `k`'s resources are paths
  under its prefix. It never sees a connector's own types.

## 5. What the root carries, and the rails

### 5.1 The barrier

A child domain decides immediately. Its state changes as it decides, and
it emits its writes and its outward requests at once; it never waits for
durability (`domain/engine.md`, 5.6). Durability is its parent's job. For
each call to its entry point, today's root does these things:

1. **Admission:** before any child changes, it checks that there is room
   for one more commit. If there is not, it answers busy.
2. **Gathering:** every write from every child the event touched goes
   into one numbered commit.
3. **Holding:** every output that someone outside could act on is tagged
   with that commit. That covers an effect, an assignment, an
   acknowledgement, and an answer to a person or to a tool call. The
   output is held until the store says the commit is durable.
4. **Releasing:** held outputs leave in their order. After a failed
   commit, nothing leaves, and the engine stops.

Restart order (`domain/engine.md`, section 6) and the authority check on
every effect belong to the same set of duties.

Together, these make "once" and "nothing ahead of its commit" true. Once
connectors are the application's children, their writes and their
outputs pass through the application's root, so that root is on this path
whatever we do. Its mistakes pass every test of expected behaviour and
show only when the engine crashes:

- **An output that leaves early:** a push that goes out before the commit
  of its outbox entry, made but never recorded;
- **A decision split in two:** a call's record in one commit and its
  outbox entry in the next;
- **Admission after a change:** a connector changed, then no room found
  to commit what it changed;
- **Restart out of order:** the core deciding before a connector has
  restored its outbox, and asking again for an effect already in flight;
- **An effect that skips authority:** a procedure's effect reaching its
  connector's outbox without the core's check.

### 5.2 The journal, in skein-lib

The barrier cannot live in the core: the core may not hold the
application's records and outputs, since its code may not be generic. It
is not policy either. Today's `Journal` (`crates/temper-engine-domain/src/decision.rs`)
"never knows child policy, worker placement, store encoding or IO". It
is a data structure, as `Queue` is. So it goes into skein-lib, generic
over the writes and outputs it holds, as lib's containers are generic
over what they hold. This follows programming-model.md, 10.2: lib may
use generic types, and application code does not hand-roll data
structures.

- **What it does:**
  - **admission by counts:** commits in flight, writes per commit,
    outputs held;
  - **a decision:** a value that gathers one decision's writes and
    outputs; it is `#[must_use]` and not `Clone`;
  - **at most one numbered commit per decision;**
  - **tags:** each output carries the commit it follows, or the last
    commit made if the decision made none;
  - **release:** outputs leave in order as the store answers, within a
    bound per step;
  - **a failed commit:** nothing after it is released, and the journal
    says stop;
  - **its worst case,** priced like every lib container.
- **What it does not do:** inspect what it holds. Whoever owns the bytes
  checks byte bounds before handing them over. The deployment's record
  and numbers are the core's.
- **It keeps to the subset.** It uses generic types and lib's own
  containers, and nothing else of what lib alone may use. It defines no
  traits and takes no closures. skein's `docs/design/lib.md` gains a
  section for it.

### 5.3 One shape for every root

The root's entry points have no outbound queue of their own. What leaves
the domain is taken from the journal by the service's `iterate`, so the
journal is the only way out:

```
step(root, event):
    journal.takes(room for this event)?  else: busy     ← before any child changes
    d = journal.decision()
    route the event through the children: each write and each outward request goes into d
    journal.accept(d)                                   → at most one commit out
store answers n → journal.committed(n) → the outputs held for n leave, in order
```

Outputs that decide nothing leave through one explicit door of the
journal, untagged, so review finds every use. These are facts streamed
to watchers and answers to a run's read tools (`domain/engine.md`, 5.2).

### 5.4 Ordering belongs to the core

- **Restart is a script.** `domain/engine.md` section 6's five steps
  become the core's states. The core asks for one step at a time:
  - its own live ranges, loaded;
  - connector `k` restored, for which the root asks the connector for its
    live ranges, loads them and feeds them;
  - runs adopted;
  - connectors reading afresh;
  - the outbox settled;
  - decisions open.

  The root does the step it is asked for, and gets the next one only
  after reporting that this one is done.
- **Effects follow one loop:**
  1. a connector proposes an effect, from a procedure's step or from an
     agent's call;
  2. the core admits it against authority;
  3. the connector records the outbox entry, in the same decision;
  4. the connector makes the effect only when the journal releases it.
- **Write holds:** the core keeps holds and writer slots (in tasks). A
  connector tells the core when it has read a resource afresh after a
  lost attempt (`domain/connectors.md`, 3.3).

### 5.5 Mistakes are loud

- **Worst cases are declared.** Each child declares the most it writes
  and outputs per event, and the root adds them up along the route
  (programming-model.md, 4.5).
- **The journal asserts.** A decision that exceeds what it reserved stops
  the engine (fail-stop), rather than leaving state ahead of the store.

### 5.6 Mistakes are caught: the conformance world

- **What the kit ships:**
  - the fakes for its own peers: the store on skein-kv's in-memory mode
    (`docs/design/store/`), scripted workers and scripted people;
  - the referee for `domain/core.md` section 10;
  - the faults: crashes at every commit, commits held, commits failed.
- **What the application adds:** its whole domain and the fakes of its
  systems (for temper, the fake forge).
- **Generic in the world, concrete in the domain.** A world is ordinary
  Rust (programming-model.md, 10.2), so the conformance world may be
  generic over the application. The application's domain stays concrete.
- **The referee looks from outside.** Before a fake system saw any
  effect, that effect's record was durable. No run was assigned without
  a durable claim. No person was answered ahead of their commit. Every
  keyed effect was made once.

### 5.7 A reference root

The second application's root (section 11) is the template every root is
copied from. With exhaustive matches and one shape (5.3), roots look
alike, and agents fill in their arms.

### 5.8 What is left to the application

Which child an event goes to, and the small total functions between
vocabularies. A mistake there is an ordinary bug in the application, and
its own stories find it. It cannot break the kit's promises silently,
because every write and every output still passes through the journal,
and the conformance world crashes the application at every commit.

## 6. Connectors

- **The vocabulary is the kit's API.** `domain/connectors.md` section 2
  lists what crosses between the root and a connector. In the kit, each
  item becomes a generic term:
  - resources named as paths of bytes, with roles;
  - effect and read kinds as numbers, whose order authority holds as
    data (`domain/connectors.md`, 3.1);
  - effects keyed per connector;
  - topics, and news for subscribers, classified;
  - procedure steps and their decisions;
  - projections of a goal;
  - brief sections and reads;
  - adoption.
- **Requirements are verdicts.** Authority's rules for landing (CI at a
  head, approvals, gates) are the forge's facts. In the kit, the
  connector that makes an effect judges its own facts and gives a
  verdict: met, wait, or refuse with a reason. The core holds only one
  rule: no effect without its verdict.
- **What the core does not act on, it does not hold.** A connector keeps
  its own state for each task, keyed by the task's number
  (`domain/connectors.md`, section 2), and the root joins the two.
  Payloads the core orders but does not read are an open question
  (section 12): sections in a brief, answers to reads, and results under
  a connector's contract.
- **A missing place in the vocabulary is the kit's to add.** No root can
  add one. That is why the vocabulary needs a second application before
  it is settled.

## 7. The store

- **The kit's part:**
  - the core's records and keys (the core's rows of `domain/engine.md`,
    5.4);
  - their encoding, order-isomorphic and versioned (`docs/design/store/`);
  - transcripts as files;
  - the secret records: sign-ins, refresh tokens.
- **The application's part:**
  - its own record families: connectors' outboxes, procedures' states,
    what was made by key, and each connector's own state;
  - the root's `Key`, `Record` and `Write`, which wrap both the kit's
    records and its own;
  - the store's protocol layer, which composes the kit's encoding with
    its own.
- **One commit spans both,** through the journal. The live families of
  both are loaded by the core's restart script (5.4). skein-kv is
  underneath.

## 8. Workers and agents

- **smith stays the agent kit.** The worker host moves to the kit:
  - slots, attempts, turns and graces;
  - agent processes;
  - the engine's hop of the channel.
- **Workspaces are the application's.** A connector says which of its
  resources are prepared for a run (`domain/connectors.md`, section 9). A
  connector whose resources are not files gives a run tools instead. The
  worker's checkout child, git, is temper's. The worker's root is the
  application's too: it composes the kit's host with the application's
  kinds of workspace.
- **Host tools** are the engine's tools, which are the kit's, plus each
  connector's reads, which the application declares.

## 9. The client

- **The kit's part:**
  - the client domain over the primitives: tasks as a tree,
    conversations, a person's inbox, proposals, escalations, questions,
    person tasks, results and live runs, with the pending keyed requests,
    pages and streams (`docs/design/web/architecture.md`, section 3);
  - the wire documents for these;
  - their views;
  - the browser shell, and the native shell the worlds use;
  - the fake person.
- **The application's part:**
  - its own kinds of object (for temper: changes, gates, CI, landing
    queues) as children of its client root;
  - their wire documents and their views.

  Its client root composes them with the kit's client domain, under the
  same rule: the root routes, and the kit's client domain keeps the
  invariants of pending requests and pages.
- **Any client.** The domain and the view are step crates, built for the
  host and for wasm32. A new platform is a new shell, perhaps a new view.
  The domain and the protocol stay.
- **The web protocol** carries the kit's documents and the application's
  over one connection, composed in each end's protocol layer. Sign-in
  providers are the application's: for temper, the forge's OAuth.

## 10. What is still temper's in the core today

| Today | Where it goes |
|---|---|
| authority: landing requirements (CI at a head, approvals, gates) | a connector's verdict (section 6) |
| people: a person is a forge and a user there | a provider and a subject; providers are the application's |
| projects: repositories with roles and a home repository | resources with roles, from any connector; a home for projections |
| goals: an issue, and a priority that orders the landing queue | the priority stays in the core; the issue and the landing queue are the forge's |
| tasks: saved-work repository tags, and repository scope bits | resource tags and scopes named by connectors |
| fleet and worker: checkouts | kinds of workspace (section 8) |
| brief, notes, views: repository and pull request sections and scopes | supplied by connectors |

temper's charters (a coder's, a reviewer's) and its deployment's
configuration are temper's, as they are today.

## 11. Where it lives

- **First, inside temper's workspace.** The kit's crates sit in a
  directory of their own. The crate graph holds the boundary: no kit
  crate depends on an application's crate, and no kit crate is named for
  a system.
- **A second application, also in the tree,** that is not about software,
  for example a research assistant whose connector is a mailbox or a
  document store. It has its own root, connector, fakes and world. It is
  the forcing function for the vocabulary (section 6) and the reference
  root (5.7).
- **The journal goes to skein-lib** (5.2).
- **Its own repository** once the boundary stops moving, after the
  next-domain cutover (`docs/plans/next-domain/07-cutover.md`). temper
  then pins it, as it pins skein and smith.
- **How this fits the migration under way** is a revision of
  `docs/plans/next-domain/`. That is a follow-up, not this document's
  concern.

## 12. Open questions

- **The kit's name.**
- **Payloads the core orders but does not read:** brief sections, read
  answers, results under a connector's contract. Either the core holds
  each as a token with a declared size, and the root or the connector
  holds the value; or the value reaches the core already in the kit's
  terms. The brief's cutting within a byte budget decides which.
- **People:** whether sign-in providers are connectors, or a part of the
  people child that the application fills in.
- **Requirements across connectors:** a gate whose facts are one
  connector's while the landing it guards is another's
  (`domain/connectors.md`, section 14).
- **Scarce resources:** whether a write hold that is taken waits or is
  refused (`domain/connectors.md`, section 14).
- **The second application:** which one.
- **The client's pages:** how the kit's pages show an application's
  objects, for example as one kind of card per family.
- **Versioning:** how the kit is released and pinned, and how a change to
  its vocabulary reaches every application.
- **The worker:** whether one worker kit fits every application, or the
  worker's root is always the application's, as section 8 assumes.
