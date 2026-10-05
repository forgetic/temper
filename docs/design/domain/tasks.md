# Tasks

Provisional, 2026-10-04. The engine's tasks: what a task is, how tasks
are made, how each moves from asked to ended, what it hears and when it
wakes, and how proposals and escalations travel the tree. It is the
first of the primitives of core.md (section 3) in depth, and the hub of
the engine's domain (engine.md, section 3). Authority, which every
action here is checked against, is authority.md; what an agent's run
is, engine.md, section 7; a connector's procedures, connectors.md,
section 6. What is still open is listed in section 13. Section 14 is the current
implemented boundary; the broader routes described below join only when
a real root caller and consumer are integrated.

## 1. In one page

- **The tasks child domain is the hub.** It keeps every live task: its
  place in the tree, its dependencies, its lifecycle, its inbox, its
  subscriptions to other tasks and to timers, its budget's numbers, and
  the proposals and escalations waiting for a decision. It knows nothing
  of agents' runs, the forge or the web: an executor is a kind and an
  opaque reference, a connector's news a message it is handed.
- **Batches, whole or not at all.** Tasks are made in batches, checked
  whole (no cycles, within the limits, within the creator's authority as
  authority.md answers) and committed at once.
- **One lifecycle for every executor,** as every entity has one
  (programming-model.md, 5.2): a task waits on its dependencies, is
  active while its executor works, closes (its run ended, its delegates
  closed, its effects settled, its resources released), and ends done,
  failed or cancelled. It may be held for a decision at any point before
  it ends.
- **Decisions go up the tree.** What a task may not do alone, and what
  holds it, waits for the nearest task or person above it with the
  authority to decide, and wakes them.
- **News is kept; only some wakes.** Each task has a durable, bounded
  inbox and a wake policy over what arrives. What does not wake a task
  waits for its next activation.
- **The tree is the plan's history.** Every task made, amended,
  cancelled, released or held, and why, is kept in the store as it
  commits, so a plan's revisions and their reasons can be read back.

## 2. Structure

`temper-engine-domain-tasks` is a child domain of the engine's root
(engine.md, section 3), and its hub, as `work` is today. Its state is a
slab of live tasks, a slab of pending proposals, their inboxes and
subscriptions, and the timers it arms. It is a step machine of its own
(programming-model.md, 4.5): its own vocabulary, limits and world.

What it does not know, and how it is told:

- **Executors** are a kind (agent, procedure, person) and a reference
  the root gives it: for an agent, nothing it reads; for a procedure,
  the connector that owns it and an opaque code; for a person, an
  opaque person or role. It asks the root to activate a task, and hears
  when the activation ends.
- **Authority** is a value of its own type, which it carries and keeps
  the numbers of (budgets spent and reserved, tasks made), as it keeps
  every other funder's (each person's pool and each project's spend, per
  period, authority.md, section 7), and never judges: every check goes to the authority child domain, which the root
  translates it for, and authority's answers say what the numbers become,
  which the root gives back to it (authority.md, section 2).
- **Connectors' news** reaches it as messages the root has already
  classified (7.3): it never sees a connector's events.
- **The store** is the root's: this domain's changes are part of the
  commit the root makes for each decision (engine.md, section 5).

## 3. A task

What the store keeps of a task, and the tasks child domain holds while
it is live:

- **Its number,** given by the engine, never reused, and its project.
- **Its spec:** words, bounded, and typed parameters, which name the
  resources it reads and writes (connectors.md, section 3), the ended
  tasks whose results it takes as **inputs**, and what its executor needs
  (a charter for an agent, a procedure's parameters, the question for a
  person).
- **Its result contract:**
  - *a report:* words, bounded;
  - *a verdict:* one of a closed list the spec gives, each with a
    contract of its own: which fields it requires, how many follow-up
    tasks it may propose and of which kinds;
  - *a change:* a branch pushed, or a change landed, as its connector
    defines it;
  - *a choice:* one of the options the spec lists, or an answer in
    words, for a person.

  Every result may also be a failure, with a reason: the executor's
  verdict that the task cannot be done as asked (5.6).
- **Its requester:** a task, a person, or the deployment, for what its
  configuration or a connector starts on a project's behalf (a recurring
  task, the repair of a landing branch's CI). Its **proposer,** when an
  accepted proposal made it requested by the accepter (section 8).
- **Its dependencies:** live tasks it starts after, each done before it
  starts.
- **Its references:** tasks it may message beyond its requester and its
  delegates (7.5).
- **Its executor** (2) and, for an agent, its **charter**: the role's
  instructions, the models, the tools, the outcome it must produce, its
  default wake policy and waiting time, whether a run resumes its
  transcript or starts fresh (engine.md, 7.1).
- **Its authority** (authority.md, section 3), and the numbers kept
  against it: spent by itself, spent by the closed allotments it funded,
  reserved for the live allotments it funds, and who funded its own current
  allotment (authority.md, 7). The allotment has a durable generation;
  historical spend and each run's cumulative committed expense survive
  closing and replacing an allotment during a move.
- **Its wake policy** (7.3) and **its subscriptions** (7.4).
- **Whether it is tracked,** and its priority if so: a goal
  (core.md, 5.2).
- **Its write holds:** the resources it writes and holds alone
  (connectors.md, 3.3).
- **Its phase** (section 5) and why, if held; its **attempt number,**
  which only grows, fencing stale runs; and its **tries** per class of
  failure, which a release resets.
- **Its result,** once it has one.

What the store keeps beside it, and the engine loads only when wanted:
its transcript (engine.md, 7.2), its history (every change made to it,
by whom, and why), and the results of its ended delegates. While a task
that ended is named by a live one (a dependency's result not yet read,
an input, a reference), the engine keeps a stub of it: its number, its
phase, and where its result is.

## 4. Batches

A batch is what one decision makes: one task or many, with the
dependencies among them.

- **Made by** an agent's `delegate` tool, a procedure's step, a person's
  request in the web, an accepted proposal, or a recurring task's timer.
  Its tasks' requester is the task or the person that made it, or the
  accepter of the proposal that asked for them to be its (section 8).
- **Dependencies** name tasks in the same batch, or live tasks the
  creator references: its own delegates, or a task it was introduced to
  (7.5). A task's dependencies are never added to once it is made
  (section 6). Admission checks the combined graph of existing dependencies
  and delegation waits with the new edges: an introduced reference can
  otherwise make a new child depend on an ancestor, or join two existing
  subtrees into a wait cycle. References alone are visibility, not wait
  edges. A result of a task that has ended reaches a new task as one of its
  inputs (section 3), not as a dependency.
- **Checked whole,** before anything of it exists:
  - every task well formed: a known executor kind, a charter or
    procedure the deployment has, a result contract its executor can
    produce, its spec within its limits;
  - no cycle;
  - within the limits: live tasks in the deployment, tasks in the tree,
    depth below its root, delegates of the creator (section 10);
  - within authority: each task's authority fits what the creator has
    left, its budgets carved from what it has left (authority.md, 8.1).

  A batch that fails a check other than authority is refused, saying
  which task and why, and nothing of it is made. One that fails only on
  authority is refused naming what it lacked, and its creator may
  propose it instead (section 8).
- **Made at once.** Its tasks, their dependencies and the creator's
  carved budgets commit together; its tasks with no dependency left to
  wait for become active in the same commit.
- **Resources named for it.** A creator names the resources a task will
  write: its own branches, which the batch resolves to names once the
  task has its number (authority.md, section 4), or resources the
  creator holds and hands down: the write hold moves with the commit
  (connectors.md, 3.3). A chat that pushed a branch hands it to the
  change task that lands it.

## 5. Lifecycle

### 5.1 Phases

```
waiting ──► active ──► closing ──► done, failed or cancelled
   │          │           │
   └──────────┴───────────┴──► held ── released ──► back where it was
```

- **Waiting:** some dependency is not done.
- **Active:** its executor is engaged (5.2 to 5.4). It enters active in
  the commit that makes it ready.
- **Closing:** its executor has finished, failed, or the task was
  cancelled, and it is settling before it ends. In order:
  1. its run, if one is live, has ended: cancelled, and its answer
     heard (a finished run has ended already);
  2. its delegates have closed and ended, deepest first (a finished
     task's were cancelled as it finished, or had ended, 5.6);
  3. every effect it asked for that went out has settled, made or
     failed, and those not sent are withdrawn if it was cancelled
     (connectors.md, 4.2);
  4. its connectors release the resources it held, as effects of their
     own, after all its others (connectors.md, 3.3).

  Then it ends, and its result, or why it failed or was cancelled, goes
  to its requester. So a task that depends on another never starts
  before what that task did is visible and settled.
- **Done, failed, cancelled:** ended. Its result is a message to its
  requester, and kept. An ended task leaves the engine's working set once
  that message is committed, leaving a stub while live tasks name it
  (section 3); the store keeps it all.
- **Held:** stopped for a decision, from waiting, active or closing. It
  says why (5.5). A release returns it where it was, the reason it was
  held judged afresh. A live run stopped by the hold must first return its
  terminal answer; release cannot overlap it with another run. A terminal
  or a settlement received while held updates the preserved prior state,
  including a closing result, without lifting the hold.

### 5.2 Agent tasks

An active agent task is one of:

- **idle:** nothing to do until a wake (7.3);
- **due:** a run is wanted: the first activation, or a wake;
- **preparing:** its brief is gathering and, if it resumes, its
  transcript loading (engine.md, 7.1). An amendment or a cancel that
  comes meanwhile drops what was gathered and decides again; a required
  section that cannot be gathered sends it back to due after a backoff,
  as no attempt was made;
- **claimed:** an attempt is committed, naming the run and its attempt
  number, and the writer slots of the write holds its workspace writes
  (connectors.md, 3.3), before the run is assigned;
- **running:** a run is live on a worker. It hears the messages its
  policy lets through as they arrive (7.2);
- **backing off:** an attempt failed and the next is due after a backoff
  (5.5).

A run ends by **finishing**, with a result the engine checks against
the contract (5.6), or by **parking**, its transcript kept, the task
idle until its next wake. A task has one run at most at a time.
Whether a waiting run holds its worker's slot or parks is engine.md,
7.4. A claim that no worker reports after a restart, its attempt having
committed no turn and no call, counts as not started: no try is spent
on it. The root issues every attempt number. Each task accepts only a
strictly greater number for a new claim, and refuses invalid or stale
external claims without changing its current run.

### 5.3 Procedure tasks

An active procedure task is stepped by the part of the engine that owns
its procedure: a connector (connectors.md, section 6), or this child
domain for the core's (section 9). The hub hears a step's decisions
(effects asked, tasks made, amended or cancelled, proposals, messages
up, the result) as one decision, committed with the procedure's state.
It steps when it is first active, when a message its policy lets
through arrives, and whenever what it reads changes, which its owner
knows (level-triggered).

### 5.4 Person tasks

An active person task is in the inbox of the person, or of the people
holding the role, its executor names (people.md, section 8). Its result
is the answer one of them gives, checked against its contract. A person
task waiting past its stall (section 10) is news for its requester,
which may amend, cancel or re-address it.

### 5.5 Failures and holds

- **Tries, within a task.** A run or a step that fails is tried again
  after a jittered backoff, within a number of tries per class of
  failure:
  - *transient:* a peer unavailable, the engine's own reads missing, a
    transcript the agent could not use (engine.md, 7.2);
  - *permanent until something on a system changes:* a repository, a
    branch or a commit missing, an identity refused, an assignment its
    worker found invalid;
  - *the run's own:* it reported failing;
  - *the agent's:* it crashed, broke its channel's rules, stalled;
  - *lost:* its worker went away;
  - *invalid:* a result that broke its contract.

  A refusal before anything ran (a worker busy, a writer slot taken) is
  no failure: the task claims again after a pause. Every attempt, a
  failed one included, takes a new attempt number.
- **Past its tries, a task is held.** So it is when its executor stalls
  past its bound, a dependency fails or is cancelled, drift breaks a
  resource it relies on (connectors.md, section 10), an effect it asked
  for fails for good, its budget or its deadline runs out (authority.md,
  section 7), a person stops it, or its procedure says it needs a
  decision. A hold keeps whatever the task had: its result while
  closing, its inbox, its attempt number.
- **A hold is escalated** (section 8): it waits for the nearest task or
  person above it that may decide, which it wakes. Whoever it reaches may
  release it, amend it and release it, or cancel it (section 6).
- **A release lifts what it releases:** tries count again from it, and
  what held the task is judged afresh.

### 5.6 Results

- **Checked twice.** An agent's run checks its result against the
  contract before it finishes (smith's `run.md`, section 7); the engine checks it
  again as it commits, against the contract and the state as it is
  now. A result that breaks its contract is a failed attempt, its reason
  given to the next run.
- **What a result may carry** beyond its words: effects, through the
  tools that ask for them; follow-up tasks its verdict's contract allows,
  as one batch. Follow-ups beyond the task's authority become one
  proposal, made as the result commits; the result stands.
- **A task ends after its delegates.** An executor that finishes while
  delegates of its task are live is refused, naming them, unless its
  finish asks for them to be cancelled, so a task that depends on
  another waits for that task's whole subtree. Its result goes to its
  requester once it has closed (5.1). The finish is a reply-bearing call
  before the run exits: refusal leaves that attempt live so the executor
  can correct the finish. An accepted activation terminal is acknowledged
  after the decision is durable; replaying its accepted answer changes
  neither the task nor its tries.
- **Delivered after commitment.** The child emits `Ended` with its ending
  after closing; the root commits the ended row and financial posting before
  releasing a live notice to a person. Reconnecting people read the named
  ended task through the root, authenticated by people (people.md, 6). There
  is no task-owned transport receipt or persistent result credit. A future
  requester inbox route must integrate its own real root handoff. A cancellation can retain a completed result alongside its reason,
  recording what was done before everything settled.
- **Kept.** A result is read by every task that depends on its task, or
  names it among its inputs, in the brief of its next run (engine.md,
  section 9), and by its requester as a message.
- **Failure is a result.** An executor that finds the task cannot be
  done as asked ends it failed, saying why. A failed task's dependents
  are held (5.5); its requester decides what follows.

## 6. Amending, cancelling, moving

- **Who may:** a task's requester, and any task above it in the tree;
  a person whose role in the project allows it; and whoever an
  escalation of it reached (section 8). Nobody else.
- **What an amendment may change:** the spec and its parameters; the
  wake policy; the dependencies of a task still waiting, by removing
  some, never adding; the authority, narrowed freely and widened only
  within the amender's own (beyond it, a proposal); the charter's
  instructions; a procedure's parameters, such as the gates a change asks
  for (forge.md, 8.3). An amendment is refused once a task is closing.
- **An amendment is a message** to the task's executor, committed with
  the change: it wakes an agent, steps a procedure, and shows a person
  what changed. A run already live hears it as it arrives; a narrowing it
  cannot honour (a write grant its live run is using) cancels that run,
  and the next starts with the narrower authority.
- **Cancelling closes down the tree.** A cancelled task, and every task
  below it, is closing (5.1), each with the same reason: live runs are
  cancelled, pending proposals withdrawn, outbox entries not yet sent
  withdrawn, and those in flight settle, since what may have landed
  counts. Each task ends once its own closing is done, deepest first, and
  its requester hears that it ended cancelled, with what it had done.
- **Moving a task.** A person may make themselves the requester of a
  live task they may amend, so that it outlives the task that asked for
  it, as a goal outlives the chat it came from. Its reservation moves to
  their funding (authority.md, section 7); the task that asked for it
  keeps a reference to it. The move and every funding replacement commit
  together: live tasks stay live, with every promised unspent budget
  preserved, or the move is refused. The hub follows actual funder links
  across the whole moved requester subtree, including delegates an
  accepted proposal funded directly from the old chat. Such allocations
  are separately re-funded if their old task funder must end; stable
  external pool or period funding keeps its original identity. Closed
  allotments are recorded exactly once by generation, and replacements
  reopen current counters without erasing history or a live run's
  cumulative charged expense (authority.md, section 7).

## 7. Messages

### 7.1 What a task hears

| Message | From | Woken by default |
|---|---|---|
| a result: done, failed or cancelled | a delegate | the last of its delegates; any failure |
| a question, expecting an answer | a delegate | yes |
| an answer | its requester | yes |
| a decision on its own proposal | the holder that decided | yes |
| an amendment | its requester, a task above it, or a person | yes |
| a cancel | the same | always |
| words | a person; a task holding a reference to it | a person's, yes; a task's, by policy |
| news: a connector's event, classified | its subscriptions (7.4) | by policy |
| a notice: a task it references done or held; a proposal passing through | its subscriptions; the engine | by policy |
| a timer | itself | yes |

Proposals and escalations waiting for its decision are not messages: they
are entries of its own (section 8), which always wake it.

### 7.2 The inbox

- **Durable.** A message is in the inbox once the commit that sent it
  is; it is taken once the commit of the run's turn, the procedure's
  step or the person's view that read it is. A run that dies without
  committing what it read leaves it for the next.
- **Bounded, with every message admitted at one entrance.** An inbox
  holds at most a configured number of messages and bytes, and every
  kind of message is admitted in one of three ways:
  - *room kept:* one result from each live delegate, one answer to each
    open question it asked, one decision on each of its pending
    proposals, and a cancel, so none of them can be refused;
  - *merged:* news of one subscription with the news of it already
    waiting, since news is a hint to read afresh; notices likewise; a
    new amendment with one still waiting; timers;
  - *refused at the sender's entrance:* words and questions, past the
    bound: a person is told the inbox is full, an agent's tool call
    answers so.
- **Named by the root.** Message, question and subscription identities and
  transport call/replay evidence belong to the root (engine.md, 5.4 and 7.5).
  A future inbox route must show its real sender and terminal consumer before
  adding that machinery. The current boundary has no inbox or offered-payload
  table; `Turn` refuses a nonempty read fence.
- **Taken by committed turns.** Tasks admits only the next turn of the
  current attempt. Root/fleet handles accepted duplicate transport inputs
  before they can reach child admission; the root owns its durable proof.
  Future inbox read-through remains a semantic admission in the same priced
  turn, when its actual root route is integrated.
- **Merged with a fresh ID.** A replacement hint keeps the oldest arrival
  time and accumulated occurrence count, but receives a fresh root number.
  Reached batch thresholds are durable and survive holds, wall corrections
  and restoration.
  An old offered payload stays immutable; committing its read cannot take
  the replacement. Subscription capacity reserves its largest replacement
  even when the current hint is smaller. An ended task's unread messages
  become archived rows in the same decision as its end.
- **Relayed live.** A task with a running run has what its policy lets
  through relayed to it as it arrives (engine.md, 7.4); what it does not
  take before its run ends stays.
- **Carried whole and read oldest first,** within the brief's budget: a
  person's words are never cut, and what does not fit is said to wait.

### 7.3 Wake policies

A wake policy is data, from a closed vocabulary:

- **for each kind of message** (7.1): whether it wakes the task, or waits
  for the next activation;
- **for news,** the class the connector gives each event for the task:
  *wakes*, *kept* (news, but nothing the task must act on now), or
  *dropped* (not the task's concern after all, and not kept). The task's
  policy may lower a class, never raise it;
- **batching,** for kinds that allow it: wake once N have arrived, or
  the oldest is older than T, so a coordinator watching twenty changes
  wakes once for a burst. A person's words, a cancel and an amendment
  are never batched, and never wait behind what a batch holds;
- **timers:** wake at a time, or every period.

A proposal or an escalation waiting for its decision always wakes a
task, as a cancel does: no policy may keep a decision from whoever must
make it.

Each charter brings a default policy, which the task's creator may
narrow or widen: a chat wakes on its person's words and on its
delegates' results; a coordinator on questions, failures, the last of
its delegates done, a person's words and landings that overlap its
work; most other agents on nothing, since they finish within one run. A
procedure's policy is its owner's: it steps on whatever it reads.

### 7.4 Subscriptions

A subscription is a task's standing interest in what changes:

- **a connector's topic,** such as the landings on a repository's
  branch or CI on a head: kept by the connector, which names the task by
  its number and classifies each event for it (connectors.md, 5.3);
- **another task's state:** done, held, or its result; for a task it
  references;
- **a timer.**

The child asks the root to deliver a task notice or timer with a fresh
message number. The root completes these callbacks, and each delegate-result
callback, before committing or returning the decision. Startup restoration
rejects an unfinished callback record; `Restore`/`Restored` are only the
startup barrier. A subscription to an ended referenced task reads its
historical result through the root in that same decision.

Subscriptions end with the task. A tracked task's subscription to the
landings in its subtree's repositories is the engine's to keep, not its
executor's (forge.md, section 7). Their number per task is bounded.

### 7.5 Who may message whom

- **Along delegation:** a requester and its delegates, both ways.
- **By reference:** a task may message any task it holds a reference to:
  its requester, its delegates, and one it was introduced to. A task
  introduces two tasks it references (a coordinator two of its
  delegates) by giving each a reference to the other, within their
  limits. A requester reserves an explicit reference for each admitted
  delegate,
  retaining its visibility after the delegate ends until it forgets the
  reference. References are kept with the task, and end with it.
- **People** message the tasks their role lets them see (people.md,
  section 5).
- **No broadcast.** Who may talk to whom stays visible and bounded.

## 8. Proposals and escalations

A proposal is an action a task may not take alone, waiting for a holder
of the authority it needs; an escalation is a held task waiting for a
decision. authority.md, section 9 says what may be proposed, what an
action needs, and what an acceptance funds.

- **Their state.** A proposal: the proposing task; the action, as the
  tool or the step asked for it (a batch, an effect, an amendment, a
  widening of its own authority); its reason, in words; whether the
  tasks it makes are to be the accepter's; where it waits; and pending,
  accepted, rejected or withdrawn. An escalation is part of its held
  task's state: one per held task, never more. Both are kept in the
  store, and pending ones in this child domain.
- **Admitted once.** A proposal is admitted at its entrance, the
  `propose` call or the step that asks, against the pending proposals
  allowed per task and in the deployment, and refused there if they are
  full; an escalation needs no room beyond its held task. Neither takes
  room in any inbox: each shows as an entry in the inbox of whoever it
  waits for, and those it passes on the way hear a notice, which merges.
- **Where they wait.** At the nearest task above the proposer, or the
  held task, whose authority covers the decision (authority.md,
  section 9), skipping procedures, which decide nothing. Above the
  tree's root task is whoever requested it: a person is the next holder
  there, when their authority covers it; past them, or past a root the
  deployment requested, it waits for the people whose role the project's
  policy names for that kind of decision (people.md, section 4). Where one waits is decided again when that holder ends, is
  held, or no longer covers it, and when it stalls (below).
- **Waking.** It always wakes the task it waits for (7.3), is relayed to
  that task's live run as messages are, and is in the brief of its next
  run (engine.md, section 9); for a person, it appears in their inbox at
  once.
- **Deciding.** A holder accepts, rejects with a reason, or passes it up
  to the next holder that covers it. An accepted proposal is made as the
  proposer's action, with the accepter's authority funding what it
  needs, in the commit that accepts it; the tasks it makes are the
  proposer's delegates, or, when the proposal asked, the accepter's (a
  chat proposing a goal asks so: the goal is its person's, and outlives
  the chat). A rejection is a message to the proposer. A person who does
  not cover a proposal may reject it or pass it up, never accept it. At
  the people the policy names there is no one further: they accept or
  reject, and the proposal waits for them, however long.
- **Stalled** past its bound, undecided, a proposal or an escalation
  passes up to the next holder that covers it, and past the last, waits
  for the people the policy names.
- **Withdrawn** when the proposer withdraws it, closes, or when the state
  it was made against has changed so that it no longer applies (a batch
  naming a task since ended); an escalation ends with its hold.

## 9. Core procedures

Procedures the core owns, stepped by this child domain:

- **Recurring.** A task whose procedure is a timer and a template: at
  each period it makes a batch from its template, as its own delegates,
  within its authority, its budget carved again each period from the
  project's spend for that period (authority.md, section 7). If the last
  batch is still live, the period passes, or the new batch waits, as
  the template says. A recurring task ends when it is cancelled.

The core has no other procedure. A static plan is a batch; waiting for
several tasks is a dependency; anything that needs a condition over
results is a coordinator's judgement (core.md, section 11).

## 10. Limits

Each is configured, refused at the entrance, and part of the engine's
worst case (engine.md, section 13):

- live tasks in the deployment, and per project, and the stubs of ended
  ones;
- tasks per tree, and the depth of a tree;
- delegates per task, and batch size;
- a spec's and a result's bytes, and inputs per task;
- messages and bytes per inbox;
- subscriptions, references and write holds per task;
- pending proposals per task, and in the deployment; open questions per
  task;
- tries per class of failure, and their backoff;
- the stall of a person task, of a pending proposal or escalation, and
  of an agent task idle with delegates still live.

A live task is never evicted: when the deployment is full, new batches
are refused, and the creator hears why.

## 11. The world

The tasks child domain has a world of its own,
`tests/engine/tasks`: its parent, the executors, the connectors' news
and the authority checks scripted, its expectations in a referee
(`docs/design/testing.md`, 5.2). Its stories: a batch made whole or
refused whole; a plan of spikes, a choice and changes, run in
dependency order; a delegate failing past its tries, held, escalated
two levels to a person, released and finished; a coordinator woken once
by a burst of results; a cancel closing a tree of three levels with runs
live and effects in flight; an amendment reaching a live run; a proposal
routed past a procedure to a person and accepted, and one that stalls and
passes up; a chat's goal accepted as its person's, outliving the chat; an
inbox filling, its news merged, words refused, a result still taken.

Its referee: a task starts only after its dependencies are done;
batches are made whole or not at all; one run at most per task; every
result reaches its requester once, after the task has closed; a cancel
ends every task below it, deepest first; no committed message is lost,
and none is refused once committed; every proposal and escalation waits
where a holder can decide it; nothing beyond a task's limits is held.

## 12. From today

- **`work`, the hub,** becomes this child domain. Its lifecycle grows
  from an item's (waiting, parked, retrying, claimed, running, applying,
  held, done) into a task's, with agent tasks keeping most of it (5.2):
  parked is idle, retrying is backing off, applying is part of closing.
  Its six failure classes, backoffs, holds that keep their outcome,
  attempts that only grow, and acknowledging an answer once durable
  stay. Records, mangled records, the recording of outcomes as comments,
  and the hold codes the top level gives go: a hold's reason is typed
  here, and the change's reasons (repairs, rebases, its pull request
  closed, stalls) are the change procedure's (forge.md, 8.4).
- **`plan`** splits. Its dependencies, its checks of a plan (no cycles,
  a bounded size) and its wakes and their batching come here; growth
  within an envelope becomes delegation within authority, and its
  acceptance a proposal (authority.md). Its change's way to landing goes
  to the forge's change procedure (forge.md, section 8).
- **Sessions** are no longer items: a chat is an agent task (core.md,
  section 8), and a supervising session a coordinator's.
- **The inbox,** derived today from what changed on the forge after a
  record's inbox position (comments, and a pull request's head, CI,
  state and verdicts) and from notices of relations (one finished, held,
  or decided), becomes a durable inbox in the store. A change's facts go
  to its procedure, which reads them afresh; the comment inbox stays only
  for participating objects (forge.md, section 13).
- **Kept:** a step is done only once the steps it added are, which is
  5.6's rule that a task ends after its delegates.
- **New:** batches made by agents and people, closing, amendments and
  cancels down the tree, moving a task, references, subscriptions to
  tasks and timers, proposals and escalations routed to whoever covers
  them, recurring tasks.

## 13. Open questions

- **Wake defaults:** which kinds wake each charter's tasks, and how
  batching is tuned; whether a cheap model should triage news before a
  coordinator wakes. Tuned from use.
- **Dependencies on a failure:** whether a task should be able to depend
  on another's ending, whatever its result, for clean-ups; until wanted,
  a requester makes the clean-up task itself.
- **Adding dependencies by amendment,** which the model forbids so that
  the cycle check stays local to a batch.
- **Priorities among tasks that are not goals,** when workers are
  scarce: today's order is by when each became due.
- **Long-lived tasks** that serve as an agent's identity across goals
  (core.md, section 11).

### Incremental accounting implementation

The retained slice owns finite project period and person pool ledgers. A
pool records its original project period as its actual parent; opening a
new period does not release old reservations. Whole-batch reservation and
closure posting happen inside the child decision. Root authenticates the
current role/authority and gathers all writes into one atomic commit.

`Turn` combines next-turn admission with the new cumulative expense delta.
`Activation` explicitly distinguishes a priced worker terminal from an
unpriced loss, fleet refusal or invalid-answer normalization. Priced inputs
preflight both semantic admission and representability of eventual actual
funding-chain postings before mutation. Refused inputs spend nothing.
Tasks saves only authentic task/funding state and closure financial history;
root-owned transport proof, transcript and terminal evidence join those
writes in the same decision (engine.md, 7.5). There is no standalone Charge
operation or second pool ledger.

Period/pool retirement, funding replacement/move and recurring procedures
remain parked until their real root routes are integrated. The finite source
table refuses fresh identities when full; unsupported closed sources and
external ledgers with direct own spend are refused at restoration.

## 14. Current concrete boundary after the tasks audit

This inventory records actual constructors and consumers in
`crates/temper-engine-domain/src/engine.rs`, rather than proposed future
routes. The pre-audit source remains on
`preserve/pre-tasks-boundary-audit-429647c` and the original implementation
branches. Deleting dormant surface does not assert those deeper designs are
complete. The first-turn durable/lost-completion restart is covered; broader
terminal-before-ACK/result restart recovery remains later root work.

### 14.1 Events: thirteen actual root constructors

| Event | Decision | Concrete caller |
| --- | --- | --- |
| OpenPeriod | Keep | `chat`, after current project authority validates its ceiling |
| CarvePool | Keep | `chat`, after current person role validates its ceiling |
| Make | Keep | `chat`, for authenticated people `StartChat` |
| Prepare | Keep | `activate`, after real authority/account readiness |
| Claim | Keep | `brief_outputs` Rendered, after brief and grant; root reserves proof room first |
| Turn | Merge charged admission; remove unpriced Turn | `fleet_outputs` Turned, correlated to the root's owned worker payload |
| Started | Keep | `fleet_outputs` Placed |
| Activation | Merge charged and unpriced causes | `fleet_outputs` Answered uses Priced; Lost/Withdrawn/Refused use Unpriced; `tasks_outputs` rejected answer normalizes Invalid unpriced |
| PreparationFailed | Keep | `brief_outputs` Failed/Refused or exhausted run/grant/proof room; `tasks_outputs` rejected Claim |
| Hold | Keep | `activate`, for authority deadline, budget or other static findings |
| Settled | Keep | `tasks_outputs` Close, after this slice's synchronous closing effects |
| Restore | Keep | `startup_page`, actual paged Live/Ledger records |
| Restored | Keep | `startup_page`, after all current root proof pages validate |

Uncalled events are removed: ForgetAdmission, Control, Amend, Move, Charge,
Send, Peek, DeliverResult, DeliverNotice, DeliverTimer, News, ForgetReceipt,
Introduce, ForgetReference, Subscribe, Unsubscribe, Release, Cancel,
RememberStub and ForgetStub. ChargedTurn merges into Turn; ChargedActivation
merges into Activation. Standalone Cancel is uncalled; legitimate
`Finished { cancel_delegates: true }` still closes a delegate tree.

### 14.2 Requests: thirteen actual root consumers

| Request | Decision | Concrete consumer |
| --- | --- | --- |
| Made | Keep | `tasks_outputs`, correlates pending people request and supplies Decided Started |
| Refused | Keep | `tasks_outputs`, completes Make refusal or Claim failure, or handles owned Turn/Answer refusal |
| Done | Keep | `tasks_outputs`, correlation releases claimed Fleet Start after commit; funding/Prepare Done has no outward effect |
| Acknowledged | Keep | `tasks_outputs`, saves root terminal evidence and holds Fleet Acknowledge |
| TurnAcknowledged | Keep | `tasks_outputs`, saves transcript and latest root proof, then holds Fleet TurnKept |
| Activate | Keep bounded context | `tasks_outputs` -> `activate`; authority and real task brief consume temporary RunContext |
| Stop | Keep | `tasks_outputs`, holds Fleet Cancel |
| Adopt | Keep kept-turn field | `tasks_outputs`, retains claim adoption before Fleet Loaded; no raw task peek |
| Close | Keep | `tasks_outputs`, routes actual synchronous Settled |
| Ended | Keep | `tasks_outputs`, erases current proof and holds person result notice |
| Save | Keep | `tasks_outputs`, wraps authentic child row into current root Decision |
| Erase | Keep | `tasks_outputs`, wraps child erase into that same Decision |
| RestoreRefused | Keep | `tasks_outputs`, stops startup before continuation or Fleet Loaded |

Sent, Inbox, Relay, Observe, Notify, Timer and Topic have no reachable producer
under actual root events and are removed with their dormant mechanics.

### 14.3 Stored, keys and queries

| Surface | Decision | Actual root use |
| --- | --- | --- |
| Live | Keep | Child Save/Erase; Tasks startup page restores bounded current semantic lifecycle and dependencies |
| Ended | Keep historical | Child Save; root's authenticated TaskResult singleton load derives person result; never live restore |
| Ledger | Keep, merge duplicate Funding | Child Save; Tasks startup page restores actual period/pool numbers |
| Closure | Keep historical | Child Save records actual ended allotment generation/financial posting; never live restore |
| Admission | Move to root RunProof/Terminal | Actual current claim, next turn and worker terminal; root pages current proofs before Fleet Loaded |
| Stub, Message, ArchivedMessage, Receipt, Offer, Question, Subscription, History | Remove | No actual root constructor; dormant restore pass-through was not a live route |
| `funding(Funder)` | Keep narrow authentic query | `chat` reads whether its actual finite source already exists; root owns no mutable copy |
| `live_task`, `task_stub` | Remove | Authority/brief use Activate's bounded RunContext, adoption uses kept, refusal uses owned payload task/attempt correlation |
| `ready`, `next_deadline`, `is_due`, `facts_lost` | Private/remove | No actual root caller; restoration terminal and root-owned timers drive the boundary |
| `fire`, `reclaim`, `pop_fact`, `max_out`, `worst_case`, `stored_bytes` | Keep | Root timer pass, iteration reclamation, neutral observation drain, bounded route/byte admission |

Dependencies remain immutable history. `waiting_on` is their bounded
unfinished subset; ending propagates removal directly to live dependents,
without historical stub storage. Restore first checks length/unique/subset
without cloning, then requires every still-live dependency in that subset
once all pages arrived. Failed dependencies hold ordinary waiting work;
already-closing cancellation continues closing. Current batch dependencies
name same-batch tasks or the creator's live delegates; nonempty Spec.inputs
are explicitly refused until an actual root input-result route joins.

Live restore rejects unheld Closing Settled: the same child step always
ends that transient phase before a root commit. Held closing state may
remain inert after its actual Settled callback. Financial restore verifies
current reservations, actual requester-ancestor task funding, original
external source links and representability of eventual postings before
Ready. Historical Ended/Closure rows never enter the live arena.
