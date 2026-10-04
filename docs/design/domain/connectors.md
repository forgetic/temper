# Connectors

Provisional, 2026-10-04. What every connector is and does: the contract
an external system meets to be driven by temper. It is the fifth
primitive of core.md (3.5) in depth. The forge is the first connector,
and forge.md designs it against this contract; section 14 sketches a
second, test environments, to check that the contract is not the
forge's in disguise. What is still open is listed in section 16.

## 1. In one page

- **A connector is a subtree of the engine's domain,** under the root,
  in a closed set the root matches on. It knows its own system and
  nothing of the others; the tasks, their inboxes and authority know
  none of them (engine.md, section 3).
- **Resources** are what has an identity on the system. A connector
  names them as paths, which authority's grants cover by prefix. A
  resource a task writes is held by that task alone.
- **Effects** are what temper does on the system, committed into the
  outbox with the decision that asks for them, and made by the
  connector after the commit, keyed so that a write repeated after an
  uncertain failure finds what the first made.
- **Facts are read afresh; events are hints.** A connector keeps a
  working set of what its system owns of the live work, and reads again
  before a decision depends on it. What changed is published on topics
  to the tasks that subscribed, classified for each: wakes, kept, or
  dropped.
- **Procedures are the mechanics,** as executors of tasks: pure step
  machines, their state committed with each step, deciding from the
  facts as they are rather than from the event that woke them.
- **Tools and brief sections** let agents read the system, within the
  connector's request budget.
- **What temper does there belongs to temper.** A connector records
  what temper made by key, writes only to what temper owns (or as a
  participant), and treats any other change to what temper relies on as
  drift: read afresh, a task held, never written over.
- **Adopted as it is.** A system is plugged in with whatever access
  temper is given; nothing on it is required, and making things there is
  an optional convenience.

## 2. Where a connector sits

```
temper-engine-domain                 the root: matches on every connector
└── temper-engine-domain-<system>    a connector's top: its resources, write holds, topics, procedures' routing
    ├── …                            its own children, as its system needs (the forge's: forge.md, section 3)
```

- **A connector's top is a child of the root,** and may have children
  of its own (programming-model.md, 4.5).
- **What crosses between the root and a connector** is the connector's
  vocabulary, which the root translates to and from the core's:

  | Down, root to connector | Up, connector to root |
  |---|---|
  | a live task names these resources; it no longer does | a resource's role, and what temper may do on it |
  | hold, hand down, release a resource; take, free its writer slot | a write hold refused: held by another task; a writer slot taken |
  | make this outbox entry | made, with what it made; failed; uncertain |
  | step this procedure task: activated, a message, closing | a step's decision: effects, batches, amendments, proposals, messages, a hold, a result |
  | project this goal: its state, its plan, its milestones | the projection's effects (section 7) |
  | release what this closing task held | the release's effects |
  | subscribe this task to this topic; unsubscribe | news for these subscribers, classified |
  | serve this read for a run | its answer |
  | gather this section for a brief | the section, cut |
  | these facts for this effect's requirements | the facts |
  | adopt this resource into this project | adopted, with what temper may do; refused |

- **Tasks are named by number.** The connector keeps the numbers of the
  tasks it serves as plain values; it never sees a task. What it projects
  of a goal, the root gives it, in the connector's own terms.
- **Its protocol is its own.** The root passes its calls down and their
  answers up, and its system's hints (webhooks, for the forge) arrive as
  events no request caused.

## 3. Resources

### 3.1 Names

- **A path of segments,** the connector's choice, stable for the
  resource's life: for the forge, the forge's host, the repository's
  owner and name, then what it is in the repository (forge.md,
  section 2). Names are bytes, compared segment by segment.
- **Prefixes** cover what begins with them, which is how authority
  grants effects on many resources at once (authority.md, section 4).
- **Kinds of effect and read** are the connector's, each with the
  resource kinds it applies to, and the order among them that authority
  holds as data.

### 3.2 Roles

Each resource a project adopts has a role (core.md, 5.1): owned, or
adopted to write and land into; a fork; context. Each object temper
deals with is owned or participating (core.md, 7.2). A connector keeps
both with the project, and every effect is checked against them before
any grant: no grant writes to a context resource, and an effect on a
participating object is a participant's write.

### 3.3 Write holds

- **A resource a task writes is held by that task alone,** such as a
  branch a change is pushed to: its write hold. The hold is taken in the
  commit that makes the task, in the commit that hands it the resource,
  or, for a run's own branch (forge.md, section 11), in the commit that
  claims the run; one held by a live task elsewhere is refused, and the
  batch or the claim with it. A run's own branch it never pushed to is
  released with the run's answer.
- **Delegates write under their holder's hold.** A change task's
  producing, repair and conflict runs write its branch.
- **One writer at a time.** Each write hold has one writer slot. A run
  that writes a held resource takes the slot when it is claimed (tasks.md,
  5.2), whether its task holds the resource or writes under a task above
  it that does, and frees it when its answer is committed, or, when its
  attempt is settled as lost past the engine's grace (engine.md,
  section 8), once the connector has read the resource afresh, so that
  what a lost attempt pushed is known before anyone writes again. A
  claim that finds the slot taken waits, which is no failure. Effects
  the connector makes on the resource (an update from its base) take
  the slot too, while they are in flight, and wait for it like a
  claim.
- **Handed down.** A creator may hand a resource it holds to a task it
  makes: the hold moves (tasks.md, section 4), and the writer slot stays
  with a run that has it until that run's answer, so the new holder's
  first writer waits for it.
- **Released as the task closes.** What becomes of the resource is the
  connector's to say, per kind and per way the task ended, as effects of
  their own, made after every other effect of the task has settled
  (tasks.md, 5.1): for the forge, a cancelled change's pull request is
  closed and its branch deleted; a landed one's branch is deleted; a
  failed one's is kept for a person to read until its tree's root closes
  (forge.md, section 11).
- **Shared branches are not held.** Landing into a repository's main is
  an effect many tasks make, ordered by the connector's own mechanics (a
  landing queue, forge.md, section 9), not by a write hold.

### 3.4 Live resources

The resources live tasks name are the connector's live resources: what
its working set holds (section 5) and what it reads afresh at a
restart. When no live task names a resource, it leaves the working set.

## 4. Effects and the outbox

### 4.1 Effects

An effect is a typed request the connector defines, naming:

- the resources it touches, and its kind, for authority;
- its **key** (4.4);
- its **condition,** where it has one: the state it was decided against
  that the system can check as it applies it (a merge at exactly a
  head);
- what it is: a **creation** (a pull request, an issue, a comment), a
  **transition** (a merge, an update from a base), or a **set** (a body,
  reviewers, a status, open or closed), which decides how it is found
  again (4.3).

### 4.2 The outbox

- **An entry per effect,** committed with the decision that asks for it.
  Its connector holds the entries not yet made, which the store keeps
  too (engine.md, 5.4).
- **Made after the commit,** in commit order per resource and one at a
  time on each, in parallel across resources within the connector's
  limits and request budget.
- **Its outcome is committed in turn:** made, with what it made (a
  number, a commit); failed, typed; or uncertain. A task closing waits
  until every entry it asked for has settled (tasks.md, 5.1).
- **Retries are the connector's:** a transient failure is tried again
  after a backoff, within its budget; a failure for good (forbidden, a
  conflict, the object gone) is news for the task that asked, and holds
  it if it was closing. A projection's or a release's failure for good
  is news for the goal, or the closing task, it was made for.
- **Withdrawn** when the task that asked for an entry is cancelled before
  the entry went out. An entry that went out is never withdrawn: what
  may have landed counts.

### 4.3 Uncertainty

- **Only what provably never reached the system did nothing.** Any other
  failure (a timeout, a reset after sending, an answer that does not
  decode) is uncertain: the write may still land, until a **lifetime**
  after it went out.
- **Found, not repeated.** An uncertain entry is looked for before it is
  tried again:
  - a creation by its key, which the system keeps with the object
    (forge.md, section 6);
  - a transition by the state it leads to: a pull request merged, and at
    which commit;
  - a set is written again, as sets are idempotent.
- **One not found** is tried again once its lifetime has passed. A new
  engine treats every entry it loads as uncertain (engine.md,
  section 6), so the lifetime is counted from when it started.

### 4.4 Keys

- **Committed with the entry,** and derived from what the effect is
  for, never from when it was asked, so asking again for the same thing
  finds the same key:
  - an agent's effect: its task, its attempt and the call that asked
    (engine.md, 7.3);
  - a procedure's: its task and the purpose within it (opening its pull
    request; merging at a head; a review at a head);
  - a projection's: its goal and the milestone or the body it writes;
  - a release's: the closing task and the resource.
- **The deployment's,** too: every key carries the deployment's id
  (engine.md, 5.4), so two deployments on one system, or a store made
  anew, never find each other's objects.
- **Carried on what it creates,** where the system keeps it, so that a
  creation can be found by its key alone. The connector records every
  object temper made, by key, in the store: the ownership of section 10.

### 4.5 Effects asked by agents

An agent asks for an effect with its `effect` tool (engine.md, 7.3): a
comment, an issue for a finding, a status. It is checked against its
task's grants and the resource's role, committed, and made like any
other. What the connector's procedures own (opening, updating and
merging a change's pull request) is not offered to agents: a run that
wants a change landed delegates it to the procedure.

## 5. Facts and events

### 5.1 The working set

- **What the system owns of the live work:** for the forge, pull
  requests' heads, states and mergeability, CI on those heads, landing
  branches' tips (forge.md, section 5). A working set, not a copy: it
  grows with the live resources, never with the system's history.
- **Kept up** by reading what changed, sooner when a hint arrives, with
  polling as the backstop, at a cost that follows the rate of change.
- **Read afresh** before a decision depends on a fact: a procedure's step
  reads what its decision needs, and the facts an effect's requirements
  name are read for the exact state it names.

### 5.2 Events

- **Hints, never state.** A webhook, or a change keeping up found, tells
  the connector that something may have changed; it reads what it needs
  and updates its working set.
- **Echoes dropped.** What temper itself did comes back as events too;
  the connector drops those it can tell are its own (an effect it made,
  by its key; a push, by its writer slot, section 10) and reads the
  rest.
- **Then three things follow:** procedures whose facts changed step;
  write holds whose resources drifted are checked (section 10); news is
  published on the topics it touches.

### 5.3 Topics

- **A topic is a subject a connector names:** for the forge, the
  landings on a repository's branch, CI on a head, a pull request's
  state, the comments on a participating object (forge.md, section 7).
- **Subscribers are tasks,** by number, and their number per topic is
  bounded. A task subscribes through its tool, through its creator, or
  through the engine (a goal's landings, core.md, 5.2).
- **Each event is classified per subscriber:** *wakes*, *kept* or
  *dropped*, by the connector's judgement and without an LLM: a landing
  that overlaps the files of a goal's open changes wakes its
  coordinator; one that does not is kept. The class travels with the
  news into the task's inbox, whose wake policy may lower it, never
  raise it (tasks.md, 7.3).

## 6. Procedures

### 6.1 What a procedure is

A step machine a connector owns, one per task whose executor it is: the
forge's change. Its state is a plain value, kept in the store with its
task and committed with each step, including the effects it has asked
for that are not yet settled and the wall times of what it waits for.
It knows its task's spec and parameters, its delegates by number, and
the facts it reads. A projection (section 7) is not a procedure: no
task asked for it.

### 6.2 When it steps

- when its task becomes active;
- when a message its owner lets through reaches its task: a delegate's
  result, an amendment, a cancel, an answer;
- when a fact it reads changes, which its connector knows from its
  working set;
- when a deadline its state names passes: a stall, a backoff. The
  connector's top arms one deadline per procedure task, from the wall
  time its state keeps; the procedure arms none.

### 6.3 What a step decides

One decision, committed with its new state:

- effects, into the outbox;
- batches of delegates: agents for what needs judgement (producing a
  change, repairing it, resolving a conflict, a review), people for what
  needs a decision;
- amendments and cancels of its delegates;
- proposals, for what its task's authority does not cover, while the
  rest of the step commits;
- messages up: a question, an escalation, news its requester should
  have;
- a hold, when it needs a decision it cannot make;
- its result, which ends its task.

Every effect and batch is checked against its task's authority, as any
decision is (authority.md, section 8); a procedure's authority is its
task's, given by its creator.

### 6.4 Level-triggered

A step decides from the facts as they are and its own state, never from
the event that woke it, so stepping twice on the same facts decides the
same, and a step lost to a restart is made again from what is read. It
never asks for an effect while one for the same purpose is outstanding
in its state, so deciding the same twice asks for nothing twice.
What it waits for (CI on a head, a gate's verdict, its turn to land) is
a condition on the facts, armed with a deadline past which it stalls.

### 6.5 Bounded

Each procedure declares its limits: repairs, conflicts, updates, the
stall of each wait. Past one, it holds its task, saying why, and its
requester hears (tasks.md, 5.5).

## 7. Projections

What a connector writes for people to read of a goal (core.md, 5.2), for
the forge its issue (forge.md, section 12), is a projection: neither a
task nor a procedure, but a mechanism of the connector's, keyed to the
goal.

- **Fed by the root:** the goal's state, its plan as its subtree's tasks
  and their phases, and its milestones as the tree's history records
  them, in the connector's own terms, whenever they change.
- **Its effects** have keys of their own (4.4), are checked against the
  authority the project's policy gives its goals' projections, and are
  made through the outbox like any other.
- **Its state** (what it last wrote, by a digest) is kept with the goal,
  and outlives it until its last write, made as the goal closes, has
  settled.

## 8. Tools and brief sections

- **Reads an agent may call** (engine.md, 7.3): typed, each with an
  answer bounded in bytes and charged to the connector's request
  budget; for the forge, a pull request, its files and diff, CI's output
  on a head, an issue and its comments (forge.md, section 2).
- **Sections a brief carries** for the resources a task names, typed and
  cut by the brief child domain's rules (engine.md, section 9); for the
  forge, the pull request against its base, CI's failures with their
  output, reviews' remarks, a conflict and what landed.
- **Neither is state.** What a run reads is not kept by the connector
  beyond its answer.

## 9. Workspaces

What a worker prepares for a run comes from the resources its task
names:

- **For each repository** a task reads or writes: its remote, the
  directory and name the agent sees, where to start (a base branch, a
  branch, a commit, saved work, or a merge in progress, worker.md,
  4.1), whether it may be written and the branch a change is pushed to,
  and the identity for git on it.
- **The connector says which of its resources are checked out.**
  Today only the forge's repositories are; a connector whose resources
  are not files (a test environment) gives the run tools to reach them
  instead, and names nothing for the worker to prepare.
- **Context resources** of the project may be added read-only, so code
  that refers to a sibling by path works.

## 10. Ownership, projections and drift

- **Owned objects are recorded by key** (4.4), so ownership is the
  store's, whatever the system shows.
- **Projections are written, never read back** (core.md, 7.1): what a
  connector writes for people to read is made through the outbox from
  the store's state, and nothing of it is parsed as state.
- **temper's own moves.** A held resource moves under temper's own hand
  in two ways: an effect the connector made, known by its key and its
  outcome, and a run's push, which the engine hears of only with the
  run's answer. While a resource's writer slot is taken (3.3), a move by
  temper's identity that only goes forward (a fast-forward of a branch)
  is taken as the writer's, provisionally, and confirmed by the head its
  answer reports, or, if its attempt is settled as lost, by a fresh read
  then. A procedure decides nothing on a resource while its writer slot
  is taken, rather than deciding on a head that is still moving.
- **Drift is a change temper did not make** to something temper owns and
  relies on: a move by another identity, a move that does not go
  forward, a move while no writer slot is taken and no effect of
  temper's is in flight, an object closed, deleted or retargeted. It
  holds the task that relies on it, saying what changed and when, and
  the web shows it; nothing is written over.
- **What is not drift:** a fact a decision reads, such as a base's tip,
  is simply read again, so a moved base is news; and a landing temper
  did not make is news like any other landing.
- **Participating objects** are those others write too. What others
  write there (comments, reviews) is read into the inbox of the task
  that deals with the object, as news on its topic; what temper writes
  there is a participant's, possibly a proposal for a person to accept
  first.

## 11. Adopting a system

- **Adoption** adds a resource to a project with a role, by a person
  whose role allows it (people.md, section 5). The connector reads what
  temper may do there (for a repository: read, write, merge, what its
  protection allows) and keeps it with the project, and the project's
  ceiling follows from it (authority.md, section 6).
- **Nothing is required** of the system beyond the access temper is
  given: no webhook, no label, no protection, no CI. What is missing
  narrows what temper may do there or slows it down, and the web says
  so: a repository without webhooks is kept up by polling alone; one
  without CI lands on the checks its project configures; one where
  temper may not merge has its landings waiting for someone else's
  merge (core.md, 7.2).
- **Provisioning** (making the system's objects: a repository, its
  webhooks, its protection) is an optional set of effects a person may
  ask for, never a step of adoption.
- **One deployment's.** temper's branch prefix on a repository is the
  deployment's: adoption refuses a repository where another deployment's
  branches already live under that prefix, and a second deployment on
  the same forge uses a prefix of its own.
- **What temper may do is read again** when the system refuses it
  something it believed it could, and the project's ceiling narrows
  with it.

## 12. Load

Each connector keeps its system's request budget:

- **Fresh reads first,** an effect's own checks among them, then
  effects, then keeping up, then slow passes, with a share of each
  window kept for the last two.
- **Load follows change, never history:** what a connector asks of its
  system per pass grows with what changed and with the live resources,
  never with the system's past, and the worlds count it.
- **A refusal for the rate** stops every call to that system until its
  reset.

## 13. Restart

At a restart (engine.md, section 6), a connector is given the live
resources the store names and the outbox's entries, and:

1. reads afresh what its system owns of those resources, within its
   budget, before any of its procedures steps;
2. looks for every outbox entry it was given, by its key or the state
   it leads to (4.3);
3. steps each of its procedures once, on what it read.

Nothing it held in memory before the restart is needed: its working set
is read again; its procedures' state, its projections', its outbox and
its own state (engine.md, 5.4) are the store's.

## 14. A second connector: test environments

A sketch, to check the contract, not a design:

- **Resources:** environments, named by their pool and their name;
  deployments in them.
- **Effects:** deploy a head of a repository into an environment;
  tear one down. A deployment is a creation, keyed.
- **Events and topics:** a deployment ready or failed; a check's result
  on it.
- **Procedures:** "deploy, then check": a gate a change could ask for,
  whose result is a verdict at the head it deployed.
- **Tools:** an environment's endpoints and logs, for an agent exploring
  it (core.md, section 9, T10).
- **Holds:** an environment a task deploys into is held by it.
- **Workspaces:** none: a run reaches an environment through its tools.

Two things it asks of the core that the forge does not: a gate whose
facts come from one connector (a deployment's check) while the landing
it guards is another's (the forge's merge), which authority's
requirements would then gather from both (authority.md, section 13); and
write holds on resources that are scarce, which a task may need to wait for
rather than be refused.

## 15. The world

Each connector has its worlds: its subtree's, with its system's fake
below it and the root scripted above it; and the engine's, where it runs
with every other part. What every connector's world checks, beyond its
own stories: keyed effects made once across restarts at drawn moments,
uncertain entries found rather than repeated; procedures that decide
the same when stepped twice on the same facts; topics whose news reaches
every subscriber once, classified; and a load that follows change, the
system preloaded with ten times the history making the same calls while
idle. The forge's are forge.md, section 18.

## 16. Open questions

- **Waiting for a scarce resource:** whether a write hold refused should be a
  wait, for environments and the like, rather than a refusal.
- **Requirements across connectors** (section 14).
- **Connectors added without a build:** the set is closed in the code,
  as the programming model's rules make it. A connector whose behaviour
  is data (a generic HTTP system) would be one connector of that kind,
  not a way around the closed set.
- **Classifying news with an LLM:** whether a cheap model should judge
  relevance where files do not tell (tasks.md, section 13).
