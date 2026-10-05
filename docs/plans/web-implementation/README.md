# Building the web client's domain

Provisional, 2026-10-05. A plan for building the domain part of temper's
web client (`docs/design/web/architecture.md`): the client domain, the
view the person reads it through, and the worlds that test both. What
the web does is `docs/design/web/ux/`, which this plan takes as given;
the mockups (`docs/design/web/mockups/`) are its picture. This document
is the overview: scope, crates and layout, what the client needs of the
engine, conventions, and the increments in order. Each part has a
document of its own beside this one.

**Citations.** As the web's design does: `architecture.md` is
`docs/design/web/architecture.md`; `ux/inbox.md` and the like are under
`docs/design/web/`; the domain's documents by their directory
(`domain/people.md`); skein's foundation documents by file name
(`programming-model.md`, `testing-strategy.md`); temper's testing as
`docs/design/testing.md`.

## 1. Scope

"Domain" here means everything the client does above its protocol layer.
Two crates and their tests:

- **`temper-web-domain`**, the client domain (`architecture.md`, section
  3): objects by id, pages, pending keyed requests, live, behind and
  offline, windows, folds, drafts. Everything `ux/` has the web decide.
- **`temper-web-view`**, the view (`architecture.md`, section 4): state to
  tree, the person's events back to actions, the diff, the Markdown
  reader.

The view is in scope because the domain's worlds need it: in every world
the scripted person reads the view tree and presses what a person would
(`architecture.md`, 7.1 and 7.2). It is a pure function with no browser
in it, so it belongs with the domain, not with the layers that come
later.

Later, each from its own plan:

- `temper-web-wire`, the web protocol's documents and their JSON;
- `temper-web-protocol`, the client's protocol layer, and the address
  bar's codec (the domain sees typed addresses, never a URL);
- `temper-web-shell`, wasm-bindgen and the bundle; the native shell;
- the engine's side of the web protocol, serving the bundle;
- the protocol worlds, the real loop and the browser tier, with the
  person's DOM face over `skein-browser`.

Nothing here depends on them. The client domain defines its own
vocabulary (`programming-model.md`, 4.4: the types belong to the domain),
and the protocol layer will translate it to the wire.

## 2. In one page

- **Two step crates under the Rust subset.** `#![no_std]`, the workspace's
  lints, `skein-lib` their only dependency, so they build for wasm32 as
  they are (`architecture.md`, section 2). The view depends on the domain;
  nothing depends on the view yet.
- **The domain is the client's whole behaviour.** Its inputs are the
  person's actions, answers, stream events, time and its seed; its outputs
  are requests to the engine, reads, streams opened and closed, the
  address, session storage and notices. A world runs all of it with no
  browser (01-domain.md).
- **The view is a function plus a diff.** It builds a tree of a closed
  vocabulary from the domain's state, keeps the last one, and diffs them
  into patches. A node a person acts on carries a typed binding, never a
  closure, and the browser holds only the node's id (02-view.md).
- **Built against the web the design describes, not the routes that exist
  today.** The client's own world scripts an engine that speaks the whole
  web protocol of `ux/`, section 10, faults included. A second world joins
  the client to the real engine root, and grows its stories as the
  engine's routes land (03-testing.md, section 3).
- **One person, scripted once.** The person is a step machine that finds
  nodes by role, name and text, so a scenario written now runs unchanged
  in the browser tier later (03-testing.md, section 2).
- **Vertical increments after two spines.** The domain's spine (keys,
  streams, link state, session storage) and the view's spine (tree, diff,
  bindings) come first; then the first slice of `ux/README.md`, section 9,
  end to end; then inbox, conversation, task page, board, forge panels,
  each a page with its cards, its requests and its stories
  (04-increments.md).

## 3. Documents

| Document | What it holds |
|---|---|
| README.md | this overview |
| [01-domain.md](01-domain.md) | the client domain: its organisation, state, vocabulary, entry points, limits, step tests |
| [02-view.md](02-view.md) | the view: tree and vocabulary, builder, bindings, cards and pages, words, diff, Markdown |
| [03-testing.md](03-testing.md) | the person, the client's world and its scripted engine, the joint world with the real engine, referees, faults, budgets |
| [04-increments.md](04-increments.md) | the increments in order: what each lands, its tests, what it needs of the engine, the design documents it updates |

## 4. Crates and layout

```
crates/temper-web-domain/         the client domain                       step crate
crates/temper-web-view/           the view: tree, cards, pages, diff      step crate
testing/temper-fake-person/       the person: scripts, the tree face      step machine
tests/web/domain/                 the client's world: scripted engine     temper-web-domain-world
tests/web/engine/                 the client with the real engine root    temper-web-engine-world
```

```
temper-web-view ──► temper-web-domain ──► skein-lib
temper-fake-person ──► temper-web-view (the tree face only)
tests/web/domain ──► temper-web-domain, temper-web-view, temper-fake-person, temper-world
tests/web/engine ──► the same, and temper-engine-domain-world (its store, worker and commits)
```

- **The fake person is a shared crate** against the usual preference for
  fakes scripted inside their world, because one person serves every web
  world and, later, the browser tier: a scenario is written once
  (`architecture.md`, 7.2). Its DOM face will be a crate of its own, so
  that only the browser tier depends on `skein-browser`.
- **The scripted engine stays inside the client's world.** It takes
  liberties the real engine does not (faults on demand, shapes the engine
  has not built yet), as the worker's worlds keep their scripted agents.
- **`docs/design/testing.md`, section 7** gains `tests/web/*` and
  `testing/temper-fake-person` in increment W3.

## 5. What the client needs of the engine

The client is designed against the web of `ux/`, and the engine is still
being built. As of 2026-10-05 (`docs/plans/next-domain/STATUS.md`), the
root has these routes for people, which the joint world uses from the
start:

| The client | The engine root today |
|---|---|
| signing in: who the person is, a sign-in number | `Event::SignedIn` (03a) |
| starting a chat, keyed, answered with its number | `Ask::StartChat` (03a, 06a) |
| reading a held chat's escalation | `Event::ReadEscalation` (02d1) |
| deciding it: release, reject with a reason, pass, at its revision | `Ask::DecideEscalation` (02d1) |
| reading a chat's result after it ended | `Event::ReadResult` (06a) |

And these it does not have yet, beyond what `ux/README.md`, section 10
already lists (each listed again in the increment that needs it):

- **Who is signed in:** the person, their projects and roles, the inbox's
  counts. The frame needs it on every page.
- **Reading a task:** its header, phase, overview, budget; the first
  slice's chat page needs its phase.
- **Watches as the root's routes:** a person's inbox, a run, a task's
  tree, a project's goals and its changes (`domain/engine.md`, section 11),
  each starting from a snapshot.
- **Paged reads:** transcripts, history, ended delegates, the person's
  chats, the inbox past the engine's limit.
- **The rest of `domain/people.md`, 5.1:** words, answers, decisions on
  proposals, stop, release, amend, cancel, take over, set a goal,
  prioritise, the read position.

The client's world fakes all of it, so no increment here waits on the
engine; only the joint world's stories do.

## 6. Conventions

Every increment follows `docs/plans/next-domain/README.md`, sections 3.1
(built beside: everything here is new) and 5.6 (increments and the
gate), and its review checklist (section 8): module docs name the kept
state, what the module never knows, its entry points and contracts;
public items are documented with their sender, terminal and bounds; code
cites `web/architecture.md`, `web/ux/<file>.md` and `domain/<file>.md`
by section. Beyond those:

- **No dormant vocabulary.** A variant of an event, request, ask or page
  is added by the increment that builds its behaviour and tests it in the
  client's world. The sketches in these documents show where the
  vocabulary is going, each variant marked with the increment that adds
  it.
- **The client never parses.** Words, specs and results arrive as owned
  bytes and are rendered by the view; addresses, form numbers and session
  storage arrive typed, decoded by the layer below (the view decodes a
  form's digits; the protocol layer, later, URLs and stored JSON).
- **Wording is the view's.** The domain carries facts (a phase, a hold's
  reason, an amount, a wall time); the view says them in words, from the
  tables of `ux/tasks.md`, section 3, `ux/chats.md`, 4.3 and
  `ux/forge.md`, 2.1.
- **Design documents move with the code.** An increment that settles a
  question of `architecture.md` or `ux/` records it there in the same
  branch, as the domain migration does.

### 6.1 Budgets

The focused suite stood at 10.993 of its 15 seconds at the last gate
(03c), and the fuzzy at 26.798 of 60, so the web's tests start small:

| New tests | Focused seconds | Fuzzy seconds |
|---|---:|---:|
| `temper-web-domain` step tests | 0.2 | — |
| `temper-web-view` step tests | 0.2 | — |
| `temper-fake-person` step tests | 0.05 | — |
| client's world (`tests/web/domain`) | 0.5 | 4 |
| joint world (`tests/web/engine`) | 0.3 | 3 |

Measured as `docs/plans/next-domain/README.md`, 5.5 measures: the sum
of a package's tests, alone, with the serial profile. A world's stories
use one to three seeds; its sweeps are fuzzy. A gate that finds a budget
broken makes the web's tests cheaper first; the budgets in
`.config/nextest.toml` are not raised.

## 7. The increments

```
W1 domain spine ──┐
                  ├──► W3 person and world ──► W4 first slice ──┬──► W5 joint world
W2 view spine ────┘                                             ├──► W6 inbox ─────────┐
                                                                ├──► W7 conversation ──┼──► W10 forge panels
                                                                └──► W8 task page ─────┘
                                                                      └──► W9 board
```

| Increment | What it lands |
|---|---|
| W1 domain spine | the domain crate: start, addresses, the frame, reads and streams, link state, keyed requests with `StartChat`, session storage, notices |
| W2 view spine | the view crate: the tree and its vocabulary, the builder, bindings, the diff; the frame, signing in and starting a chat |
| W3 person and world | the fake person's tree face; the client's world with its scripted engine, referee and faults; starting a chat across restarts and reloads |
| W4 first slice | the chat page, minimal; a held chat's escalation card; the Markdown reader; two tabs deciding one thing |
| W5 joint world | the client against the real engine root, for the first slice |
| W6 inbox | the inbox: needs you and updates, filters, every card, decided elsewhere, the read position |
| W7 conversation | the transcript from its end, runs, streaming, words and their states, stop, close, cancel |
| W8 task page | the universal page: header, overview, activity, the plan as a tree, history, budget, steering |
| W9 board | a project's goals by priority, setting a goal, prioritising |
| W10 forge panels | a change's panel and card, changes and landing queues, approvals at a head |

W1 and W2 run in parallel (W2 needs only the domain's `Action` and the
frame from W1, agreed in W1's first commit). After W4, W5 to W8 run in
parallel, three at a time at most (memory: three building agents). What
follows W10 (a project's tasks, findings, notes, settings, spend, the
system's page) is planned once the board is built, from `ux/projects.md`.

## 8. Risks, taken first

- **The test budget.** Two worlds and three crates of step tests join a
  focused suite at three quarters of its cap. The client's world is cheap
  by construction (no store, no channel, a scripted engine), and its
  stories run on one seed each; W3 measures it before more stories join.
- **The view in debug builds.** Building a whole page's tree and diffing it
  on every step that changed what is shown is the design's price for a
  view with no state. W2 measures a page at its limits in a debug build;
  if it is too slow for the worlds, the view builds only the parts whose
  inputs changed (a per-section generation), which changes no interface.
- **Two ends written together.** The client's world and the client are
  written by the same hands and can share a misreading of the engine. The
  joint world (W5) is the check, and later the wire's transcripts
  (`architecture.md`, section 5).
- **The engine's routes.** The client is ahead of them by design. Where the
  joint world cannot yet tell a story, the gap is listed in that
  increment, and in `ux/README.md`, section 10 if it is not there yet.

## 9. Open questions

- **Filters in the address,** so a filtered inbox can be bookmarked; for
  now filters are page state and a reload keeps them through session
  storage.
- **Several pages held at once,** for an instant back button; for now one
  page is held, and going back reopens it from its snapshot.
- **A child domain for connectors' panels:** the forge's panel is a module
  of the domain, shaped to become a child crate (as the engine's
  connectors are) when a second connector comes or the module outgrows
  the crate (01-domain.md, section 9).
