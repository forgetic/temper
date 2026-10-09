# One host for jig's runs

Draft, 2026-10-07, adopted the same day. A proposal from a design
discussion on 2026-10-07: jig keeps one state machine for hosting runs,
wherever their agents run, instead of two. jig's `hosts.md` and the
documents of section 6 now describe it, with the twin named the inline
agent, `jig-inline-agent`, and jig's for now (section 7's first
question). This draft keeps the reasoning; smith's side is smith's draft
`one-host.md`.

## 1. In one page

- **Today jig has two hosts** (jig's `hosts.md`, sections 5 and 6):
  - `jig-local-host`, inside the engine: a parent of smith's domains,
    one per run on the engine's slots;
  - `jig-worker-host`, on a worker: a hub between the link to the
    engine, smith's host domain (agent processes) and the application's
    workspace.

  Both implement the contract of jig's `hosts.md`, section 2, and the
  run's lifecycle behind it, twice.
- **The proposal: one hub,** `jig-host`, today's worker host, composed by
  every root that hosts runs: the engine's, when the application runs
  agents there, and a worker's. What differs between the two shapes
  moves into the hub's capabilities and its root, where the worker host
  already puts it.
- **The agent is a capability of two kinds** (section 4):
  - smith's host domain, agent processes, on a worker;
  - its in-memory twin, smith's domains in this process (smith's
    `host.md`, section 9), in the engine.

  Both hand the hub the same: calls, turns, waiting, notices and an
  answer in order, and the agent's failures, typed.
- **The link to the core is the root's:** over a channel the worker
  dials, through its protocol layer; or, in the engine, straight into the
  core's fleet, linked from construction and lost only with the engine.
- **Workspaces stay the application's capability,** absent in the engine
  as today: by policy, no longer by mechanism.
- **Why** (section 5): the run's lifecycle is built, tested and extended
  once; the engine, a worker and agents can share one process, as
  smith's `host.md`, section 9, already expects; and "local host" no
  longer names two different things in jig and smith.

## 2. Where the two hosts come from

- **History, not a design choice.** The worker host was temper's
  worker's host child, moved into jig (69e5a044). The local host was
  written for `ops` on smith's one-process form (36eb49cf). jig's
  documents never compared the two.
- **The worker host is already the general one.** It does not link
  smith: its only dependency is skein's. It knows a hosted run's
  lifecycle and nothing of the application's workspaces, processes or
  channels, which are capabilities its root translates to and from
  (jig's `hosts.md`, section 4).
- **The local host fuses two jobs.** It is the lifecycle, and it is the
  parent of smith's domains: it composes them, translates the core's
  charter into smith's, and routes their LLM requests.

## 3. What the two share

From their boundaries, each implements:

- slots and admission, refusing as busy or invalid;
- fencing by task and attempt;
- messages named, a bounded few held until the run is live, the rest
  bounced by name;
- calls relayed up, each answered once, withdrawn past the run's
  deadline;
- grants refreshed while the run lives;
- turns kept until acknowledged, within bounded room, the run waiting
  past it;
- cancel, a grace, then exactly one answer, kept until acknowledged,
  whose acknowledgement frees the slot;
- a worst case, checked by measurement.

## 4. What differs, and where it goes

| | In the engine today | On a worker today | With one hub |
|---|---|---|---|
| The agent | smith's domain, composed by the local host | a process, through smith's host domain | the agent capability: in memory, or processes |
| Stopping | smith's cancel, dropped past a grace | cancel, terminate, kill; the tree proved empty | the capability says the agent is gone; the hub waits for it either way |
| The link to the core | through the root, directly | a channel the worker dials: hello, redial, graces | the root's: directly, or through the protocol layer over a channel |
| Losing contact | never; lost only with the engine | a grace, a stop bound, everything sent again after the hello | inert in the engine: one hello at construction, never lost |
| Workspace and delivery | none | the application's | the application's capability, absent in the engine |
| The charter in smith's terms | translated by the local host | translated by the engine's protocol layer, carried as bytes | translated in one place for both: the function the protocol layer uses |
| LLM completions | smith's requests, routed by the root to the protocol layer | the agent process's own io, with grants | unchanged: neither passes through the hub |

**The in-memory twin** is the one new piece. It is smith's one-process
form (smith's `host.md`, section 9) packaged as a child that speaks the
same vocabulary as smith's host domain (smith's `host.md`, section 4):

- it starts a run as a composed smith domain, not a process;
- it hands its parent what smith's host domain hands it, in the same
  order, so a root translates both with the same small total functions;
- it stops a run by smith's cancel and, past the grace, drops it, which
  is when the run is gone;
- its LLM requests go up to its root, which routes them to the protocol
  layer, as the local host's do today.

What the twin gives up is said in smith's `host.md`, section 9: no
process as the run's containment boundary. That is why the engine still
runs no commands for its agents, by default.

**The fleet keeps its two kinds of host** (jig's `engine.md`, section
8): workers, which dial in and may be lost and come back; and the
engine's slots, present from construction and settled at once on a
restart. Both are reached with the same vocabulary; the engine's root
routes one to a channel and the other to its own hub.

## 5. What it buys

- **One lifecycle.** Admission, fencing, turns and answers kept until
  acknowledged, relays, cancel and the answer are written, reviewed and
  tested once. The open questions of jig's `hosts.md`, section 12
  (fairness on the engine's loop, saving periodically), are answered
  once for both shapes.
- **One world for it.** The hub's world runs it with smith's scripted
  agent processes and with smith's real domain over skein's fake LLM.
  The local host's world is no longer needed.
- **One root shape.** The engine's root and a worker's compose the same
  three kinds of child: the hub, an agent capability, and an optional
  workspace.
- **One process when wanted.** Development, tests and one person's
  machine can run the engine, a worker's hub with a workspace, and agents
  in one process (smith's `host.md`, section 9), with no code of their
  own.
- **One meaning per word.** smith's local host (`smith-local-domain`,
  smith's `host.md`, section 8) is a host for one person on a machine;
  jig would no longer have a "local host" of its own. smith's local host
  needs the same twin for its one-process form.

## 6. What changes

- **jig's documents:** `hosts.md`, sections 1 and 3 to 6 and 11; the
  trees in its `README.md` (section 9), `domain/README.md`, `root.md`,
  `engine.md` and `examples.md` (`ops` composes the hub and the twin);
  the row for the hosts in `testing.md`.
- **Crates:**
  - `jig-worker-host` becomes `jig-host`, its documentation no longer
    about workers only;
  - `jig-local-host` is retired: its lifecycle is the hub's, its charter
    translation joins the protocol layer's, and its parent of smith's
    domains becomes the twin;
  - the twin is new, smith's or jig's (section 7).
- **The plan:** the local host's increments (jig 13.2 to 13.4) are
  superseded once the twin and the hub's world cover them. The order:
  1. finish the hub's typed vocabulary, so one remains of today's
     legacy, `V2` and typed variants;
  2. build the twin, with smith's real domain in the hub's world;
  3. compose the hub and the twin in `ops`'s engine and retire
     `jig-local-host`;
  4. rename, and update jig's documents.

## 7. Open

- **Who owns the twin.** smith, as the one-process form of its host
  domain, which smith's local host needs too; or jig, beside the hub.
  Leaning to smith.
- **Bytes across the hub in the engine.** The hub carries what it does
  not read as opaque bytes: the charter, turns, call inputs and answers.
  The core keeps turns as bytes already; the charter and each call are
  encoded and decoded once more than they need to be. Whether that ever
  shows, and if so whether the hub carries tokens its root resolves.
- **Workspaces in the engine.** Whether a deployment may give in-engine
  runs a workspace, for one person's machine, knowing it has no
  containment; the mechanism would allow it.
- **The hand-off rule** (jig's `hosts.md`, section 4) gains one hop in
  the engine (twin, hub, core and back); each entry point must still
  complete its hand-offs before it returns.
