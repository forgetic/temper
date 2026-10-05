# Testing temper

Provisional, 2026-10-03. How temper is tested. The strategy is skein's
`docs/foundation/testing-strategy.md`: the tiers, faults, what every fake
shares, what every world checks, scenarios and the referee, the two
suites, and where a failure is fixed. This document keeps what is
temper's: its tiers (section 2), its neighbours and their fakes (sections
3 and 4), its scenarios and referee (section 5), what its worlds check
beyond the strategy (section 6), its layout (section 7), where things
stand (section 8), what is not built yet (section 9), and what is still
open (section 10).

## 1. In one page

- **Behaviour is tested in the domain layer.** The engine, the worker and
  the agent are each a root domain with child domains, and each has
  worlds of its own; system worlds run the real engine, worker and agents
  together. Domain and system worlds are all that is built so far: the
  tiers above wait for the layers they make real.
- **Peers across a wire, the machine beneath.** LLM providers, the forge
  and people on the web are peers, whose fakes grow into services and
  meet temper at the wire. The machine (files, processes, and the
  programs they run: git, rg, sh, temper's own) stays one set of state
  machines, with a face for each layer of temper it can sit under.
- **The fake forge is Forgejo-shaped,** API and git in one store, so a
  branch the worker pushes is the head the engine reads.
- **Checks follow content.** The machine's sh is a small subset of POSIX
  sh, in which fake repositories write their checks and their CI, so a
  change that is wrong fails in the run and on the forge alike.
- **The referee is built,** in the shared world harness, and every engine
  world and both system worlds hold their scenarios' expectations in one.
- **The fakes of temper's own components have retired:** the system
  worlds run the real engine and worker.

## 2. temper's tiers

| Tier (testing-strategy.md, section 2) | In temper |
|---|---|
| step tests | every domain crate and every fake; lib's are skein's |
| domain worlds | each child domain with the world as its parent, and each component's root domain with its child domains; most of temper's tests |
| system worlds | the engine, the worker and agents together (2.1) |
| machine worlds | skein's, for the machines it ships; temper's only for a machine of its own |
| protocol worlds | each component's protocol layer against its peers' fakes, the machine at its io face (2.2) |
| io worlds | skein's: temper does not retest io |
| simulated worlds | every process's `iterate` on skein's simulator (2.3) |
| real loop | temper as it ships, on the real kernel and a real machine (2.4) |

Every child domain has a world of its own but the engine's rules, which
keep no state and are tested by their step tests alone: a departure from
testing-strategy.md, 2.2, since with no state there is no sequence for a
world to drive.

### 2.1 System worlds

Two are built, both with the real engine driving the real worker: the
agent's top-level world, whose worker hosts real agents, and the whole
worker's world, whose worker hosts scripted agents over a channel that
fails (4.5). They aim at what crosses components: the tree an agent left
is the tree the worker pushes, and the outcome the run accepted is the
one the engine records.

### 2.2 Protocol worlds

Each component's protocol layer meets the protocol layers of its peers'
fakes, and the machine sits there at its io face (3.2). They test what
translation adds in temper: the JSON an LLM writes as a tool's input,
the git invocations and the parsing of their output. They run the system
worlds' scenarios, through bytes.

### 2.3 Simulated worlds

Every process in a scenario runs its `iterate` on skein's simulator: the
engine, the worker, the agents it spawns, and the fakes of the peers as
services. A spawn the worker asks for starts an agent's service, and a
kill ends it. The machine sits behind the simulator (3.2).

### 2.4 The real loop

temper as it ships, with one io_uring loop driving everything: the fake
peers as services listening on loopback sockets, the engine, the worker
and the agents it spawns. The machine is real: a sandbox directory,
inside the containment production uses (agent-domain.md, section 6;
worker-domain.md, section 8), with real git, rg and sh. Beyond what the
strategy lists, it shows that the containment holds (section 6), and
git's actual output.

## 3. Neighbours and their faces

### 3.1 Peers

A peer's fake grows the layers that the temper component facing it
grows, and each tier joins the two at the lowest layer both have
(testing-strategy.md, 4.1):

```
agent    domain ─ protocol ─ io   ⇄   io ─ protocol ─ domain   fake LLM provider
engine   domain ─ protocol ─ io   ⇄   io ─ protocol ─ domain   fake forge
engine   domain ─ protocol ─ io   ⇄   io ─ protocol ─ domain   fake people, on the web
```

A peer can be a client as well as a server: the fake forge serves its API
and git, and sends webhooks to the engine.

### 3.2 The machine

The machine's fake grows no protocol or io layers (testing-strategy.md,
4.3). It shows a face to whichever layer of temper sits just above it:

| temper is real down to | The machine's face | It answers |
|---|---|---|
| the domain | domain face | typed operations: the tools' file operations, commands and searches; the checkout's git operations |
| the protocol layer | io face | io's records: open, read, store, rename, spawn with an argument vector, bytes on a pipe, an exit |
| io | behind the simulator | the file and process operations the simulator passes on |
| everything | none | the real kernel, in a sandbox |

Every component's files and processes go through it: the worker's
workspaces and agent processes, the agent's tools and checks, and the
engine's store.

## 4. The fakes

A fake that more than one tier needs is a step crate under `testing/`;
what only one world needs stays in that world, as a script specialised to
what the world aims at. What every fake shares, from its own vocabulary to
faults drawn from the seed, is the strategy's (testing-strategy.md,
section 4).

### 4.1 LLM providers

`testing/temper-fake-llm-domain` is a provider seen from the inside. It
answers each call after a drawn latency, by a world's script or at random
(failures by configured chance, a number of tool rounds, then a final
answer), and rejects conversations a real provider would reject.

It grows the server side of each provider API the agent speaks: HTTP,
server-sent events, the provider's JSON, its errors and rate limits, and
an OAuth server that rotates refresh tokens (llm.md, section 11;
credentials.md, section 10). It checks its client as the providers do:
the bearer, ChatGPT's account id, Anthropic's identity, and the opaque
blocks it gave coming back unchanged. Its faults move down with it: a
stream cut mid-event or between events, a malformed event, a chunk size
that lies, a slow trickle, a stall with pings only.

### 4.2 The forge

The fake forge holds what temper meets on a forge: repositories, issues,
pull requests, comments, labels, reviews, CI, merges, wikis and webhooks;
and the git a worker clones, fetches and pushes. Both live in one store,
so a branch the worker pushes is the head the engine reads through the
API, and a merge the engine asks for moves the branch the next fetch
sees.

- **A domain first.** The engine's worlds need one (engine-domain.md,
  section 14): CI that passes, fails or never reports; merges that
  conflict; webhooks that come late or not at all; requests that fail or
  hit a rate limit; people who comment and edit. Its git keeps git's
  rules: a push moves a branch only by a fast-forward, and another party
  can move a branch under a run.
- **Then a service:** Forgejo's API and GitHub's, the subset temper uses,
  over one domain; signed webhooks sent to the engine; and git over HTTP,
  the subset real git needs to clone, fetch and push. It keeps Forgejo's
  quirks (its default order, its page cap, its unpaged threads) and
  counts the requests it serves, which the engine's cost checks hold to
  ceilings (forge.md, sections 9 and 10).
- **CI follows content.** A repository's CI runs its checks in the shell
  subset (4.3) on the pushed tree, so a change that is wrong fails on the
  forge as it fails in the run.

The domain is built, as `testing/temper-fake-forge-domain`, a step crate
with a vocabulary of its own. What building it settled:

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
  (`temper-fake-checkout`) reaches the forge through a typed transport,
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
  protocol is its command line: a domain of what it does to the files, and
  a face that takes an argument vector and prints the bytes the real
  program prints. The programs are git, whose working trees are the
  machine's and whose remotes are on the fake forge; rg; sh; and temper's
  own. The agent is a program the worker spawns, with its channel on the
  program's pipes.

At the domain face, a world calls what a program does directly: a typed
fetch is the git program's fetch, with no argument vector. From the io
face down, every program runs through its command line. That is what
tests the protocol layer's invocations and its parsing, and only the
invocations temper makes need a face.

The forge's git is not the machine's. The git program reaches the fake
forge as real git does, across the network: through a typed transport
that the world or the simulator routes, and over HTTP in the real loop.
That split is built: `temper-fake-checkout` keeps working trees and
their git directories, the commits a tree cloned, fetched or made among
its files, while the forge keeps the repositories, and the transport
carries git's calls between them (where a repository's branches are, a
fetch, a push, a branch created for a base).

The checkout's merge world independently predicts complete trees, both
parents and remote refs for clean and conflicted merges, unchanged merge
trees, saved work, stale expected heads and pushes that land but time out.
Each scenario settles all its typed promises, replays from its seed, and
has the same requests and effects with facts disabled. Its counted-memory
test includes construction and maximum conflict-path sets in retired
preparations alongside live pushes before reclaim.

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
correct notes (engine-domain.md, section 14), on the forge and on the
web. Their fake is a scripted actor, drawn from the seed. On the fake
forge it acts as a forge user. On the web it is a client of the engine's
web protocol, a peer like the others, and it grows that protocol's client
side when the web is built.

Today people are scripted in the engine's world, ordinary Rust rather
than a domain: each story's person looks at the forge as observed, never
at its store, and does what the story calls for next, one thing at a
time, so a call lost to a restarting engine is simply tried again. On the
web they make the engine's own calls, which no protocol carries yet. The
whole worker's world reuses them; the agent's top-level world scripts its
own, as its agents cannot take a session's turns yet.

#### 4.4.1 The people child domain's parent and store

The new `tests/engine/people` world plays the root, authority and tasks
for increment 03a. Sign-ins, authoritative roles and keyed chat requests
reach the real people child. The root scripts authority's outcome, makes
a task and commits it alongside people's answer record. It holds every
reply behind its commit, including a duplicate whose record exists only
in the domain's state ahead of the store.

Cold restarts keep only durable people records and task creations.
Scenarios cut before durability, after durability before reply delivery,
and while routing temporarily waits. Client calls lost with the process
are abandoned and retried with a fresh reply destination and the same key.
The referee observes client replies and durable task creations, checking
reply order, roles and one creation per key; negative tests check each
safety rule and a missing reply's deadline. Focused tests cover signing
in, expiry, roles, initial-owner bootstrap, duplicate waiters and refused
admission, replay, inert facts and counted peak memory. The fuzzy matrix
settles every world and reaches each named ending within the people's
world allotment (`docs/plans/next-domain/README.md`, 5.5).

#### 4.4.2 The tasks child domain's parent and executors

`tests/engine/tasks` drives the real tasks child with always-allow
authority and scripted agent executors in increment 02a. The parent
commits task records and requester result mail together, and withholds
replies, assignments, stops and closing requests until durability.
Preparation, claimed attempts and actual running work are distinct;
`Close`/`Settled` represents effects and resource release at the root.

The independent referee observes durable task creation, assignments,
terminals, closing/settlement and requester results. It checks whole
batches from actual durable creations, cycles through dependency and
delegation waits, dependency order, one run per task, increasing attempts,
once-only durable replies/results, limits and every cancelled descendant
ending before its requester. A table independently violates every initial
rule. Focused stories cover failures in every class, assignment/preparation
refusals, holds and release, a three-level cancellation and retained input
stubs. Restart cuts independently bracket durability of make, claim,
terminal, cancel and settlement decisions. Counting-allocator tests fill
payloads, graph edges, stubs, alarms, held closing results and retired
slots, and cold-restore the records against the child's worst-case bound.
Replay agrees with facts consumed or left full. The seeded fuzzy lifecycle
sweep adds failures, held/released tasks, failed dependency results,
refusals, restarts and complete subtree cancellation within the tasks
world allotment (`docs/plans/next-domain/README.md`, 5.5). Later increments
extend this same world with procedures, amendments, proposals and funding.

The 02b parent completes result, notice, historical-result read and timer
callbacks before making a decision durable, then releases actual relays to
its scripted running agent. Each claim names its brief's readable IDs.
The separate inbox referee observes persisted rows and committed reads,
checking whole-message and promise capacity, message identities, offer
fences, reference bounds, callback closure and exactly the offered prefix
consumed by a turn. Its negative table violates each new invariant and each
read rule independently. Focused stories cut message admission and turns
before/after durability, merge a newer hint over an old offer, keep a
policy-deferred older message, fill inboxes while accepting promised results
and answers, cancel at full capacity, accept an introduced dependency and
refuse a cycle spanning existing subtrees, restore held inboxes and timers,
and clean subscriptions at owner end. The randomized lifecycle story now
also asks/answers a question, mixes classified news and person words, commits
or discards a drawn turn before restart, and replays with facts consumed or
full. Counted-heap saturation includes inbox, offer, receipt, question and
subscription arenas and cold restoration. The whole tasks world keeps its
0.5 s focused and 4 s fuzzy serial allotments across increments.

### 4.5 Temper's own components

A fake worker served until the worker's domain was built, and a fake
engine until the engine's was. Both have retired: the system worlds run
the real engine, each wiring it as the protocol layers would.

- **The agent's top-level world** (`tests/agent/domain`): the
  real engine, worker and agents; the fake LLM provider; one fake forge,
  which the engine reaches through the forge's protocol as the engine's
  world plays it, and io's git through the working trees' transport; the
  engine's store; people who hand in issues whose step is an agent step
  or a change, each with a record as an earlier life of the engine would
  have written it (the agent's side cannot take a session's turns yet), a
  reviewer, and a person who stops a run now and then. Its channel
  between engine and worker keeps its order and never drops.
- **The whole worker's world** (`tests/worker/domain`): the
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

A scenario (testing-strategy.md, section 7) is set up in the terms of
temper's fakes: each conversation's script for the LLM, the repositories
on the forge with their files and checks, the items and the people, and
the faults to inject. A scenario cues its scripts through content, as the
agent's top-level world does today: a repository's files say which script
its run follows.

### 5.2 The referee

The referee is the strategy's (testing-strategy.md, section 7). temper's
expectations are of this kind: safety, such as nothing landing on a
default branch without green CI, or no forge credential reaching an
agent; liveness, such as a pull request opened, with green CI, within
the hour. What temper adds:

- **It steps on what it observes,** and on its own timers, not on every
  iteration, so its work per iteration stays bounded. Its earliest
  deadline is covered by the shell's one ring timeout, as every layer's is
  (skein's `docs/foundation/programming-model.md`, section 9).
- **A failure in the real loop** is reported with the trace, since the
  seed does not replay there.

The referee is built in the shared harness, `tests/world`,
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
take the allowed holds as a parameter. The worker's child domain worlds,
and the agent's tools', session's and run's, keep their checks inline.

## 6. What the tiers check

Every world checks what the strategy lists (testing-strategy.md, section
6). In temper:

- **Contracts** include each operation's deadline and identity, and each
  component's own (worker-domain.md, section 9).
- **Memory** is measured per step: the counting allocator records the
  most a domain held in one step, against its worst case.
- **Replay** compares traces: a seed replays to the same trace. No world
  compares a digest of the state yet.
- **The real loop** adds that production's containment holds: a stopped
  run's process tree is empty, and a command cannot write a git directory.

### 6.1 Focused tests and fuzzy tests

The two suites are the strategy's (testing-strategy.md, section 8), run
by the commands in `docs/development/workflow.md`. In temper:

- **Focused tests** are the step tests, and each world's scenarios,
  referee tests, replay, facts changing nothing (a run is the same
  whether its facts are kept or dropped), and memory at the worst case:
  every test binary of a world but its fuzzy ones.
- **Fuzzy tests** are each world's `tests/fuzzy_*.rs` binaries. A sweep
  settles each random world under every invariant, and reaches every
  ending among them.

A fuzzy test lives in its world's crate, beside the focused ones, and
uses that world: a world's settings, random ones included, are the
world's, not either suite's. The suites are told apart by binary name.
Each fuzzy file is a binary of its own, so a memory test's counting
allocator never runs under a sweep.

### 6.2 Worker runtime v2 extension

`tests/worker/domain/src/next.rs` drives the real worker root and its three
children against scripted engine and process records and deterministic git
terminals. The existing v1 system-world scripts and random draws are
unchanged. Its independent boundary referee checks complete turn replay,
exact names and cumulative spend, read credit, identical answers, separate
push title/body, merge paths and expected heads, and release at iteration
reclaim points. Focused stories cover busy backoff, reconnects, crossed
acknowledgements, ordinary saved work and snapshot-free merge parking.
Replay compares digests of the full boundary records and facts; dropping
facts leaves every boundary record unchanged. Referee tests inject a
changed duplicate body and an unseen turn prefix and require rejection.
Counted memory in the root and both changed child worlds exercises
saturated turn windows, transcripts and conflict paths; a small
deterministic sweep varies body lengths and one-to-three count and
byte windows. This runtime extension uses the existing worlds' shared
suite headroom (next-domain/README.md, 5.5); legacy focused coverage stays.

## 7. Layout

```
crates/*/src/tests.rs           step tests
testing/temper-fake-*           fakes, as step crates: the forge and the LLM provider
tests/world                     the world harness: schedule, stage, ledger, trace, heap, referee
tests/<component>/<child>       a child domain's world, temper-<component>-<child>-world
tests/<component>/domain        a component's world: the engine's, and the system worlds
tests/*/*/tests/*.rs            a world's focused tests
tests/*/*/tests/fuzzy_*.rs      its fuzzy tests: random worlds, domains driven at random
tests/fake-checkout             the machine and working trees' git, today
tests/fake-forge                the fake forge's tests in ordinary Rust: its memory
tests/fake-llm                  the fake provider's tests in ordinary Rust: its memory
```

Under `tests/<component>/domain`, the engine's is its own
world, and the agent's and the whole worker's are system worlds (2.1);
the engine world's crate is a library the system worlds use (4.5). The
world harness is temper's until a second service needs it
(testing-strategy.md, section 7).

The protocol worlds find their homes beside the system worlds they
replay: the link (the engine and a worker) and an agent's channel
(a worker and its agents), then an agent against the fake provider, and
the engine against the fake forge (channel.md, section 15; llm.md,
section 12; forge.md, section 10). The simulated worlds and the real loop
find theirs when the first of each is built.

## 8. Where things stand

As of 2026-10-03.

| Tier | Built |
|---|---|
| step tests | every domain crate and the fakes |
| domain worlds | the agent's tools, session and run; the worker's checkout, agent and host; the legacy engine's work, plan and forge, the new people and tasks children, fleet, brief, notes and views (the rules by step tests alone); the engine's own |
| system worlds | the agent's top-level world: the engine, the worker and agents; the whole worker's: the engine, the worker and scripted agents |
| machine worlds | none: temper has no machine of its own |
| protocol worlds | none: no protocol layer exists |
| simulated worlds | none: no service or shell exists |
| real loop | none |

The fakes:

- **LLM provider:** its domain, `testing/temper-fake-llm-domain`.
- **Engine:** retired; the system worlds run the real one (4.5).
- **Forge:** its domain, `testing/temper-fake-forge-domain` (4.2), with
  its git. The forge child domain's world, the engine's and the system
  worlds run against it, the forge child domain world's translation
  standing in for the protocol layer; io's git reaches it through the
  working trees' transport, with no latency or faults of its own there, as
  io's stand for them. It has no inline review comments, and no
  description or link on a status, which the worlds answer empty.
- **Machine:** `temper-fake-checkout`, ordinary Rust rather than a step
  crate, at its domain face only, through each world's translation. It has
  files with versions, links and roots, a search, and the working trees'
  git. A command is matched whole and answered with canned output and
  file changes, whatever the files hold. Agents are programs already, at
  the domain face: the agent's top-level world starts an agent domain for
  each spawn, with its channel on the process's pipes.
- **People:** scripted in the engine's world (4.4), and reused by the
  whole worker's; the agent's top-level world scripts its own, and the
  engine child domains' worlds script what people do through the parent
  they script.

The checks:

- **Memory** is measured in every world, and the fake forge's and provider's
  against their worst cases in `tests/fake-forge` and `tests/fake-llm`.
  The provider caps queries, stored scripts and delayed answers, counting
  their arrays and payloads, and checks reply sizes before making them.
- **Replay** is checked in every world, by its trace.
- **Transition coverage** is not measured, and nothing is fuzzed yet:
  there is no protocol machine or decoder for `cargo fuzz`, and no
  domain's step functions are fed recorded events.
- **Scenario expectations** are a referee's (5.2) in every engine world
  and both system worlds. The worker's child domain worlds, and the
  agent's tools', session's and run's, check theirs inline, beside their
  contracts. The engine's referee rejects repeated keyed creations across
  a restart at a drawn moment as well as within one life.
- **Focused scenarios** in the engine's world tell each of its stories
  (engine-domain.md, section 14) on one seed, among them a tracking label
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
  world places runs on scripted ones; worker-domain.md, section 11); in
  the agent's top-level world, what the agent's side lacks
  (agent-domain.md, section 10): sessions, inbound events, waiting,
  parking and relayed calls; in the whole worker's world, plans' stories,
  which run in the engine's world only.
- **Protocol worlds,** each with the protocol layer it tests, and the
  engine's cost checks through bytes (forge.md, section 10).
- **Transcripts** of the real peers for the machines and decoders:
  LLM providers' streams, captured through their own clients, and
  Forgejo's answers and webhooks, recorded by the real Forgejo check
  (llm.md, section 12; forge.md, section 10).
- **Simulated worlds,** with the services and the shells, on skein's io
  and simulator.
- **The real loop:** a shell that drives every service in one loop, and
  the sandbox.
- **The referee** in the worlds that still check their scenarios inline,
  so that those checks run at every tier.
- **Checks:** transition coverage; fuzzing, starting with the domains'
  step functions fed recorded events; and a digest of the state on
  replay.

By fake:

- **LLM provider:** its protocol layer and service.
- **Forge:** inline review comments, statuses' descriptions and links,
  commit messages, more than one protected branch in a repository, and CI
  that runs checks in the shell subset; then its protocol layer (Forgejo,
  GitHub, webhooks, git over HTTP) and service.
- **Machine:** a step crate; programs as step machines with command-line
  faces; the shell subset; the io face, and its place behind the
  simulator; the real loop's sandbox.
- **People:** inboxes, person tasks and adoption after the new child's
  signing-in, roles and keyed requests; then the client side of the web's
  protocol.

## 10. Open questions

- **The forge's git in the real loop:** git over HTTP served by the fake
  forge, or bare repositories on disk that real git and the fake forge
  both reach.
- **How much sh:** which commands and syntax the subset takes. It stays a
  subset of POSIX, so that real sh agrees.
- **Where the shell's interpreter lives,** so that the machine and the
  forge's CI share it.
- **Differential checks:** whether a scenario must end the same way at
  every tier it runs at, as a check that the layers below the domain
  decide nothing.
- **What the referee sees in the real loop:** the machine is real there
  and reports nothing, so an expectation about it, such as what an
  agent's environment holds, needs facts that say so, or is checked only
  in the tiers below.
- **The real loop's privileges:** the containment it shares with
  production needs user namespaces and a delegated cgroup on the test
  host.

## 11. Deferred conformance issues

- **The fake provider's Rust subset.** `cued` in
  `testing/temper-fake-llm-domain/src/respond.rs` uses a `while let` loop
  to find a script's cue. Each round consumes a byte of the bounded query,
  so execution terminates, but programming-model.md, sections 3 and 10.3,
  forbids `while` in step code. Rewriting it as a bounded `for` is deferred;
  this records the discrepancy rather than granting a general exception.

### Session extension (migration 05e)

The session world's existing v1 scenarios and fuzzy sweep stay intact.
`tests/agent/session/src/recorded.rs` is a scripted v2 parent/provider
world: its referee observes requests, provider message bytes, concrete turns
and cumulative spend. It computes expected charges independently from token
counts, checks the original opaque bytes and provider ids/input, and feeds
its durable turn values into a new session. The harness separately checks
request terminals, output bounds and settled session/run/kit counts.

Focused cases cover fresh v2 admission, transcript identity/structure/size
and unresolved-ticket refusals before effects, concrete committed results
after a turn, unanswered yielded calls, per-completion rounding, overflow,
child spend exactly once before and after reclaim, and cancellation races.
The same observations replay and remain identical with facts disabled.
The counting allocator also fills a v2 delegated transcript and restores
concrete history exactly to their byte cap, counting the turn and provider
copies at emission against the session's declared worst case. The bound
includes the record envelopes while incoming messages move into the session's
bounded transcript list. A separate fuzzy test varies token counts,
child spend, concrete result sizes and the closing/terminal races over 64
seeds. It adds no runtime-system scenario and changes no legacy case.
