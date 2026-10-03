# Temper's testing pyramid

Provisional, 2026-10-02. How temper is tested, from one step function to
the whole system under one io_uring loop: the tiers, what is real and
what is fake in each, and the fakes that stand in for temper's
neighbours as they grow. Section 6 says what the tiers check, section 8
where things stand, section 9 what is not built yet, and section 10 what
is still open.

## 1. In one page

- **Each tier makes one more layer real.** The bottom tier tests one step
  function. Each tier above runs more of temper for real and fakes the
  rest, up to every process on the real kernel. A tier is added when the
  layer it makes real is built.
- **Fakes are models too.** Every fake follows skein's
  `programming-model.md`. It shares no domain types with what it stands in
  for, so a world translates between them as a protocol layer would. It
  checks its client as it goes, refusing what the real neighbour would
  refuse.
- **Two kinds of neighbour.** Peers sit across a wire: LLM providers, the
  forge, people on the web. Their fakes grow into services, with protocol
  and io layers of their own, and meet temper at the wire. The machine
  sits under io: files, processes, and the programs they run. Its fake
  stays one model, with a face for each layer of temper it can sit under
  (section 3).
- **Fakes grow with temper.** A fake's protocol and io layers are built
  alongside the temper layers they face, so each tier finds its fakes
  ready.
- **One fake, every tier.** A fake's model is the same at every tier; only
  the face it shows changes. A scenario written for the fakes (an LLM's
  script, repositories on the forge, what people do) runs unchanged at
  every tier where those fakes appear (section 5).
- **The test is a step too.** A scenario's expectations run in the loop
  as a step machine, the referee, which watches what the fakes see, arms
  the test's deadlines and ends the test (section 5.2).
- **Deterministic up to the simulator.** Every tier but the top one owns
  the clock, the seeds and everything around temper, so a seed replays to
  the same run. The top tier trades replay for the real kernel, the real
  network stack and the real programs.
- **Focused tests always, fuzzy tests at the gate.** Focused tests check
  what a model is expected to do, and run on every change in a handful of
  seconds; fuzzy tests search many random worlds for what nobody thought
  to check, spend more CPU, and gate merges to main (section 6.1).
- **Some fakes are temporary.** A fake of one of temper's own components
  stands in until its real model exists, then retires. The worker's and
  the engine's both have.

## 2. The tiers

```
            ┌───────────────┐
            │   real loop   │  everything real: kernel, sockets, git, rg, sh
          ┌─┴───────────────┴─┐
          │     simulator     │  io real: a simulated ring, network and machine
        ┌─┴───────────────────┴─┐
        │    protocol worlds    │  protocol real: bytes between layers, no io
      ┌─┴───────────────────────┴─┐
      │       system worlds       │  several of temper's models together
    ┌─┴───────────────────────────┴─┐
    │         model worlds          │  one model, or one sub-model
  ┌─┴───────────────────────────────┴─┐
  │            step tests             │  one step function
  └───────────────────────────────────┘
```

| Tier | Real | Faked | Replays |
|---|---|---|---|
| step tests | one step function | its events, by hand | yes |
| model worlds | one model with its sub-models, or one sub-model | its parent and neighbours: scripted, or a fake's model | yes |
| system worlds | the models of several components | the peers and the machine, at their model faces | yes |
| protocol worlds | each component's model and protocol layer | io and the network; the peers with their protocol layers; the machine at its io face | yes |
| simulator | every process's service: model, protocol and io | the kernel: the ring, sockets, the clock; the machine at its kernel face | yes |
| real loop | everything temper ships, under its shells | the peers, as services on loopback sockets; the machine is real, in a sandbox | no |

### 2.1 Step tests

A crate's own tests feed its step functions events and inspect the
requests they emit: one transition, a full queue, a stale handle, a
refusal at the entrance. lib's containers are tested hard here. Fuzzing
with `cargo fuzz` joins this tier with the protocol machines: each
machine alone, fed `Bytes` under every demand, and the step functions fed
recorded event sequences.

### 2.2 Model worlds

A model world runs one model in one loop, from a seed: a sub-model with
the world as its parent (programming-model.md, 4.5), or a component's
top-level model with its sub-models beneath it. The world plays
everything else. It scripts the neighbours that are temper's own, taking
liberties the real ones do not, and uses fakes for the peers and the
machine. Behaviour is tested here, and most of temper's tests live here.

### 2.3 System worlds

A system world is a model world where several of temper's components
meet, each as its real top-level model. Two are built, both with the
real engine driving the real worker: the agent's top-level world, whose
worker hosts real agents, and the whole worker's world, whose worker
hosts scripted agents over a channel that fails (4.5). A system world
aims at what crosses components, which no component's own world can
see: the tree an agent left is the tree the worker pushes, and the
outcome the run accepted is the one the engine records. It is still the
model layer only: where a protocol layer will sit, the world translates.

### 2.4 Protocol worlds

A protocol world adds each component's protocol layer, and each peer
fake's own, and passes bytes between them in place of io and the network,
cut and joined at random. It tests what translation adds: framing,
codecs, the JSON an LLM writes as a tool's input, the git invocations and
the parsing of their output. The machine sits there at its io face
(section 3.2). It runs the system worlds' scenarios, through bytes.

### 2.5 The simulator

The simulator drives `service::iterate` of every process in the scenario,
the function each shell runs, and plays the kernel under them: the ring
(it reads submissions, fills op buffers through io's API, and produces
completions), the network between the processes' sockets, the clock, the
seeds, and the machine at its kernel face. It runs with tiny limits
(slabs of capacity 2), and injects cancellation and timeout in every
state, refusal at every admission point, short reads and writes, and
completions after a cancel. Processes come and go: a spawn the worker
asks for starts an agent's service, and a kill ends it. Where the model
worlds test behaviour, the simulator tests mechanics.

### 2.6 The real loop

The top tier runs temper as it ships, with one io_uring loop driving
everything: the fake peers as services listening on loopback sockets, the
engine, the worker and the agents it spawns. The machine is real: a
sandbox directory, inside the containment production uses (agent-model.md,
section 6; worker-model.md, section 8), with real git, rg and sh. It
shows what only the real kernel and real programs can: the ring adapter,
the containment of process trees, git's actual output. The referee
(section 5.2) judges it and ends it, as at every tier, its deadlines on
the wall clock. It does not replay. A failure found there is rerun as a
scenario lower down, where it does.

## 3. Neighbours and their faces

### 3.1 Peers

A peer's fake is a service in the making. It grows the layers that the
temper component facing it grows, and each tier joins the two at the
lowest layer both have:

```
agent    model ─ protocol ─ io   ⇄   io ─ protocol ─ model   fake LLM provider
engine   model ─ protocol ─ io   ⇄   io ─ protocol ─ model   fake forge
engine   model ─ protocol ─ io   ⇄   io ─ protocol ─ model   fake people, on the web
```

In a model world, the world translates between the two models'
vocabularies. In a protocol world, it passes bytes between the two
protocol layers. In the simulator, the simulated network joins the two io
layers, and in the real loop, sockets do. A peer can be a client as well
as a server: the fake forge serves its API and git, and sends webhooks to
the engine.

### 3.2 The machine

The machine is not a peer. Files and processes are io's own operations,
so there is no wire to meet it at, and its fake grows no protocol or io
layers. It stays one model, and shows a face to whichever layer of temper
sits just above it:

| Temper is real down to | The machine's face | It answers |
|---|---|---|
| the model | model face | typed operations: the tools' file operations, commands and searches; the checkout's git operations |
| the protocol layer | io face | io's records: open, read, store, rename, spawn with an argument vector, bytes on a pipe, an exit |
| io | kernel face | ring submissions, with completions |
| everything | none | the real kernel, in a sandbox |

Every component's files and processes go through it: the worker's
workspaces and agent processes, the agent's tools and checks, and the
engine's store.

## 4. The fakes

What every fake shares:

- **A step crate** under `testing/` when every tier needs it, so the
  simulator and the real loop can host it as they host temper's models.
  What only one world needs stays in that world, as a script specialised
  to what the world aims at.
- **No shared types.** A fake's vocabulary is its own; a world, or a
  protocol layer, translates.
- **It checks its client.** It refuses what the real neighbour refuses,
  and asserts what the real neighbour's contract guarantees.
- **Faults are configured:** latency, failures, drops and refusals, drawn
  from the seed.

### 4.1 LLM providers

`testing/temper-llm-model` is a provider seen from the inside. It answers
each call after a drawn latency, by a world's script or at random
(failures by configured chance, a number of tool rounds, then a final
answer), and rejects conversations a real provider would reject.

It grows the server side of each provider API the agent speaks: HTTP,
server-sent events, the provider's JSON, its errors and rate limits. Its
faults move down with it: a stream cut mid-event, a malformed chunk, a
slow trickle.

### 4.2 The forge

The fake forge holds what temper meets on a forge: repositories, issues,
pull requests, comments, labels, reviews, CI, merges, wikis and webhooks;
and the git a worker clones, fetches and pushes. Both live in one store,
so a branch the worker pushes is the head the engine reads through the
API, and a merge the engine asks for moves the branch the next fetch
sees.

- **A model first.** The engine's worlds need one (engine-model.md,
  section 14): CI that passes, fails or never reports; merges that
  conflict; webhooks that come late or not at all; requests that fail or
  hit a rate limit; people who comment and edit. Its git keeps git's
  rules: a push moves a branch only by a fast-forward, and another party
  can move a branch under a run.
- **Then a service:** Forgejo's API and GitHub's, the subset temper uses,
  over one model; signed webhooks sent to the engine; and git over HTTP,
  the subset real git needs to clone, fetch and push.
- **CI follows content.** A repository's CI runs its checks in the shell
  subset (4.3) on the pushed tree, so a change that is wrong fails on the
  forge as it fails in the run.

The model is built, as `testing/temper-forge-model`, a step crate with a
vocabulary of its own. What building it settled:

- **Forgejo-shaped, not better than Forgejo.** It refuses what Forgejo
  refuses (too little permission, a label not defined, an empty title or
  body, a stale head on merge, a merge its base's protection does not
  allow), and keeps what Forgejo keeps and no more. Listings are
  Forgejo's: the items updated at or after a time, at a second's
  resolution, least recently updated first, paged by number, each page
  read from the order as it is when its call arrives, so an item that
  changes during a listing moves to its end and can shift another onto
  a page already read. An item's updated time moves for what moves
  Forgejo's (a comment, a review, its labels, its title or body, a push
  to its head, closing, reopening, merging), not for a status on its
  head nor for a comment edited or deleted; a configuration can make
  either move it. A pending review is hidden until it is submitted, and
  keeps the id it was started with. Stale approvals are dismissed only
  where the protection says so, off by default as on Forgejo. Anyone
  with write permission may edit or delete another's comment, as on
  Forgejo's web, so a person can mangle the engine's record.
- **Timing.** A call is decided as it arrives: refused for its user's
  rate, failed before it is made, made and then timed out, or made, its
  answer held for a drawn latency, sometimes late. A call may also time
  out first and land afterwards, as a request a client gave up on can
  still be acted on. The forge keeps a clock of its own, at its
  resolution and skewed from the world's, and every time it shows is on
  it. Webhooks name the branch and commit they are about, and come after
  a drawn latency, late, out of order, or never.
- **CI is drawn or cued by content.** When a commit becomes a head, each
  of its repository's check contexts goes pending, then passes, fails or
  never reports, by chance; or, where the repository configures a cue,
  passes on a commit whose file at a path holds a marker, as the stand-in
  until CI runs checks in the shell subset. A context may run again once
  after it reported, so a head's CI can move after it settled. People
  with write permission report statuses of their own.
- **Merges** happen at an exact head, as a squash: one commit on the
  base's tip, three-way from the newest commit head and base share, which
  conflicts where both changed a path differently. A repository has at
  most one protected branch, which takes a merge only with the statuses
  and approvals its protection asks for, and refuses pushes, creation and
  deletion. Deleting a branch closes the pull requests it was the head or
  base of.
- **Its git is the only forge git.** Commits live in the forge's one
  store, named by a count so a seed replays to the same names, each a
  parent and a tree; they carry no message. A working tree's git
  (`temper-checkout-fake`) reaches the forge through a typed transport,
  as real git reaches a remote across the network (4.3): the worker's and
  the agent's worlds route it to the fake forge directly, answered in the
  same instant, since io's latency already stands for the network and the
  forge.
- **Seen from outside.** Every change it makes is observed, content and
  all, and so is every write it refused, in a bounded queue a world
  drains for its referee (5.2); a world can also read its store at
  settle, count what it did and refused, and ask how much room is left,
  so that a forge that filled up is told from one that works. Needing
  room, it forgets the statuses of the oldest commit no head shows;
  commits it never forgets.

### 4.3 The machine

The machine is the host temper's processes run on. It has three parts:

- **Files:** directories, files, links and special files; a version that
  changes with every change; roots that operations resolve beneath, as io
  does; and each process tree's view of them, writable or read-only.
- **Processes:** trees, with pipes, an environment, an exit and a kill;
  the time they take, on the world's clock; deadlines raced as io races
  them.
- **Programs,** what a process runs. Each is a small service whose
  protocol is its command line: a model of what it does to the files, and
  a face that takes an argument vector and prints the bytes the real
  program prints. The programs are git, whose working trees are the
  machine's and whose remotes are on the fake forge; rg; sh; and temper's
  own. The agent is a program the worker spawns, with its channel on the
  program's pipes.

At the model face, a world calls what a program does directly: a typed
fetch is the git program's fetch, with no argument vector. From the io
face down, every program runs through its command line. That is what
tests the protocol layer's invocations and its parsing, and only the
invocations temper makes need a face.

The forge's git is not the machine's. The git program reaches the fake
forge as real git does, across the network: through a typed transport
that the world or the simulator routes, and over HTTP in the real loop.
That split is built: `temper-checkout-fake` keeps working trees and
their git directories, the commits a tree cloned, fetched or made among
its files, while the forge keeps the repositories, and the transport
carries git's calls between them (where a repository's branches are, a
fetch, a push, a branch created for a base).

**The shell subset.** An LLM can type any command, and no fake can run a
real build. The machine's sh interprets a small subset of POSIX sh: a few
commands (`test`, `grep -q`, `cat`, `echo`, `exit`) joined by `&&`, `||`
and `;`. The fake LLM's scripts write their commands in it, and fake
repositories write their checks (`.temper/pre-pr`) and their CI in it.
This gives two things:

- **Checks follow content.** An edit is right or wrong, and the check,
  the run and the forge's CI all say so. A scenario can tell a run that
  fixed the bug from one that only said it did.
- **Scenarios run on a real machine unchanged,** since real sh speaks the
  subset.

A command outside the subset fails as an unknown command does. Programs
a world scripts by name, such as a test suite that takes two hours, stay
as named programs of the machine.

### 4.4 People

People chat, accept and reject plans, release held work, and write and
correct notes (engine-model.md, section 14), on the forge and on the
web. Their fake is a scripted actor, drawn from the seed. On the fake
forge it acts as a forge user. On the web it is a client of the engine's
web protocol, a peer like the others, and it grows that protocol's client
side when the web is built.

Today people are scripted in the engine's world, ordinary Rust rather
than a model: each story's person looks at the forge as observed, never
at its store, and does what the story calls for next, one thing at a
time, so a call lost to a restarting engine is simply tried again. On the
web they make the engine's own calls, which no protocol carries yet. The
whole worker's world reuses them; the agent's top-level world scripts its
own, as its agents cannot take a session's turns yet.

### 4.5 Temper's own components

A fake of one of temper's components stands in for it in its neighbours'
worlds until its real model exists, then retires, and the worlds meet the
real component. A fake worker served until the worker's model was built,
and a fake engine until the engine's was. Both have retired: the system
worlds run the real engine, each wiring it as the protocol layers would.

- **The agent's top-level world** (`tests/integration/agent/model`): the
  real engine, worker and agents; the fake LLM provider; one fake forge,
  which the engine reaches through the forge's protocol as the engine's
  world plays it, and io's git through the working trees' transport; the
  engine's store; people who hand in issues whose step is an agent step
  or a change, each with a record as an earlier life of the engine would
  have written it (the agent's side cannot take a session's turns yet), a
  reviewer, and a person who stops a run now and then. Its channel
  between engine and worker keeps its order and never drops.
- **The whole worker's world** (`tests/integration/worker/model`): the
  real engine and worker; the fake forge, reached by the engine and by
  the worker's git; the engine's world's people and store; and the
  process trees and scripted agents of the worker's agent world, their
  words drawn from their charters. Its channel drops for less and more
  than the worker's grace, stalls, and carries copies of frames, some
  late; the worker shuts down and a new one starts cold; the engine
  restarts, at drawn moments and while it applies an outcome.

Both use what the engine's world's crate offers as a library: the
codecs for what the engine writes inside comments and wiki pages and for
the charters and outcomes a worker carries as bytes, the forge's
protocol as the world plays it, the store, the engine's referee, and the
names runs and attempts take on a worker's channel (a run by its item,
an attempt by its item and count); the whole worker's world uses its
people too.

Scripted stand-ins inside a world are another matter, and stay. The
worker's worlds keep agents that hang, crash and break the channel's
rules, and the run's world keeps a host that cancels twice, because the
real ones never do.

## 5. Scenarios

### 5.1 Setting one up

A scenario is what a test sets up in the fakes' own terms, with a seed:
each conversation's script for the LLM, the repositories on the forge
with their files and checks, the items and the people, and the faults to
inject. A scenario cues its scripts through content, as the agent's
top-level world does today: a repository's files say which script its run
follows.

Since a fake's model is the same at every tier, a scenario runs at every
tier its fakes reach. A failure found high up, in the real loop say, is
rerun lower down, where it replays.

### 5.2 The referee

A scenario's expectations are a step machine of their own, the referee.
It runs in the loop beside everything else, at every tier.

```
observations ──► referee ──► a verdict: passed, failed and why, or stop
(fakes, facts)      │   └──► stimuli: the network drops, a component restarts
                    └── deadlines of its own: the test's time limits
```

- **The same shape as every step:** its own state, `env.now`, events in,
  requests out, and a deadline table of its own, whose earliest deadline
  the shell's one ring timeout covers as it covers every layer's
  (programming-model.md, section 9). In the tiers that replay, its
  deadlines are simulated time, so "within two hours" takes milliseconds;
  in the real loop the same deadline is wall time.
- **It steps on what it observes,** and on its own timers, not on every
  iteration, so its work per iteration stays bounded.
- **It watches from outside.** It sees what the fakes saw (a push, a pull
  request, a comment, a call to the LLM) and the facts temper emits, never
  a service's state. What it sees is then the same at every tier, whether
  an agent runs in the loop or as a process of its own, so a scenario's
  expectations are written once and run wherever the scenario runs.
- **Two kinds of expectation.** Safety, what must always or never happen
  (nothing lands on a default branch without green CI; no forge
  credential reaches an agent), is checked on every observation.
  Liveness, what must happen by a deadline (a pull request opened, with
  green CI, within the hour), is a deadline armed; one that fires fails
  the test, listing what is still pending.
- **It ends the test:** passed, once every expectation is met and nothing
  is in flight; or failed, reported with the seed where the tier replays
  and with the trace in the real loop.
- **It injects what belongs to no fake,** at moments of the scenario: the
  network dropping, a component restarting, a worker told to shut down.

The referee is not where a tier's boundary contracts go. One terminal per
request, each operation's deadline and identity, and the invariants once
things settle are seen differently at each tier, so they stay in each
tier's harness (section 6). What a scenario expects of the system as a
whole goes in the referee, and travels with the scenario.

The referee is built in the shared harness, `tests/integration/world`,
as ordinary Rust. Each world writes its scenario's expectations: what
its referee observes, the names of what it expects to happen, the
stimuli it may inject, and what it checks of each observation. Through
them the referee checks safety and fails at once, arms a liveness
expectation as a deadline, meets it, or withdraws and re-arms it; a
liveness expectation whose deadline fires fails the test and is no
longer pending, so meeting it later counts for nothing. A stimulus comes
out when it is due, or at once from the observation that calls for it,
so a world can inject it between two things it does, such as a restart
between two writes. The verdict is passed, still open (and what is
pending), stopped early by the scenario, or failed (when, why, and what
was pending then).

Every engine world has a referee, the engine's own included, and so do
both system worlds, each with two: the engine's world's, over what the
forge did and what the engine assigned, and one of its own where the
components meet (the worker and its agents; or the channel, over the
engine's acknowledgements, the calls relayed and answered once, and each
answer taken once). Their stimuli are an engine restarting cold (at a
drawn moment; at once as it posts an outcome, so that it restarts while
applying it; or, in the engine's world, at a moment a scenario chooses
as the forge shows it), a worker's channel dropping or a worker vanishing,
and a worker told to shut down. The engine's referee meets a story only
when its item closes. In the agent's top-level world a story may end
held, so its own referee ends an item's expectation on a hold the
scenario allows (for its runs' failures, a person's stop or its plan's
reasons, and for its writes or record only where the forge or the store
were scripted to fail) as well as on a close; the engine's referee could
take the allowed holds as a parameter. The worker's sub-model worlds,
and the agent's tools', session's and run's, keep their checks inline.

## 6. What the tiers check

Every world, and the simulator, checks:

- **Contracts as it goes:** one terminal event per request, one reply per
  call, each operation's deadline and identity, and each component's own
  (worker-model.md, section 9).
- **Invariants once it settles:** no live entities (every slab empty),
  nothing in flight, every process gone and read to its end, every answer
  taken once; and ownership is a tree, with no orphans.
- **Memory:** a counting allocator measures the most a model held in one
  step, against its worst case (programming-model.md, 6.3); the simulator
  checks it at every iteration.
- **Replay:** a seed replays to the same trace.
- **Transition coverage:** each cell is a handler function
  (programming-model.md, 5.4), so function coverage of the handlers over
  a run (`cargo llvm-cov`) lists the transitions exercised and those never
  reached.

These are the harness's. The referee checks the scenario's expectations
on top, at every tier, the real loop included (section 5.2), and the
fakes check their clients as they go (section 4). The protocol tiers
add fuzzing of each machine. The real loop adds that production's
containment holds: a stopped run's process tree is empty, and a command
cannot write a git directory.

### 6.1 Focused tests and fuzzy tests

The tests come in two suites, run by different commands
(`docs/development/workflow.md`):

- **Focused tests** check the behaviour a model is expected to have: the
  step tests, and under `tests/integration` each world's scenarios,
  referee tests, replay, facts changing nothing, and memory at the worst
  case. A scenario that needs randomness runs the few seeds that show its
  behaviour; a cheap random world may stand as a smoke test. They run by
  default, on every change, and must be fast: the whole suite takes a
  handful of seconds, 15 at most, enforced.
- **Fuzzy tests**, under `tests/fuzzy`, look for what no scenario names:
  sweeps of many random worlds, each settled under every invariant and
  every ending reached among them; models driven at random against their
  worst case; a function against a naive one. They are CPU intensive, and
  may take longer where that is worth it for the bugs they catch: a minute
  at most for the suite, enforced. They gate merges to main, and do not
  run on every change.

A fuzzy test lives in a package of its own under `tests/fuzzy`, beside
its world's under `tests/integration`, and uses that world: a world's
settings, random ones included, are the world's, not either suite's. A
seed a fuzzy test finds failing is fixed and kept, as a scenario if it
shows behaviour worth naming, or among the sweep's seeds; one that cannot
be fixed yet is a finding, replayed by an ignored test until it is.

## 7. Layout

```
crates/*/src/tests.rs                       step tests
testing/                                    fakes, as step crates
tests/integration/world                     what every world shares: schedule, stage, ledger, trace, heap, referee
tests/integration/<component>/<sub-model>   sub-model worlds, the engine's in tests/integration/engine
tests/integration/<component>/model         component and system worlds
tests/integration/checkout                  the machine and working trees' git, today
tests/integration/forge                     the fake forge's tests in ordinary Rust: its memory
tests/fuzzy/<component>/<sub-model>         each world's fuzzy tests: random worlds, models driven at random
tests/fuzzy/lib                             lib's byte search against a naive one
```

Under `tests/integration/<component>/model`, the engine's is its own
world, and the agent's and the whole worker's are system worlds (2.3);
the engine world's crate is a library the system worlds use (4.5).

The protocol worlds, the simulator and the real loop find their homes
when the first of each is built.

## 8. Where things stand

As of 2026-10-03.

| Tier | Built |
|---|---|
| step tests | every model crate, the fakes, and lib |
| model worlds | the agent's tools, session and run; the worker's checkout, agent and host; the engine's work, plan, forge, fleet, brief, notes and views (the rules, which keep no state, by step tests alone); the engine's own |
| system worlds | the agent's top-level world: the engine, the worker and agents; the whole worker's: the engine, the worker and scripted agents |
| protocol worlds | none: no protocol layer exists |
| simulator | none: no io layer, service or shell exists |
| real loop | none |

The fakes:

- **LLM provider:** its model, `testing/temper-llm-model`.
- **Engine:** retired; the system worlds run the real one (4.5).
- **Forge:** its model, `testing/temper-forge-model` (4.2), with its git.
  The forge sub-model's world, the engine's and the system worlds run
  against it, the forge sub-model world's translation standing in for
  the protocol layer; io's git reaches it through the working trees'
  transport, with no latency or faults of its own there, as io's stand
  for them. It has no inline review comments, and no description or link
  on a status, which the worlds answer empty.
- **Machine:** `temper-checkout-fake`, ordinary Rust rather than a step
  crate, at its model face only, through each world's translation. It has
  files with versions, links and roots, a search, and the working trees'
  git. A command is matched whole and answered with canned output and
  file changes, whatever the files hold. Agents are programs already, at
  the model face: the agent's top-level world starts an agent model for
  each spawn, with its channel on the process's pipes.
- **People:** scripted in the engine's world (4.4), and reused by the
  whole worker's; the agent's top-level world scripts its own, and the
  engine sub-models' worlds script what people do through the parent
  they script.

The checks:

- **Memory** is measured in every world, and the fake forge's against its
  worst case in `tests/integration/forge`.
- **Replay** is checked in every world.
- **Transition coverage** is not measured, and nothing is fuzzed yet, as
  there is no protocol machine.
- **Scenario expectations** are a referee's (5.2) in every engine world
  and both system worlds. The worker's sub-model worlds, and the agent's
  tools', session's and run's, check theirs inline, beside their
  contracts.
- **Focused scenarios** in the engine's world tell each of its stories
  (engine-model.md, section 14) on one seed, among them a tracking label
  taken off, a garbled record, a run watched and stopped, a supervisor
  woken once by a burst, and an approval of an earlier head.
  Some restart the engine at a moment they choose, as the forge shows it
  (an outcome posted, a plan's first item made, a goal's record grown, a
  claim written, a label taken off, a record garbled); those between a
  write and its answer assert that nothing is made twice.
- **The whole worker's world** has another party delete branches between
  attempts, and its sweep reaches every way a run's preparation fails.
- **Findings:** every seed found failing is fixed and replays, passing;
  no test is ignored.

## 9. Not built yet

By tier:

- **System worlds:** several real workers in one world (the engine's
  world places runs on scripted ones; worker-model.md, section 11); in
  the agent's top-level world, what the agent's side lacks
  (agent-model.md, section 10): sessions, inbound events, waiting,
  parking and relayed calls; in the whole worker's world, plans' stories,
  which run in the engine's world only.
- **Protocol worlds,** each with the protocol layer it tests.
- **The simulator,** with the io layers, the services and the shells.
- **The real loop:** a shell that drives every service in one loop, and
  the sandbox.
- **The referee** in the worlds that still check their scenarios inline,
  so that those checks run at every tier; and nothing made twice across
  a restart at a drawn moment, which the engine's referee only counts
  for now (engine-model.md, section 16).
- **Checks:** transition coverage, fuzzing.

By fake:

- **LLM provider:** its protocol and io layers.
- **Forge:** inline review comments, statuses' descriptions and links,
  commit messages, more than one protected branch in a repository, and CI
  that runs checks in the shell subset; then its protocol (Forgejo,
  GitHub, webhooks, git over HTTP) and io layers.
- **Machine:** a step crate; programs as step machines with command-line
  faces; the shell subset; the io face and the kernel face; the real
  loop's sandbox.
- **People:** a model, then the client side of the web's protocol.

## 10. Open questions

- **The forge's git in the real loop:** git over HTTP served by the fake
  forge, or bare repositories on disk that real git and the fake forge
  both reach.
- **Agents in the real loop:** hosted in the one loop when the worker
  spawns them, or real processes, each with a loop of its own, as in
  production.
- **How much sh:** which commands and syntax the subset takes. It stays a
  subset of POSIX, so that real sh agrees.
- **Where the shell's interpreter lives,** so that the machine and the
  forge's CI share it.
- **Differential checks:** whether a scenario must end the same way at
  every tier it runs at, as a check that the layers below the model
  decide nothing.
- **What the referee sees in the real loop:** the machine is real there
  and reports nothing, so an expectation about it, such as what an
  agent's environment holds, needs facts that say so, or is checked only
  in the tiers below.
- **The real loop's privileges:** the containment it shares with
  production needs user namespaces and a delegated cgroup on the test
  host.
