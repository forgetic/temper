# temper on jig

Draft, 2026-10-06. temper's core becomes jig, a kit for agentic
applications, and temper becomes jig's first application. jig's own
design is jig's `docs/design/README.md`, under `jig/` in this repository
while jig is built here. This document is temper's side: how jig is built
inside temper, what of temper goes to jig, what stays, and how jig moves
out. Until it is adopted, `docs/design/domain/` describes what is built.

## 1. In one page

- **jig is built inside temper first.** The boundary is drawn by
  refactoring temper's own code. A small second application checks it,
  and the work iterates until the boundary is clear. Then jig moves to
  its own repository, `~/src/rust/jig`, as it is.
- **`jig/` is jig's future repository,** laid out as it will be: its
  `README.md`, `AGENTS.md`, `docs/design/`, its crates, its worlds and
  test kits, and its example application. One rule holds the boundary:
  nothing under `jig/` depends on or names temper.
- **temper keeps what is temper's:**
  - the forge connector, and Forgejo's protocol;
  - git checkouts as its kind of workspace;
  - its charters;
  - its own kinds of object on the web;
  - its configuration;
  - the root of each of its processes.
- **The move is mechanical** if the rules of section 3 hold from the
  first commit: `git subtree split --prefix=jig`, jig's own workspace
  files, and temper's path dependencies turned into git dependencies.

## 2. Layout

```
temper/
├── jig/                  jig's future repository, as it is
│   ├── README.md
│   ├── AGENTS.md
│   ├── docs/design/
│   ├── crates/           jig-* crates, carved out of temper's engine, worker and web crates
│   ├── tests/            jig's worlds, the conformance world
│   ├── testing/          jig's fakes and kits: scripted workers and people, the fake person
│   └── examples/ops      the example application, production management: the reference root
├── crates/ …             temper, depending on jig's crates by path
```

jig's crates are members of temper's workspace while they are here, so
temper's gate (`docs/development/workflow.md`) covers them, and temper's
test budgets include them.

## 3. Rules while jig is here

1. **Final names.** jig's crates are named `jig-*` from their first
   commit, so the move renames nothing.
2. **One-way dependencies.** jig's crates depend on skein, smith and each
   other only, and the core and its children on skein alone. A check in
   the gate enforces this from jig's first crate.
3. **Citations in their final form.** temper cites jig's documents as
   jig's `<file>.md`, the way it cites skein's and smith's, never by a
   path inside this repository. jig's documents and code never cite
   temper's.
4. **jig's own test kit.** The fakes jig needs live under `jig/`. The fake
   forge and the fake checkout's git stay temper's.
5. **jig's own `AGENTS.md`** applies to everything under `jig/`.
6. **What is generic goes to skein first.** The journal (jig's
   `README.md`, 6.2) is a skein-lib container before the core uses it.

## 4. What goes to jig

| temper today | In jig |
|---|---|
| `temper-engine-domain`: routing, commits, loads, restart, runs, calls | the core; commits through skein-lib's journal; the connector routes stay in temper's root |
| `temper-engine-domain-tasks`, `-authority`, `-people`, `-fleet`, `-accounts`, `-brief`, `-notes`, `-views` | the core's children, cut free of the forge (section 5) |
| the store's records and encoding for these (`docs/design/store/`) | jig's store part |
| `temper-worker-domain`, `-host`, `-agent` | the worker host; the worker's root becomes temper's |
| the engine's hop of `temper-channel` | jig's channel, the engine's side and the worker's |
| `temper-web-domain`, `temper-web-view`, and the web's wire, protocol and shells to come | jig's client, minus temper's own kinds of object |
| `testing/temper-fake-person`, the engine world's scripted workers, people and store | jig's test kit and conformance world |

What stays:
- `temper-engine-domain-forge-*` and `temper-forge-forgejo`;
- `temper-worker-domain-checkout`;
- `temper-oauth`, if sign-in providers remain the application's (jig's
  `README.md`, section 14);
- the fake forge.

The legacy crates go at the cutover, as planned, and none goes to jig.

## 5. What is still temper's in the core today

| Today | Where it goes |
|---|---|
| authority: landing requirements (CI at a head, approvals, gates) | the forge connector's verdict (jig's `README.md`, section 7) |
| people: a person is a forge and a user there | a provider and a subject; providers are the application's |
| projects: repositories with roles and a home repository | resources with roles, from any connector; a home for projections |
| goals: an issue, and a priority that orders the landing queue | the priority stays in the core; the issue and the landing queue are the forge's |
| tasks: saved-work repository tags, and repository scope bits | resource tags and scopes named by connectors |
| fleet and worker: checkouts | kinds of workspace, temper's git checkouts among them |
| brief, notes, views: repository and pull request sections and scopes | supplied by connectors |

## 6. Where temper's documents go

| temper's `docs/design/domain/` | jig's `docs/design/` | What stays in temper |
|---|---|---|
| `core.md` | `core.md` | temper as an application of it: a software factory |
| `tasks.md` | `tasks.md` | |
| `authority.md` | `authority.md` | landing rules, as the forge's requirements |
| `connectors.md` | `connectors.md` | |
| `engine.md` | `engine.md` | temper's root: its connectors and their routes |
| `people.md` | `people.md` | signing in through the forge; people on the forge |
| `worker.md` | `hosts.md`, the worker host | checkouts; temper's worker root |
| `agent.md` | `hosts.md`, what the core takes from smith | temper's charters |
| `forge.md` | | all of it |
| `docs/design/store/` | `store.md` | the forge's records |
| `docs/design/web/` | `client.md` | temper's kinds of object, `ux/` for them |
| `docs/design/testing.md` | `testing.md` | the fake forge, temper's stories |

## 7. The move

When the boundary has stopped moving, and after the next-domain cutover
(`docs/plans/next-domain/07-cutover.md`):

1. **Split:** `git subtree split --prefix=jig` gives jig's history, and is
   pushed into `~/src/rust/jig`.
2. **jig's workspace files** are copied from temper's: `Cargo.toml`, with
   its lints and profiles; `clippy.toml`; `rustfmt.toml`; and the gate's
   workflow document.
3. **temper's dependencies** on jig turn from paths into git
   dependencies, pinned as skein and smith are.
4. **jig's `AGENTS.md`** loses its item about being built inside temper,
   and this document is removed.

The part that is not mechanical comes after the move: every change at
the boundary then needs a new pin. That is why the move waits until the
boundary has settled.

## 8. Open questions

- **How this fits the migration under way:** a revision of
  `docs/plans/next-domain/`, which this draft leaves for later.
- **Carving before or after the cutover:** whether the core is carved out
  of the new root as it is built, or after the cutover.
