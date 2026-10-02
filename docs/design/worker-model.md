# The worker's model layer

Provisional, 2026-10-02. What the temper worker does, as a model layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of `programming-model.md`, and the agent it hosts is
described in `agent-model.md`. Each part's details are settled as it is
built; what is still open is listed in section 10.

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
- **The model is complete** (programming-model.md, section 4). A world of
  models and fakes runs everything the worker does, with no protocol and
  no io (section 9).

## 2. The worker in the system

```
engine   the forge's only API client: what work exists, for whom; claims; applies outcomes
worker   execution host: checkouts, agent runs, pushing their changes; relays the rest
agent    LLM work: one run per agent process, reporting to the worker
```

- **The worker dials the engine** and keeps one channel open. Assignments,
  inbound events, cancels and the answers to relayed calls come down; host
  calls, facts, parks and ends go up. The engine never connects to a
  worker, so a worker can run wherever it can reach the engine.
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
  unavailable. A stale attempt can never act over a newer one.
- **Losing contact is survivable.** If the channel drops, runs go on for a
  grace period; on reconnecting, the worker says what it hosts and the
  engine keeps or cancels each run. Past the grace, the worker cancels them
  itself, saving their work first.

## 3. Structure

```
temper-worker-model                 the worker loop's entry point: the engine link, routing
├── temper-worker-model-host        hosted runs: admit, prepare, start, relay, park or end
├── temper-worker-model-checkout    workspaces: prepare, commit, push, save; the cache
└── temper-worker-model-agent       agent processes: spawn, channel, watchdog, cancel then kill
```

The tree follows programming-model.md, 4.5: each sub-model is a step
machine with its own vocabulary, limits and world; a parent owns its
children's state and routes between them; siblings share no domain types.
`host` is the hub, as the run is in the agent: it knows a hosted run's
lifecycle and nothing of git, processes or channels. The other two are
capabilities. `temper-worker-model` faces the protocol layer, and its
vocabulary is what crosses the model boundary: the engine's messages, the
agent channel, git operations, and the process and file operations of
io.

## 4. Hosted runs

### 4.1 The assignment

What the engine gives the worker for one run:

- **Run and attempt.** The engine's names for the run and for this attempt
  at it; every message about the run carries both.
- **Workspace.** A workstream key, which names the checkout so later runs
  of the same work find it cached, and the repositories: for each, where
  to start (a base branch, a branch, a commit, or saved work), whether it
  may be written, the branch a change is pushed to, and the push identity
  (a name the protocol layer maps to credentials).
- **Saving.** Whether to save unfinished work, and to which branch.
- **Charter.** What the agent's run is given (agent-model.md, 4.1). The
  worker adds where the repositories sit and passes the rest through.
- **Snapshot,** optional: the state of a parked run to resume from.

Its size is bounded by the worker's limits and checked at the entrance.

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
   slot is taken) or invalid (beyond the limits).
2. **Prepare** the checkout (section 5).
3. **Start** an agent process with the charter, and the snapshot if there
   is one (section 6).
4. **Relay** while the run is live. Inbound events go down to the run as
   they arrive; host calls come up. Pushes are served by the worker
   (section 5); forge reads and outlets go to the engine and their answers
   come back. A run that yields waits for its next inbound event, and
   holds its slot while it waits.
5. **Park or end,** as the run decides. A run that parks hands over a
   snapshot if it has one, and exits.
6. **Stop:** wait until the agent process and everything it started are
   gone, killing them if they outstay the grace (section 6). Nothing
   touches the checkout while anything of the run is still running.
7. **Save** unfinished work, if the assignment asks: commit what the
   writable repositories hold and push it to the saved-work branch, so a
   later run on any worker starts from it. A run that ended with an
   accepted change has nothing left to save.
8. **Release** the checkout to the cache and the slot to the next
   assignment.
9. **Answer** the engine, exactly once: ended with the run's outcome,
   parked with its snapshot, or failed with a typed failure. The engine
   decides what happens next.

A cancel from the engine, the watchdog, a lost channel or shutdown takes
the same tail: stop, save, release, answer.

### 4.3 Failures

Typed, so the engine can act on them without reading prose:

- **Refused** at the entrance: busy or invalid.
- **Preparation failed:** the forge could not be reached (transient), or a
  repository, branch or commit does not exist (permanent).
- **The run failed,** as it reports it (agent-model.md, 4.2: model,
  budget, policy).
- **The agent failed:** it exited without answering, it broke the channel's
  rules, or the watchdog stopped it (no progress, or wall time).
- **Cancelled.**

Each carries bounded detail for operators, such as the tail of the
agent's error output; none of it is shown to an LLM.

## 5. Checkouts

The checkout sub-model owns the workspaces on the worker's disk. Its
vocabulary is typed git and file operations (fetch, check out, commit,
push, remove), which the protocol layer runs as contained processes and
whose output it parses.

- **A cache, keyed by workstream.** The work on one item (implementing an
  issue, repairing its CI, resolving a conflict) shares a key, so its runs
  find the repositories already cloned. A cached checkout is reset to the
  assignment's starting point before use: nothing local is authoritative.
  The cache is bounded by count, and evicts the least recently used
  checkout that no run holds.
- **Repositories side by side.** A workspace holds one directory per
  repository, so code that refers to a sibling by path works, and the
  agent's write authority is the list of writable ones.
- **Prepare** fetches what the assignment names and checks out its
  starting point. A base branch that does not exist yet is created on the
  forge from the default branch, and only created, never moved.
- **Push commits what the run checked.** When a run asks to push, at
  `finish` with a change or proposing one mid-run, it has run the
  repository's checks with nothing else writing to the checkout
  (agent-model.md, 4.4); the worker commits exactly that tree and pushes
  the commit. A push is a fast-forward and never forced: if the branch
  moved since the run started, the push fails and the run is told, which
  makes it the freshness check too.
- **Saved work** is ordinary commits on the saved-work branch, pushed the
  same way. A run that resumes from saved work starts where the last one
  stopped. Work done since the last save is lost if the worker dies.
- **No credentials on disk.** Identities and tokens go to each git
  invocation that needs them, never into a checkout's configuration, where
  an agent could read them.

## 6. Agent processes

The agent sub-model runs each hosted run in an agent process of its own.

- **One process per run.** Its process tree is the run's containment
  boundary: stopping a run means that tree is gone, which io proves. Its
  environment holds the LLM provider credentials the agent is configured
  with, and no forge credentials.
- **One channel** over the process's pipes (agent-model.md, section 8):
  the charter and snapshot going in, then inbound events, cancels and
  answers to host calls going down; host calls, facts, parks and ends
  coming up. Malformed or out-of-order traffic is an agent failure.
- **A watchdog on progress.** The run's facts (an LLM call or a tool
  started or finished) count as progress; silence past the no-progress
  deadline stops the run. The clock pauses while the run waits for an
  inbound event or for a host call to be served, and stretches to the
  deadline of a long operation the run reports, such as the repository's
  checks, so neither a human who has gone to lunch nor a two-hour test
  suite looks like a hang. A separate bound covers the run's wall time.
- **Cancel, then kill.** A cancel goes down the channel first, and the
  run winds down on its own (agent-model.md, 4.2); past a grace period
  the process tree is terminated, then killed. Only an exited process
  and an empty tree release the slot.

## 7. Facts

What the run reports as it goes (agent-model.md, section 7) feeds the
watchdog and goes up to the engine for liveness, operators and live
views, such as a session's streaming text. Forwarding is best effort: a
bounded queue, with what does not fit dropped and counted. Nothing the
worker or the engine decides depends on a fact arriving, and no fact
holds a slot.

## 8. Below the model

What the protocol and io layers owe the model, to be designed after it:

- **The engine:** one framed channel, dialled by the worker and
  authenticated, carrying assignments, inbound events, cancels, host calls
  and their answers, facts, parks and ends.
- **Agents:** spawning the agent in a contained process tree (a delegated
  cgroup), with proof that the tree is empty after it stops; a framed
  channel over its pipes; an environment without forge credentials.
- **Git:** each typed operation is one git invocation in a contained
  process, its output parsed into a typed outcome; credentials passed per
  invocation, never in a URL or a file.
- **Files:** workspace directories, and removing them when the cache
  evicts.

## 9. The world

The worker's world runs the model against fakes that share none of its
types (programming-model.md, section 11):

- **an engine** that assigns, sends inbound events, cancels, answers
  relayed calls, and checks that every assignment is answered once;
- **an agent** that follows a script: progress, host calls, yields, parks
  and ends, but also hangs, crashes and ignored cancels;
- **git:** a remote of refs and commits, and local working trees, with
  fast-forward rules and failing fetches.

The worker and the agent meet in a larger world, where the real agent
model, with a fake LLM provider, takes the scripted agent's place.

## 10. Open questions

- **Saved-work branches:** their names, whether pushing to them triggers
  CI, and when the engine deletes them.
- **When to save:** at park and at an unfinished end only, or also
  periodically, so a dying worker loses less.
- **Snapshots:** what the agent puts in one, its size limit, and where
  the engine keeps it. Until runs offer them, parking ends the run and
  resuming starts a fresh one from forge state (agent-model.md, 4.5).
- **Checks without an LLM:** validating an exact head needs a checkout
  and the repository's checks but no conversation. It could be a run whose
  outcome is its checks' result; add it when it is wanted.
- **Code graph:** indexing a checkout for codebase-memory-mcp as part of
  preparing it, and keeping that index with the cache.
- **Placement:** whether the engine prefers a worker that holds a
  workstream's checkout; the worker could report its cached workstreams.
