# The worker's model layer

Provisional, 2026-10-02. What the temper worker does, as a model layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of `programming-style.md`, and the agent it hosts is
described in `agent-model.md`. Each part's details are settled as it is
built; what is still open is listed in section 10, and what is not built
yet in section 11. How the worker is tested, and the fakes around it, is
in `testing-pyramid.md`.

## 1. In one page

- **A worker is an execution host.** The engine decides what work exists
  and holds the forge; the agent does the LLM work. The worker sits between
  them: it prepares checkouts, starts agent runs and keeps them in bounds,
  pushes the changes they make, and relays everything else between a run
  and the engine.
- **The worker keeps nothing.** What must survive lives in the forge
  (pushed branches, saved work) or with the engine (claims, snapshots). A
  worker can be killed at any moment and any worker can take any work;
  its checkouts are caches.
- **It hosts runs, not jobs.** A run may finish once, or wait for input,
  take events and park (agent-model.md, 4.3 and 4.5). The worker's unit is
  a hosted run with that lifecycle: start, fresh or from a snapshot;
  inbound events in; host calls out; park or end.
- **Policy is data.** The worker interprets no workflow vocabulary: no
  role, queue, action or verdict names. An assignment says which
  repositories to check out and how, what to save, and the charter to hand
  the agent.
- **Its one forge write is a git push:** a change its run accepted, or
  saved work. Push credentials stay in the worker's protocol layer; an
  agent never sees them. Everything else that touches the forge goes
  through the engine.
- **Every run is supervised.** A watchdog on progress, a bound on wall
  time, cancel that goes polite then kills, and a slot that comes back only
  once the run's process tree is gone.
- **The model is complete** (programming-style.md, section 4). A world of
  models and fakes runs everything the worker does, with no protocol and
  no io (section 9).

## 2. The worker in the system

```
engine   the forge's only API client: what work exists, for whom; claims; applies outcomes
worker   execution host: checkouts, agent runs, pushing their changes; relays the rest
agent    LLM work: one run per agent process, reporting to the worker
```

- **The worker dials the engine** and keeps one channel open. Assignments,
  inbound events, cancels, the answers to relayed calls and the
  acknowledgements of the worker's answers come down; host calls, facts,
  parks and ends go up. The engine never connects to a worker, so a worker
  can run wherever it can reach the engine. It dials at once, and redials
  with a jittered backoff that doubles up to a ceiling, starting over once
  a channel has proved itself.
- **Hello first.** On every channel the worker first says its slots (none
  once it is shutting down), the workstreams its checkouts hold, the runs
  it hosts with where each is (4.2), and those whose answers the engine
  has yet to acknowledge. Facts go only after it.
- **The engine never reads a repository.** It works from forge state, and
  what a checkout holds (a repository's checks, its `AGENTS.md`) is for the
  run to find (agent-model.md, section 2).
- **The engine holds the claim.** It claims work on the forge before
  assigning it, and decides what follows a run: apply its outcome, run it
  again on any worker, or park the work for a human. The worker never
  retries a run and never decides that work is done.
- **Attempts are fenced.** Every assignment carries an attempt. Once the
  engine has cancelled or reassigned an attempt, whatever it still sends
  is dropped; on the worker, a cancelled run's host calls are answered as
  unavailable, and an attempt assigned again while the worker hosts it is
  dropped. A stale attempt can never act over a newer one.
- **Answers are acknowledged.** The worker keeps each answer until the
  engine acknowledges it, and sends it again after every hello. The
  engine acknowledges an answer only once it has made it durable on the
  forge (engine-model.md, 4.2): a copy that comes before then is dropped
  unacknowledged, and one for an attempt the engine has finished with is
  acknowledged again.
  An answer not yet acknowledged keeps its slot. A refusal goes once and
  keeps nothing: one lost with the channel reads as a lost attempt.
- **Losing contact is survivable.** If the channel drops, runs go on for a
  grace period, and what they send the engine waits, never dropped:
  bounced events, and relays, unless their call has closed meanwhile (an
  outlet must not happen after its run was told it did not). On
  reconnecting, the worker says what it hosts and the engine keeps or
  cancels each run. Past the grace, the worker cancels them itself, saving
  their work first, and keeps their answers for the next channel.
- **Shutting down** cancels every run and admits no more. The worker is
  done once the engine has every answer. It gives the answers it keeps
  up, counted, only once no run is left and the engine is still out of
  reach past the grace; a channel that opens before then gets them after
  the hello.

## 3. Structure

```
temper-worker-model                 the worker loop's entry point: the engine link, routing
├── temper-worker-model-host        hosted runs: admit, prepare, start, relay, park or end
├── temper-worker-model-checkout    workspaces: prepare, commit, push, save; the cache
└── temper-worker-model-agent       agent processes: spawn, channel, watchdog, cancel then kill
```

The tree follows programming-style.md, 4.5: each sub-model is a step
machine with its own vocabulary, limits and world; a parent owns its
children's state and routes between them; siblings share no domain types.
`host` is the hub, as the run is in the agent: it knows a hosted run's
lifecycle and nothing of git, processes or channels. The other two are
capabilities, which the top level translates to and from through small
total functions. A hand-off goes from a capability to the host, from the
host to a capability, and at most once more back and forth, so each entry
point completes them before it returns, with no ready list.
`temper-worker-model` faces the protocol layer, and its vocabulary is what
crosses the model boundary: the engine's messages, the agent channel, git
operations, and the process and file operations of io.

## 4. Hosted runs

### 4.1 The assignment

What the engine gives the worker for one run:

- **Run and attempt.** The engine's names for the run and for this attempt
  at it; every message about the run carries both.
- **Workspace.** A workstream key, which names the checkout so later runs
  of the same work find it cached, and the repositories, each with: its
  name, the directory it sits in and what the agent calls it, one safe
  path component; its remote, the forge's address, which the protocol
  layer maps to a URL; where to start (a base branch, a branch, a commit
  by its full object id, or saved work); whether it may be written, and if
  so the branch a change is pushed to; and the identity for every forge
  operation on it (a name the protocol layer maps to credentials and a
  commit author).
- **Saving.** Whether to save unfinished work, and to which branch.
- **Charter.** What the agent's run is given (agent-model.md, 4.1). The
  worker adds where the repositories sit and passes the rest through.
- **Snapshot,** optional: the state of a parked run to resume from.

Its sizes and names are checked at the entrance against the worker's
limits, and so is a workspace with no repository or with a name twice.

### 4.2 Lifecycle

```
admit ─► prepare ─► start ─► active ⇄ waiting
                               │  host calls: push, served here;
                               │  forge reads and outlets, relayed to the engine
                               ▼
                         park or end ─► stop ─► save ─► release ─► answer
cancel, from any state ──────────────► stop ─► save ─► release ─► answer
```

1. **Admit** the assignment, or refuse it at the entrance: busy (every run
   slot is taken, an answer not yet acknowledged keeping its own; the run
   is hosted under another attempt; or the worker is shutting down) or
   invalid (beyond the limits).
2. **Prepare** the checkout (section 5).
3. **Start** an agent process with the charter, and the snapshot if there
   is one (section 6).
4. **Relay** while the run is live. Inbound events go down to the run as
   they arrive, a bounded few held until it is live; what does not fit,
   or comes as it ends, is bounced to the engine, which keeps it. Host
   calls come up, a bounded number per run and one push at a time; one
   more is answered as busy. Pushes are served by the worker (section 5);
   forge reads and outlets go to the engine and their answers come back.
   A call the run withdraws, past its own deadline for it, is answered as
   withdrawn at once if relayed; a push goes on to its end. A run that
   yields waits for its next inbound event, and holds its slot while it
   waits.
5. **Park or end,** as the run decides, its last word. A run that parks
   hands over a snapshot if it has one, and exits.
6. **Stop:** answer the run's relayed calls as unavailable, and wait until
   the agent process and everything it started are gone, killing them if
   they outstay the grace (section 6), and until a push in flight has
   settled: a push cannot be abandoned half way, and what it lands counts.
   Nothing touches the checkout while anything of the run is still
   running.
7. **Save** unfinished work, if the assignment asks: at park, at an
   unfinished end and at cancel. Commit what the writable repositories
   hold and push it to the saved-work branch, so a later run on any worker
   starts from it. A run that ended with a landed change has nothing left
   to save, and nor has one whose agent never started; one that landed
   mid-run, then parked, failed or was cancelled still saves.
8. **Release** the checkout to the cache.
9. **Answer** the engine, exactly once: ended with the run's outcome,
   parked with its snapshot, or failed with a typed failure; each with
   what the run left on the forge, the last commit landed in each
   repository (the head a pull request is to show) and how its save went.
   The engine decides what happens next, and its acknowledgement frees the
   slot.

A cancel from the engine, a lost channel or shutdown, or a fault of the
agent (the watchdog's among them), takes the same tail: stop, save,
release, answer. A cancelled run may still say how it finishes as it
winds down, and what it says first, before its agent has gone, is its
answer: a push that lands meanwhile is its outcome (agent-model.md, 4.4),
and a cancel it reports is the worker's. A run past its wall time winds
down the same way, but a cancel it reports is its wall time's failure; a
run stopped for any other fault is answered with the fault. Only a run
that says nothing is answered as the worker stopped it. The host arms no
timers: each wait is bounded below it, by the deadlines of git
operations, the agent's watchdog and wall time, and the grace of cancel,
then kill.

### 4.3 Failures

Typed, so the engine can act on them without reading prose:

- **Refused** at the entrance: busy, or invalid, saying what is beyond
  the limits.
- **Preparation failed:** transient (the forge could not be reached, the
  worker's side failed or ran out of time, or another run of the
  workstream holds its checkout), or permanent until something changes on
  the forge, naming the repository: the forge does not have it, a branch
  or a commit, or it refuses the identity.
- **The run failed,** as it reports it (agent-model.md, 4.2: model,
  budget, policy, stale).
- **The agent failed:** it could not be started, it exited without
  answering, it broke the channel's rules or said more than the limits
  allow, or the watchdog stopped it (no progress, or wall time).
- **Cancelled:** by the engine, for lost contact, or for shutdown.

Each carries bounded detail for operators, such as the tail of the
agent's error output; none of it is shown to an LLM.

## 5. Checkouts

The checkout sub-model owns the workspaces on the worker's disk. Its
vocabulary is typed git and file operations (make a workspace, clone,
fetch, create a branch, check out, commit, push), which the protocol layer
runs as contained processes and whose output it parses.

- **A cache, keyed by workstream.** The work on one item (implementing an
  issue, repairing its CI, resolving a conflict) shares a key, so its runs
  find the repositories already cloned. The engine runs them one at a
  time; a worker given a second while the first holds the checkout fails
  its prepare as transient. A cached checkout is reused only if it holds
  exactly the repositories named, by directory and remote, and is reset
  to the assignment's starting point: nothing local is authoritative.
  One that a killed or broken git may have damaged is not trusted, and is
  rebuilt from an empty directory, as a new one is. The cache holds a
  fixed number of checkouts; once they are all made, a new workstream
  takes the least recently used one that no run holds.
- **Repositories side by side.** A workspace holds one directory per
  repository, so code that refers to a sibling by path works, and the
  agent's write authority is the list of writable ones.
- **One operation at a time.** A run holds its checkout from the moment
  its prepare is admitted, so a cancel aborts a prepare rather than
  waiting it out. A checkout runs one operation at a time (a prepare, a
  push or a save), each a sequence of git operations with deadlines as
  data, one for the forge and one for the disk; the checkout arms no
  timers.
- **Prepare** fetches what the assignment names and checks out exactly its
  starting point. A base branch that does not exist yet is created on the
  forge from the default branch, for a writable repository only, and only
  created, never moved; for a read-only one it is missing.
- **Push commits what the run checked.** When a run asks to push, at
  `finish` with a change or proposing one mid-run, it has run the
  repository's checks with nothing else writing to the checkout
  (agent-model.md, 4.4); the worker commits exactly that tree and pushes
  the commit, with the run's message whole as its title. A push is a
  fast-forward and never forced: if the branch moved since the run
  started, the push fails and the run is told, which makes it the
  freshness check too.
- **Each repository lands on its own.** Pushing several is not atomic:
  the run is told done only if every repository with a change landed it,
  moved if a branch moved, and nothing to push if none had a change. A
  push the forge refused (permissions, a protected branch, a hook) is a
  landing of its own, which a retry will not change.
- **Pushes are never cancelled.** A push in flight runs to its end, which
  its deadline bounds, so one that lands is reported landed; one that
  ended ambiguously (out of time, broken, the forge lost on the way) is
  verified against the forge, by fetching its branch.
- **Saved work** is ordinary commits on the saved-work branch, pushed the
  same way. A run that resumes from saved work starts where the last one
  stopped. Work done since the last save is lost if the worker dies.
- **No credentials on disk.** Every git operation that reaches the forge,
  or commits, names the repository's identity, which the protocol layer
  maps to credentials or an author for that invocation only, never into a
  checkout's configuration, where an agent could read them.

## 6. Agent processes

The agent sub-model runs each hosted run in an agent process of its own.

- **One process per run.** Its process tree is the run's containment
  boundary: stopping a run means that tree is gone, which io proves. Its
  environment holds the LLM provider credentials the agent is configured
  with, and no forge credentials. It is spawned within a deadline, and a
  run whose agent could not be started fails as such. It has gone only
  once its process has exited, its tree is empty and its channel has been
  read to the end, so what it said before it went is heard.
- **One channel** over the process's pipes (agent-model.md, section 8).
  Down: the charter and snapshot first, then inbound events, answers to
  host calls and at most one cancel, in order. Up: host calls, named by
  the run and each answered once, also once the run has withdrawn it; the
  withdrawals; facts; a long operation, its span bounded by the limits,
  and its end; waiting, with how many inbound events the run has read;
  and how the run finishes, its last word. A call name reused while in
  flight, anything after the last word, a payload beyond the limits or a
  message that does not decode breaks the channel's rules: an agent
  failure.
- **A watchdog on progress.** Every message from the run counts as
  progress; silence past the no-progress deadline stops the run. The
  clock pauses while the run waits for an inbound event, having read
  every one sent to it, and while a host call waits for its answer, so a
  relayed call is bounded by the run's own deadline for it, past which it
  withdraws the call. It stretches to the span of a long operation the
  run reports, such as the repository's checks, until the run says it is
  done, so neither a human who has gone to lunch nor a two-hour test
  suite looks like a hang. A separate bound covers the run's wall time.
- **Cancel, then kill.** A cancel goes down the channel first, and the
  run winds down on its own (agent-model.md, 4.2); past a grace period
  the process tree is terminated, then killed. Wall time ends the same
  polite way, as the run is alive and may still report; a broken rule, no
  progress, or a channel hung up while the run is live terminate the tree
  at once. A run that has said how it finishes has the same grace to
  exit. Only an exited process and an empty tree release the slot.

## 7. Facts

What the run reports as it goes (agent-model.md, section 7) feeds the
watchdog and goes up to the engine, with the run's and the attempt's
names, for liveness, operators and live views, such as a session's
streaming text. Forwarding is best effort: a bounded queue, with what
does not fit dropped and counted, sent on an open channel only, after the
hello. Nothing the worker or the engine decides depends on a fact
arriving, and no fact holds a slot.

## 8. Below the model

What the protocol and io layers owe the model, to be designed after it:

- **The engine:** one framed channel, dialled by the worker and
  authenticated, carrying assignments, inbound events, cancels, host calls
  and their answers, facts, parks and ends, and acknowledgements. The
  run's facts go after what the model emitted, so none goes before the
  hello.
- **Agents:** spawning the agent within a deadline in a contained process
  tree (a delegated cgroup), with proof that the tree is empty after it
  stops; a framed channel over its pipes, whose start message the
  protocol layer completes with where the repositories sit; an
  environment without forge credentials.
- **Git:** each typed operation is one git invocation in a contained
  process, its output parsed into a typed outcome, and a commit named by
  its full object id (a SHA-1 id zero-padded to a SHA-256 id's 32 bytes).
  One that ran out of time or was cancelled ends only once its process
  tree has exited, so nothing of it touches the workspace afterwards. A
  repository's remote is mapped to a URL, and its identity to credentials
  or an author, per invocation; credentials never go in a URL or a file.
- **Files:** workspace directories. Making one empties it first, so what
  the cache evicted, or a worker left behind before a restart, cannot fail
  a prepare.

## 9. The world

The worker's worlds run the model against fakes that share none of its
types (programming-style.md, section 11):

- **an engine** that assigns, sends inbound events, cancels, answers
  relayed calls, acknowledges answers, and checks that every attempt is
  answered once;
- **a network** whose channel drops at random, for less and for more than
  the grace;
- **an agent** that follows a script: progress, host calls, yields, parks
  and ends, but also hangs, crashes, broken rules and ignored cancels;
- **git:** a remote of refs and commits, and local working trees, with
  fast-forward rules, failing fetches, refused pushes, another party
  moving a branch, and pushes that land and say they timed out.

Each sub-model has a world of its own, its parent and neighbours scripted
in it. The whole worker's world has all four, the agents' process trees
standing in for io, and shuts the worker down at random. It checks that
the engine takes each attempt's answer once, that what lands is exactly
the tree the agent left and saved work exactly the tree at the stop, that
no git operation runs while a run's agent may, but the push it asked for,
and that nothing is live once it settles.

The worker meets the real agent in the agent's top-level world: each
process it spawns is an agent model with a fake LLM provider, its tools
working in the trees the worker prepared, and the engine records the
outcome the run accepted. Its network and git are kept simple there, as
the whole worker's world covers their faults. The agent's worlds no
longer use a fake worker: the run's world scripts its host itself.

The fake engine is a step crate, as much of an engine as a worker meets,
and temporary: the engine's model takes its place once it exists.

testing-pyramid.md places these worlds among temper's tiers and tracks
their fakes. The working trees' remotes are on the fake forge, in one
store with its API, so that a branch the worker pushes is the head the
engine reads; the worlds reach it through git's transport, answered in
the same instant, since io's latency stands for the network and the
forge. The files and working trees are to become the machine, whose
programs, git among them, grow command-line faces for the protocol layer
to meet (testing-pyramid.md, sections 4.2 and 4.3).

## 10. Open questions

- **Saved-work branches:** their names, whether pushing to them triggers
  CI, and when the engine deletes them. A save from any other start onto
  a saved-work branch that exists finds it moved, so while one exists the
  engine starts runs from it, or deletes it first.
- **Inbound acknowledgement:** the worker bounces an inbound event a run
  could not take, saying why but not which event it was, and says nothing
  of those it delivered. The engine moves an item's inbox position
  (engine-model.md, 4.3) only past what a run took, so it needs to know
  which: a bounce could carry the event's name, or the run's answer could
  say how many events the run read.
- **A push's message:** the worker commits the run's message whole, as
  the title; the channel could carry a change's title and body apart.
- **Before the first channel:** the grace starts only when an open
  channel is lost, so a worker that has never reached the engine dials on
  without one; whether its failed dials want a grace of their own.
- **Snapshots:** what the agent puts in one; the worker carries it opaque,
  within its limits. The engine keeps them as a cache (engine-model.md,
  section 6). Until runs offer them, parking ends the run and resuming
  starts a fresh one from forge state (agent-model.md, 4.5).
- **Checks without an LLM:** validating an exact head needs a checkout
  and the repository's checks but no conversation. It could be a run whose
  outcome is its checks' result; add it when it is wanted.
- **Code graph:** indexing a checkout for codebase-memory-mcp as part of
  preparing it, and keeping that index with the cache.
- **Refusals and spend on the channel:** the channel has no refusal, so
  an agent that refuses a run reports it as failed for policy, and what a
  run spent has no place on it; whether the engine needs either.

## 11. Not built yet

The model layer runs everything above in its worlds. What it does not do
yet, each to be designed before it is built:

- **The agent's side of the channel** (section 6; agent-model.md,
  section 10): inbound events after the first request and the run's
  waiting, parking with a snapshot and resuming from one, and relayed
  calls (forge reads and outlets) with their answers. The worker speaks
  all of it; only scripted agents use it yet.
- **The layers below the model** (section 8): the engine's protocol, the
  agent channel's framing, git invocations and their parsing, contained
  process trees, workspace directories.
- **The engine.** The worker's worlds meet a fake one, temporary until
  the engine's model exists: its sub-models are built, the fleet among
  them, and its top level is being built (engine-model.md, sections 8
  and 16).
- **Saving periodically** (4.2), so a dying worker loses less: a run
  saves at park, at an unfinished end and at cancel only.
- **Checks-only runs and code-graph indexing** (section 10).
- **Agent configuration in a spawn:** a spawn carries the charter, the
  snapshot and where the repositories sit; what the agent is configured
  with (its LLM endpoints, their credentials, its limits) is not part of
  it yet.
- **Several workers in one world:** the fake engine places runs on one
  worker; placement across several, preferring one that holds the
  workstream's checkout, is the engine's (engine-model.md, section 8).
- **A workspace's repositories in parallel:** a checkout runs one
  operation at a time, so its repositories are cloned, fetched and pushed
  one after another.
