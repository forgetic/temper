# The worker

Provisional, 2026-10-04. What the temper worker does, as a domain layer:
its parts, what each is responsible for, and how they fit together. It
hosts the runs of the engine's agent tasks (engine.md, section 7); the
agent it starts is smith, as agent.md says, and the worker is its host
(smith's `host.md`). The mechanics are those of skein's
`docs/foundation/programming-model.md`. What is still open is listed in
section 12, and what it defers in section 13.

## 1. In one page

- **A worker is an execution host.** The engine decides what work exists
  and keeps it in its store; the agent does the LLM work. The worker sits
  between them: it prepares workspaces, starts agent runs and keeps them
  in bounds, pushes the changes they make, and relays everything else
  between a run and the engine.
- **The worker keeps nothing durable.** What must survive lives in the
  engine's store (claims, transcripts, results) or on the forge (pushed
  branches, saved work). A worker can be killed at any moment and any
  worker can take any run; its checkouts are caches.
- **It hosts runs, not jobs.** A run may finish once, or wait for
  messages, take them and park (smith's `run.md`, section 6). The worker's unit is a
  hosted run with that lifecycle: start, fresh or from a transcript;
  messages in; tool calls out; turns up; park or end.
- **Policy is data.** The worker interprets no workflow vocabulary: no
  task, role, verdict or tool names. An assignment says which
  repositories to check out and how, what to save, and the charter to
  hand the agent, which is smith's and which it carries as bytes.
- **Its one forge write is a git push:** a change its run accepted, or
  saved work. Push credentials stay in the worker's protocol layer; an
  agent never sees them. Everything else that touches the forge goes
  through the engine.
- **Nothing it sends is lost.** A run's turns and its answer are kept
  until the engine acknowledges each, once it has committed it, and sent
  again after every hello.
- **Every run is supervised.** A watchdog on progress, a bound on wall
  time, cancel that goes polite then kills, and a slot that comes back
  only once the run's process tree is gone.
- **The domain is complete** (programming-model.md, section 4). A world of
  domains and fakes runs everything the worker does, with no protocol and
  no io (section 10).

## 2. The worker in the system

```
engine   tasks, authority, connectors, the store: what runs, with what; commits what runs say
worker   execution host: workspaces, agent runs, pushing their changes; relays the rest
agent    LLM work: one run per agent process, reporting to the worker
```

- **The worker dials the engine** and keeps one channel open.
  Assignments, messages for runs, cancels, the answers to relayed calls,
  credential grants, and the acknowledgements of the worker's turns and
  answers come down; tool calls, turns, facts, parks and ends go up. The
  engine never connects to a worker, so a worker can run wherever it can
  reach the engine. It dials at once, and redials with a jittered backoff
  that doubles up to a ceiling, starting over once a channel has proved
  itself.
- **Hello first.** On every channel the worker first says its slots (none
  once it is shutting down), how long it may take to stop a run it lost
  contact for (its grace, then the longer of its cancel's grace and a
  push's deadline, then a save's commit and push), the
  workstreams its checkouts hold, the runs it hosts with where each is
  (4.2), and those whose turns or answers the engine has yet to
  acknowledge. Facts go only after it.
- **The engine never reads a repository.** It works from what its
  connectors read, and what a checkout holds (a repository's checks, its
  `AGENTS.md`) is for the run to find (agent.md, section 2).
- **The engine holds the claim.** It commits a claim in its store before
  assigning a run, and decides what follows it: commit its result, run it
  again on any worker, or hold its task for a decision. The worker never
  retries a run and never decides that work is done.
- **Attempts are fenced.** Every assignment carries an attempt. Once the
  engine has cancelled or reassigned an attempt, whatever it still sends
  is dropped; on the worker, a cancelled run's calls are answered as
  unavailable, and an attempt assigned again while the worker hosts it is
  dropped before it becomes another hosted run. A fresh assignment for an
  attempt already hosted is refused as busy. A stale attempt can never
  act over a newer one.
- **Turns and answers are acknowledged.** The worker keeps each turn and
  each answer until the engine acknowledges it, and sends it again after
  every hello. The engine acknowledges only once it has committed it
  (engine.md, 5.2): a copy that comes before then is dropped
  unacknowledged, and one for an attempt the engine has finished with is
  acknowledged again. An answer not yet acknowledged keeps its slot. A
  refusal goes once and keeps nothing: one lost with the channel reads as
  a lost attempt.
- **Losing contact is survivable.** If the channel drops, runs go on for a
  grace period, and what they send the engine waits, never dropped:
  turns, bounced messages, and relays, unless their call has closed
  meanwhile (a tool must not take effect after its run was told it did
  not). On reconnecting, the worker says what it hosts and the engine
  keeps or cancels each run. Past the grace, the worker cancels them
  itself, saving their work first, and keeps their turns and answers for
  the next channel. The engine keeps a lost worker's runs for a grace
  of its own, strictly longer than the stop bound the worker declares at
  its hello: its grace, then the longer of its cancel's grace and a push's
  deadline, then a save's commit and push (engine.md, section 8), so no
  next attempt starts while this one may still push.
- **Shutting down** cancels every run and admits no more. The worker is
  done once the engine has every turn and answer. It gives up what it
  keeps, counted, only once no run is left and the engine is still out of
  reach past the grace; a channel that opens before then gets them after
  the hello.

## 3. Structure

```
temper-worker-domain                 the worker loop's entry point: the engine link, routing
├── temper-worker-domain-host        hosted runs: admit, prepare, start, relay, park or end
├── temper-worker-domain-checkout    workspaces: prepare, merge, commit, push, save; the cache
└── smith-host-domain                agent processes: spawn, channel, watchdog, cancel then kill
```

The tree follows programming-model.md, 4.5: each child domain is a step
machine with its own vocabulary, limits and world; a parent owns its
children's state and routes between them; siblings share no domain
types. The agent child is smith's host domain (smith's `host.md`,
section 4), a child like the others. `host` is the hub: it knows a hosted run's lifecycle and nothing
of git, processes or channels. The other two are capabilities, which the
top level translates to and from through small total functions. A
hand-off goes from a capability to the host, from the host to a
capability, and at most once more back and forth, so each entry point
completes them before it returns, with no ready list.
`temper-worker-domain` faces the protocol layer, and its vocabulary is
what crosses the domain boundary: the engine's messages, the agent
channel, git operations, and the process and file operations of io.

## 4. Hosted runs

### 4.1 The assignment

What the engine gives the worker for one run:

- **Run and attempt.** The engine's names for the run (its task's number
  and the run's) and for this attempt at it; every message about the run
  carries both.
- **Workspace.** A workstream key, which names the checkout so later runs
  of the same work find it cached (the number of the task that holds what
  is written, engine.md, 7.1), and the repositories, each with:
  - its name, the directory it sits in and what the agent calls it, one
    safe path component;
  - its remote, the forge's address, which the protocol layer maps to a
    URL;
  - where to start: a base branch, a branch, a commit by its full object
    id, saved work, or **a merge in progress**: a branch, and the commit
    of its base to merge into it (section 5);
  - whether it may be written, and if so the branch a change is pushed
    to;
  - the identity for every git operation on it (a name the protocol layer
    maps to credentials and a commit author).
- **Saving.** Whether to save unfinished work, and to which branch.
- **Charter.** smith's, as the engine wrote it (agent.md, section 4),
  carried as bytes. The worker gives it to the agent with the workspace
  it prepared: where the repositories sit, which may be written, and for
  a merge in progress the files in conflict (smith's `run.md`, 3.2).
- **Transcript,** optional: the turns of the task's earlier runs, for a
  run that resumes, and the calls committed after the last of them
  (engine.md, 7.2). The worker carries them opaque, within its
  limits.
- **Grants.** The credentials the run's LLM accounts and git identities
  need, as names the protocol layer fills (`credentials.md`, section 7).

Its sizes and names are checked at the entrance against the worker's
limits, and so is a workspace with no repository or with a name twice.

### 4.2 Lifecycle

```
admit ─► prepare ─► start ─► active ⇄ waiting
                               │  turns: kept until acknowledged
                               │  tool calls: delivery, a push, served here; the rest relayed to the engine
                               ▼
                         park or end ─► stop ─► save ─► release ─► answer
cancel, from any state ──────────────► stop ─► save ─► release ─► answer
```

1. **Admit** the assignment, or refuse it at the entrance: busy (every run
   slot is taken, an answer not yet acknowledged keeping its own; the run
   is hosted under another attempt; or the worker is shutting down) or
   invalid (beyond the limits).
2. **Prepare** the workspace (section 5).
3. **Start** an agent process with the charter, and the transcript if
   there is one (section 6).
4. **Relay** while the run is live:
   - **Messages** go down to the run as they arrive, each named, a
     bounded few held until it is live; what does not fit, or comes as it
     ends, is bounced to the engine by name, which keeps it in the task's
     inbox.
   - **Turns** come up as the run ends each, numbered; each is kept until
     acknowledged, and a run with as many unacknowledged turns as its
     limit allows is not read from until one is (section 8).
   - **Tool calls** come up, a bounded number per run and one push at a
     time; one more is answered as busy. Pushes, smith's deliveries, are
     served by the worker (section 5; agent.md, section 6); every other
     call goes to the engine and its answer
     comes back. A call the run withdraws, past its own deadline for it,
     is answered as withdrawn at once if relayed: the host cancels its
     delivery through the engine link and keeps the call until the link
     reports cancellation or the answer that won the race. Cancellation
     does not undo a call the engine already took. The run may ask the
     call again by the same name once its earlier relay has ended. A
     push goes on to its end.
   - A run that **waits** holds its slot until its next message, naming
     the last message it has read.
5. **Park or end,** as the run decides, its last word. A run parks only
   once every turn it took is sent; its transcript is its state, so it
   hands over nothing else, and exits.
6. **Stop:** answer the run's relayed calls as unavailable, and wait until
   the agent process and everything it started are gone, killing them if
   they outstay the grace (section 6), and until a push in flight has
   settled and every cancelled relay has its terminal event: a push cannot
   be abandoned half way, and what it lands counts. Nothing touches the
   workspace while anything of the run is still running.
7. **Save** unfinished work, if the assignment asks: at park, at an
   unfinished end and at cancel. Commit what the writable repositories
   hold and push it to the saved-work branch, so a later run on any worker
   starts from it. A run that ended with a landed change has nothing left
   to save, and nor has one whose agent never started, nor one that
   started from a merge in progress, whose next attempt prepares the
   merge again; one that landed mid-run, then parked, failed or was
   cancelled still saves.
8. **Release** the workspace to the cache.
9. **Answer** the engine, exactly once: ended with the run's result,
   parked, or failed with a typed failure; each with how many turns the
   run took (the engine commits the answer only once it has them all),
   what it spent, what the run left on the forge (the last commit landed
   in each repository, the head a pull request is to show) and how its
   save went. The engine decides what happens next, and its
   acknowledgement frees the slot.

A cancel from the engine, a lost channel or shutdown, or a fault of the
agent (the watchdog's among them), takes the same tail: stop, save,
release, answer. A cancelled run may still say how it finishes as it
winds down, and what it says first, before its agent has gone, is its
answer: a push that lands meanwhile is its result (smith's `run.md`,
8.2), and a
cancel it reports is the worker's. A run past its wall time winds down
the same way, but a cancel it reports is its wall time's failure; a run
stopped for any other fault is answered with the fault. Only a run that
says nothing is answered as the worker stopped it. The host arms no
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
- **The run failed,** as it reports it (smith's `run.md`, section 10: the
  model, budget, policy, stale, a transcript it could not resume), or because an LLM
  account it needs is exhausted.
- **The agent failed:** it could not be started, it exited without
  answering, it broke the channel's rules or said more than the limits
  allow, or the watchdog stopped it (no progress, or wall time).
- **Cancelled:** by the engine, for lost contact, or for shutdown.

Each carries bounded detail for operators, such as the tail of the
agent's error output; none of it is shown to an LLM.

## 5. Checkouts

The checkout child domain owns the workspaces on the worker's disk. Its
vocabulary is typed git and file operations (make a workspace, clone,
fetch, create a branch, check out, merge, commit, push), which the
protocol layer runs as contained processes and whose output it parses.

- **A cache, keyed by workstream.** The runs of one piece of work (a
  change's producing, its repairs, its resolutions) share a key, so they
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
  starting point. A branch or base that does not exist is a preparation
  failure, permanent until something changes on the forge: the worker
  creates no branch but by pushing a run's change, and branches temper
  needs before then are made by the engine, as effects (forge.md,
  section 11).
- **A merge in progress** fetches the branch and the base commit, checks
  out the branch, and merges the base into it as git does, leaving the
  files in conflict with their markers and the merge's state in the
  repository. The agent edits the files and runs the checks; it has no git
  writes (smith's `tools.md`, section 5). Its push commits the tree as the merge,
  with two parents, the branch's head and the base commit, even when the
  tree is unchanged from either; a file the merge left in conflict that
  still holds a conflict marker is refused at push, and the run is told
  which.
- **Push commits what the run checked.** When a run asks to deliver, at
  `finish` with a change or mid-run, it has run the repository's checks
  with nothing else writing to the checkout (smith's `run.md`, 8.1); the
  worker commits exactly that tree and pushes the
  commit, with the run's title and body. A push is a fast-forward and
  never forced: if the branch moved incompatibly with the run's commit,
  the push fails and the run is told. This checks freshness against
  divergent changes; it does not detect every branch movement
  (section 13). When the assignment supplies an expected head, the remote
  must also still be at that exact head.
- **Each repository lands on its own.** Pushing several is not atomic:
  the run is told done only if every repository with a change landed it,
  moved if a branch cannot take the commit as a fast-forward, and nothing
  to push if none had a change. A push the forge refused (permissions, a
  protected branch, a hook) is a landing of its own, which a retry will
  not change.
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

The agent child domain is smith's host domain (smith's `host.md`,
section 4): it runs each hosted run in an agent process of its own,
smith's agent. What follows is what temper relies on of it, and the
choices temper makes.

- **One process per run.** Its process tree is the run's containment
  boundary: stopping a run means that tree is gone, which io proves. Its
  environment holds no credentials: the tokens its LLM calls use come as
  grants on its channel (`credentials.md`, section 7). It is spawned
  within a deadline, and a run whose agent could not be started fails as
  such. It has gone only once its process has exited, its tree is empty
  and its channel has been read to the end, so what it said before it went
  is heard.
- **One channel** over the process's pipes, smith's (smith's `host.md`,
  sections 2 and 3). Down:
  the charter and transcript first, then messages, answers to tool calls,
  grants and at most one cancel, in order. Up: tool calls, named by the
  run and each answered once, also once the run has withdrawn it; the
  withdrawals; turns, numbered; facts; a long operation, its span bounded
  by the limits, and its end; waiting, with the last message the run has
  read; notices that a credential was rejected or an account exhausted,
  which go on to the engine (`credentials.md`, section 7); and how the
  run finishes, its last word. A call name
  reused while in flight, a turn out of order, anything after the last
  word, a payload beyond the limits or a message that does not decode
  breaks the channel's rules: an agent failure.
- **A watchdog on progress.** Every message from the run counts as
  progress; silence past the no-progress deadline stops the run. The
  clock pauses while the run waits for a message, having read every one
  sent to it, while a tool call waits for its answer, so a relayed call is
  bounded by the run's own deadline for it, past which it withdraws the
  call, and while a turn waits for room to be sent. It stretches to the
  span of a long operation the run reports, such as the repository's
  checks, until the run says it is done, so neither a person who has gone
  to lunch nor a two-hour test suite looks like a hang. A separate bound
  covers the run's wall time.
- **Cancel, then kill.** A cancel goes down the channel first, and the
  run winds down on its own (smith's `run.md`, section 10); past a grace period the
  process tree is terminated, then killed. Wall time ends the same polite
  way, as the run is alive and may still report; a broken rule, no
  progress, or a channel hung up while the run is live terminate the tree
  at once. A run that has said how it finishes has the same grace to exit.
  Only an exited process and an empty tree release the slot.

## 7. Facts

What the run reports as it goes (smith's `run.md`, section 11) feeds the watchdog
and goes up to the engine, with the run's and the attempt's names, for
liveness, operators and live views, such as a chat's streaming text.
Forwarding is best effort: a bounded queue, with what does not fit
dropped and counted, sent on an open channel only, after the hello.
Nothing the worker or the engine decides depends on a fact arriving, and
no fact holds a slot. Turns are not facts: they are kept until
acknowledged (section 8).

## 8. Turns

- **What a turn is:** the bytes of one turn of the run's conversation, in
  the agent's own vocabulary (smith's `session.md`, section 3), numbered within its
  attempt, with what it spent and the last of the messages it read. The
  worker reads only its number, its size and that name.
- **Kept until acknowledged,** like an answer: sent at once, sent again
  after every hello, and after a backoff when the engine answers it busy
  (engine.md, 5.1), dropped when the engine acknowledges it. A run may
  have a bounded number of turns, and bytes, unacknowledged; past it, the
  worker stops reading the run's channel, and the run waits until there
  is room, its watchdog paused.
- **Before the answer.** The answer says how many turns the run took, and
  is sent after them. A turn the engine has already committed is
  acknowledged again and dropped there.
- **Lost with the worker.** A turn not yet acknowledged when the worker
  dies is lost with the run; the engine's transcript ends at the last turn
  it committed, and the task's next run starts from there.

## 9. Below the domain

What the protocol and io layers owe the domain. The protocol layer is
designed in `docs/design/protocol.md`, with the channels in `channel.md`
and credentials in `credentials.md`; README.md, section 5 lists what this
design changes in them.

- **The engine:** one framed channel, dialled by the worker and
  authenticated, carrying assignments, messages, cancels, tool calls and
  their answers, turns and their acknowledgements, facts, parks and ends,
  and the answers' acknowledgements. The run's facts go after what the
  domain emitted, so none goes before the hello.
- **Agents:** spawning the agent within a deadline in a contained process
  tree (a delegated cgroup), with proof that the tree is empty after it
  stops; smith's channel over its pipes (`smith-channel`, the host's
  half), whose start message the
  protocol layer completes with where the repositories sit and, for a
  merge in progress, which files are in conflict; an environment without
  forge credentials.
- **Git:** each typed operation is one git invocation in a contained
  process, its output parsed into a typed outcome, and a commit named by
  its full object id (a SHA-1 id zero-padded to a SHA-256 id's 32 bytes).
  New: a merge that leaves its conflicts in the working tree and says
  which files; a commit with several parents. One that ran out of time or
  was cancelled ends only once its process tree has exited, so nothing of
  it touches the workspace afterwards. A repository's remote is mapped to
  a URL, and its identity to credentials or an author, per invocation;
  credentials never go in a URL or a file.
- **Files:** workspace directories. Making one empties it first, so what
  the cache evicted, or a worker left behind before a restart, cannot fail
  a prepare.

## 10. The world

The worker's worlds run the domain against neighbours that share none of
its types (testing-strategy.md, section 4):

- **an engine:** the real one, the engine's root domain, which assigns,
  relays messages, cancels, answers relayed calls, and acknowledges turns
  and answers once its fake store has committed them;
- **a network** whose channel drops at random, for less and for more than
  the grace;
- **an agent** that follows a script, smith's scripted agent: progress, tool calls, turns, waits,
  parks and ends, but also hangs, crashes, broken rules and ignored
  cancels;
- **git:** a remote of refs and commits, and local working trees, with
  fast-forward rules, merges that conflict, failing fetches, refused
  pushes, another party moving a branch, and pushes that land and say
  they timed out.

Each child domain has a world of its own, its parent and neighbours
scripted in it. The whole worker's world has all four, the agents'
process trees standing in for io: a system world, where the worker meets
the real engine over a channel the world translates as the protocol
layers would, and both reach the fake forge. Its channel drops, stalls
and carries late copies of frames; it shuts the worker down and starts a
new one cold; the engine restarts. It checks that the engine commits
each attempt's turns and answer once and acknowledges them in time, that
each relayed call reaches the engine once and is answered once, that what
lands is exactly the tree the agent left (a merge with both parents when
it started from one) and saved work exactly the tree at the stop, that no
git operation runs while a run's agent may, but the push it asked for,
and that nothing is live once it settles.

The worker meets the real agent in the agent's top-level world: each
process it spawns is smith's domain with smith's fake LLM provider, its tools
working in the trees the worker prepared, and the engine commits the
turns and the result the run reports. `docs/design/testing.md` places
these worlds among temper's tiers and tracks their fakes; the working
trees' remotes are on the fake forge, in one store with its API, so a
branch the worker pushes is the head the engine reads.

## 11. From today

- **The worker stays** as it is built, its lifecycle, its checkout cache,
  its fenced attempts and acknowledged answers, its supervision.
- **The agent becomes smith:** the agent child domain is smith's host
  domain, the channel to the agent smith's, and the charter smith's,
  carried as bytes (agent.md, section 11).
- **Items become tasks** in its names: a run is named by its task, and its
  workstream key is the holding task's number.
- **Snapshots go.** No agent produces one today, and the agent's protocol
  refuses a start that carries one; the transcript replaces them, carried
  opaque in the assignment and kept up turn by turn.
- **New:**
  - a merge in progress as a place to start, through the four
    starting-point types the engine, the host, the checkout and the
    channel each have, and the conflicted files in the agent's start;
  - a merge operation that leaves conflicts, and a commit with two
    parents that is never skipped as unchanged;
  - turns kept until acknowledged, on the same footing as answers;
  - what a run spent, in its turns and its answer;
  - a run named by its task and its run, where today it is named by its
    repository and item;
  - a change's title and body kept apart into the commit, where today the
    agent joins them and the worker commits the whole as the title.
- **Goes:** creating a missing base branch on the forge from the default
  branch, which today's prepare does for a writable repository.
- **Already true, and now said:** a bounced message names the message,
  and a run's waiting names the last it read; an assignment carries its
  credential grants; the agent's notices of a rejected credential or an
  exhausted account; an LLM account's exhaustion is a failure of its own;
  static git identities (commit authors) are the worker's configuration.
- **Still below the domain, not built:** the engine's protocol to the
  worker is built, the rest is not: the agent channel's framing on the
  worker's side, git invocations and their parsing, contained process
  trees, workspace directories (`docs/development/protocol-implementation.md`).

## 12. Open questions

- **A push's message:** the worker commits the run's title and body; how
  a change's pull request takes them, and whether a squash keeps them.
- **Several workers in one world:** the whole worker's world runs one;
  placement across several is exercised with scripted workers in the
  engine's world.
- **Before the first channel:** the grace starts only when an open
  channel is lost, so a worker that has never reached the engine dials on
  without one; whether its failed dials want a grace of their own.
- **Checks without an LLM:** validating an exact head needs a checkout
  and the repository's checks but no conversation. It could be a run
  whose outcome is its checks' result, for repositories without CI
  (forge.md, section 20).
- **Code graph:** indexing a checkout for codebase-memory-mcp as part of
  preparing it, and keeping that index with the cache.
- **Saving periodically,** so a dying worker loses less than the work
  since the run's last save.
- **A workspace's repositories in parallel:** a checkout runs one
  operation at a time, so its repositories are cloned, fetched and pushed
  one after another.

## 13. Deferred conformance issues

- **Exact-head freshness on push.** The git operation enforces an
  ordinary fast-forward, with no expected previous head. If another party
  rewinds the remote branch to an ancestor, a run may still push
  successfully and restore commits that party removed. Detecting every
  movement since the run started needs an expected-head precondition,
  including a decision about branches that did not exist at prepare.
  With branches held by one task (connectors.md, 3.3), such a rewind is
  drift the engine looks for after the push; where an assignment names no
  expected head, the precondition stays deferred, and pushes reject
  incompatible history and never force a branch.
