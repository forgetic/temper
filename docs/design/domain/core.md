# temper's core

Provisional, 2026-10-04. What temper is beneath its parts: an engine
that runs work done by agents, procedures and people, keeps it in its
own store, and drives external systems through connectors, of which the
forge is the first. It is the model every other document in this
directory builds on, and the first to read (README.md gives the order
and the names). The mechanics are those of skein's
`docs/foundation/programming-model.md`. What is still open is listed in
section 11.

## 1. In one page

- **A generic agentic engine.** temper runs work: it keeps what was
  asked, who does it and what came of it; it runs agents on workers,
  steps its own procedures, asks people, and routes to each what it
  needs to hear. It is to be as flexible as a coding agent at a
  terminal, with what such an agent lacks: work that outlives processes
  and machines, many agents at once, rules no agent can loosen, and
  effects on other systems made once.
- **Five primitives** (section 3): the **task**, what is asked; the
  **executor**, who does it: an agent, a procedure or a person; the
  **message**, everything a task hears once it has started;
  **authority**, what a task may do, narrowed whenever it delegates; and
  the **connector**, an external system. Everything else is built from
  them: plans, coordinators, chats, goals, reviews, landings.
- **Structure belongs to the work.** Plans and coordinators are not
  mechanisms: a plan is the tasks one task has delegated, and a
  coordinator is an agent whose charter is to delegate and supervise
  (section 6). Agents make structure with tools, as a coding agent
  spawns sub-agents; the engine makes it durable, schedules it, and
  holds it to its authority.
- **temper owns its state.** Projects, tasks, their inboxes and results,
  transcripts, proposals, notes and the outbox live in temper's store,
  which only the engine reads and writes. The engine holds bounded
  working sets, rebuilt from the store and the connectors after a
  restart. The forge keeps git, pull requests and CI: what is the
  forge's, not what is temper's.
- **A decision commits once** (section 4): its change of state and the
  effects it asks for, in one commit to the store. Nothing leaves the
  engine (an effect, a run started, an answer to a person) before the
  commit it follows from is durable. Effects are made from the outbox,
  keyed, so a write repeated after an uncertain failure finds what the
  first made.
- **External systems are connectors** (3.5). Each gives resources,
  effects, events, procedures and tools, and the core knows none of
  them. The forge is the first (forge.md); test environments are a
  likely second.
- **What temper does on a system belongs to temper** (section 7). temper
  writes only to what it owns, lands into shared branches, and takes
  part as one participant in what others own. Anything else that changes
  is drift, found by reading afresh and reconciled, never coordinated
  with.
- **temper is where people work** (section 8). People talk to agents,
  set goals, accept proposals, answer questions, steer and watch through
  temper's web. They may read the forge, which temper keeps readable,
  but need not act there. Feedback on code is the agents' business;
  people steer at the level of goals, plans and outcomes.
- **Mechanics are code, judgement is an LLM's.** Procedures carry out
  what needs no judgement, such as a change's way to landing or a branch
  brought up to date. An LLM is woken for what does: a conflict, a
  failure past its repairs, a plan to revise.
- **Bounded, and complete in its domain.** What the engine holds grows
  with the work in progress, never with history, which stays in the
  store. Every entity has a limit, refused at the entrance. A world of
  domains and fakes runs everything temper does, with no protocol and no
  io (programming-model.md, section 4).

## 2. The system

```
people    the web: chats, goals, proposals to accept, questions to answer, watching
engine    where work is decided: tasks, authority, connectors; the store's only client
store     temper's durable storage: projects, tasks, inboxes, transcripts, notes, the outbox
workers   execution hosts: workspaces, agent runs, pushes; they relay the rest
agents    LLM work: one run per agent process, reporting to its worker
systems   what connectors drive: the forge first, with git, pull requests, CI and issues
```

- **One engine per deployment.** A deployment is one engine, its store,
  its workers and its configuration. The engine is the only writer of
  the store and the only client of every connector's API, so it never
  needs a compare-and-swap against itself. Scaling out is a later
  problem.
- **The engine decides; workers host; agents think.** Workers dial the
  engine, prepare workspaces and run agents (worker.md); agents do the
  LLM work and reach the engine only through tools their worker relays
  (agent.md: smith, as temper uses it). What the engine is made of is
  engine.md.
- **The store is assumed.** It keeps durably what the engine gives it,
  commits several changes as one, applies commits in order, and answers
  bounded reads. What the engine asks of it is engine.md, section 5; what
  it is and how it is built is not this directory's concern.
- **People reach temper through the web,** which the engine serves
  (people.md). A person is known by signing in through the forge; what
  they may do in a project follows from their role there.

## 3. The five primitives

### 3.1 Task

A unit of work someone asked for (tasks.md):

- **its spec,** in words and in typed parameters, and its **result
  contract**, which says what counts as done: a report; a verdict from a
  closed list, with a contract per verdict; a change landed; a person's
  choice;
- **its requester:** the task, or the person, that asked for it.
  Requesters link tasks into a tree, rooted at a person or at the
  deployment;
- **its dependencies:** the tasks it starts after. A dependency means
  only "after it is done", and its result reaches the task: an agent's
  brief carries it. Dependencies link tasks into a graph without cycles;
- **its executor** (3.2), **its authority** (3.4) and **its wake
  policy** (3.3);
- **its lifecycle:** waiting on its dependencies, active, closing (its
  run, its delegates and its effects settling, its resources released),
  then done, failed or cancelled; held for a decision at any point before
  it ends (tasks.md, section 5);
- **its result,** delivered to its requester as a message, and kept.

Tasks are created in **batches.** A batch, with the dependencies among
its tasks, is checked whole (no cycles, within the limits and the
creator's authority) and made at once, so that a plan is never half
made.

### 3.2 Executor

Who carries a task out. A task has one:

- **An agent:** an LLM, set up by a charter: instructions, models,
  tools, the resources its workspace holds, the outcome it must produce.
  Its transcript belongs to its task and is kept turn by turn in the
  store. It is activated as a run on a worker, which ends by parking or
  finishing; between runs, nothing of it is live (engine.md, section 7).
- **A procedure:** engine code, pure: given the task's state, what it
  has heard and the facts it reads afresh, it says which effects to
  make, which tasks to create, amend or cancel, and whether it is done.
  Procedures belong to connectors, such as the forge's change
  (forge.md, section 8), or to the core, such as a recurring task
  (tasks.md, section 9). They cost no LLM turns.
- **A person,** who answers in the web: a choice, a decision, an
  approval, an answer (people.md, section 8).

All three can delegate, each within its authority: an agent with its
tools, a procedure in its code, a person in the web.

### 3.3 Message

Everything a task hears once it has started:

- results and questions from the tasks it delegated;
- amendments, cancels, answers and decisions from its requester;
- a person's words;
- news from what it subscribed to: a connector's events, another task's
  state, a timer.

What waits for its decision, a proposal or an escalation from below, is
not a message but an entry of its own, which always wakes it (tasks.md,
section 8).

Each task has an inbox, durable in the store, and a **wake policy**,
which decides when its executor runs for what has arrived: a run for an
agent, a step for a procedure, a notification for a person. A wake costs
an agent an LLM turn, so news is kept, unless its connector finds it is
not the task's concern after all, and only some of it wakes:
a person's words, a held delegate, an escalation, a failure past its
repairs, a landing that overlaps the task's work, the last of its
delegates done. A policy may batch. What does not wake a task waits in
its inbox, and its next run hears it (tasks.md, section 7).

### 3.4 Authority

What a task may do (authority.md):

- the tools its agent has, and the effects it may make, on which
  resources: push to which branches, land into which, open issues where,
  deploy to which environment;
- what it may spend: tokens, money, time;
- what it may delegate: which kinds of task, how many, how deep, with
  what authority.

**Delegation only narrows it:** a task gives its delegates at most what
it has, and what it gives of its budget is no longer its own to spend.
The root of every tree is the deployment's rules and the project's
policy, which no task loosens.

**Beyond its authority, a task proposes.** The action becomes a proposal
to whoever holds the authority, up the tree: a task above it, or a
person. They accept it, and it is made with their authority, or reject
it, and the task hears why. Acceptance, escalation and widening a plan's
reach are all this one mechanism.

### 3.5 Connector

An external system, as temper sees it (connectors.md):

- **resources:** what has an identity there (a repository, a branch, a
  pull request, an environment), which a task names and a workspace may
  hold. A resource a task writes is held by that task alone while it
  writes;
- **effects:** what temper does there, made through the outbox
  (section 4);
- **events:** what changed there, delivered to the tasks that
  subscribed. They are hints: the state they point at is read afresh;
- **procedures:** the mechanics of working there, as executors (3.2);
- **tools:** what an agent may call to read it, and the sections a brief
  carries about it.

The set of connectors is closed in the code: each is a part of the
engine's domain, and the engine's top level knows them all. The tasks,
their inboxes and their authority know none of them (engine.md,
section 3).

## 4. Decisions, the store and the outbox

- **A decision commits once.** A run's answer, a procedure's step, a
  person's request, a tool call: each is decided against the engine's
  state, and what it changes (tasks made, a result kept, an inbox read,
  a proposal accepted) and the effects it asks for go to the store in
  one commit.
- **Nothing leaves before its commit.** An effect is made, a run is
  assigned, a worker's answer is acknowledged and a person is answered
  only once the commit it follows from is durable. The engine's state
  may run ahead of the store by the commits in flight, which are
  bounded; if the engine dies meanwhile, nothing anyone saw depended on
  what was lost, and a restart starts from the last commit (engine.md,
  section 5).
- **The outbox makes each effect, keyed.** An effect committed is an
  entry in the outbox, with a key derived from what the effect is for
  (an agent's call, a procedure's purpose), never from when it was
  asked. Its connector makes it; a creation carries its key where the
  system keeps it, so a write repeated after an uncertain failure finds
  what the first made (connectors.md, section 4). Only entries that may
  have been made are looked for before they are tried again, and only
  they wait out a write's lifetime: after a failure that leaves it
  uncertain, and, after a restart, every entry not committed as made.
- **Facts are read afresh.** What a connector's system owns (a branch's
  tip, a pull request's head, CI) is read again before a decision that
  depends on it, as level-triggered state. Events are hints that
  something changed, never the state itself.
- **A restart reads the store,** then reads afresh, from the connectors,
  what they own of the live work, and looks for what uncertain effects
  may have made (engine.md, section 6).
- **A commit that fails stops the engine.** The store is assumed to do
  what temper needs; when it cannot, the engine stops rather than act on
  state it could not keep, and a restart starts from the last commit.

## 5. Projects and goals

### 5.1 Projects

A project is a body of work people steer, kept in the store:

- **its repositories,** each adopted with a role: *owned* or adopted,
  where temper writes and lands; a *fork* of an upstream, where temper
  owns the fork and takes part upstream; or *context*, checked out and
  never written (forge.md, section 4). A project's repositories may
  span forges, later systems of other kinds;
- **its home repository,** where its goals' issues are kept;
- **its policy:** the most any task in the project may do, the rules its
  landings follow, its spend per period (authority.md, section 6);
- **its people and their roles** (people.md, section 4);
- **its notes** (engine.md, section 10).

Every task belongs to one project, which its delegates inherit.

### 5.2 Goals

A goal is a task people track: the task a person asks for, or that a
chat proposes and a person accepts, marked so. It is a role a task
takes, not a sixth primitive. A goal a chat proposes is requested by the
person who accepts it, not by the chat, so it outlives the conversation
it came from (tasks.md, section 8). A tracked task gets:

- **a priority** among the project's goals, which orders its changes in
  a landing queue (forge.md, section 9);
- **an issue,** projected on the project's home repository: the goal,
  its plan as a task list, and its milestones (forge.md, section 12);
- **news of landings** in the repositories its subtree changes, through
  a subscription the engine keeps for it (forge.md, section 7);
- **a scope for notes,** which its subtree reads and writes.

Any task may be tracked; most are not. A chat, a review or a repair is
not a goal; "add OAuth login" is.

## 6. Plans and coordinators

### 6.1 What a plan is

What today's plan keeps in one value is three things:

1. **The strategy:** why this approach, in what order, what was dropped
   and why. It is text, and it is the LLM's: its transcript, its notes,
   the goal's issue. It changes freely.
2. **The structure:** tasks and their dependencies. It is the engine's
   state, and the engine runs it.
3. **The authority** to run it: the budget, the kinds and number of
   tasks, the branches and repositories. It is the task's authority
   (3.4).

Apart, **a plan is the subtree of tasks one task has delegated.**
Revising it is creating, amending or cancelling tasks, each with its
reason, which the store keeps as the plan's history; a cancelled task's
run is cancelled, and its delegates with it. Planning everything up
front is one batch (3.1): checked whole and, beyond its creator's
authority, one proposal for a person to accept.

### 6.2 What a coordinator does

A coordinator is an agent whose charter is to break a goal down,
delegate, watch its inbox, revise, and report. The engine knows nothing
particular of it: it is an agent's task with delegates, woken by what
its wake policy lets through. It may be the chat a person talks to,
which keeps the conversation's context, or a fresh agent that chat
delegates the goal to, which keeps a long conversation out of the
supervision. The choice is the charter's.

A coordinator woken rarely and far apart starts fresh by default, from a
brief: its goal, the plan's state, its inbox and its notes, with the
reasons recorded at each revision carrying its reasoning from one run
to the next. A chat resumes its transcript, which the store keeps whole.
Either is its charter's choice (engine.md, 7.4).

### 6.3 Shapes of work

Not every goal needs a coordinator. The same primitives make each of
these:

1. **Done alone.** A person asks a chat for a small fix. Its agent
   makes the change, pushes it, and hands the branch to a task the
   forge's change procedure runs to landing (forge.md, section 8). There
   is no plan and no coordinator.
2. **Delegated once.** "Review this pull request for memory safety": the
   chat's agent creates one task, with a reviewer's charter, hears its
   verdict, and tells the person.
3. **A static plan.** Tasks with dependencies and no LLM over them.
   Results, failures and escalations go to the requester, a person or a
   chat.
4. **Coordinated.** An agent supervises and revises, for the goals that
   need it.
5. **Recurring.** A timer creates tasks from a template: nightly
   benchmarks that file regressions, a dependency bump (tasks.md,
   section 9).

### 6.4 Two levels of delegation

An agent delegates in two ways, and chooses between them:

- **sub-agents within its run** (smith's `run.md`, 5.3): cheap,
  ephemeral, sharing its workspace and its budget, and ended with the
  call that asked for them;
- **tasks:** durable and scheduled, with an executor, a workspace and an
  authority of their own, surviving restarts.

An agent reaches the engine through tools its worker relays: delegate a
batch of tasks; message; amend, cancel and release its delegates;
decide what they propose; propose; subscribe; make an effect; note and
recall; wait; finish. Each is checked against its task's authority
(engine.md, 7.3).

## 7. temper and external systems

### 7.1 What belongs to temper

- **The rule.** What temper does on an external system belongs to
  temper: the pull requests, branches, issues, comments, reviews and
  statuses it makes. temper writes only to what it owns, with two
  exceptions: it lands changes into shared branches, such as a
  repository's main, and it takes part as one participant in objects
  others own (7.2).
- **Ownership is the store's.** The store records every object temper
  makes, with the key it made it under. On the forge, ownership shows as
  authorship, by temper's own user, and as temper's branch prefix, which
  branch protection can reserve to it. Labels are for display only.
- **Projections are written, never read back.** What temper writes for
  people to read (an issue's body, a milestone comment, a review's text)
  is a projection of the store's state: keyed, made through the outbox,
  and never parsed back as state, beyond finding temper's own creations
  by their keys.
- **What the system owns is read afresh** before each decision that
  depends on it (section 4).

### 7.2 Owned and participating

Each object temper deals with is one of two kinds:

- **Owned,** the default: temper is its only writer. What comes back
  from it is only the system's facts: CI on a head, a merge, a state.
- **Participating:** others write too. temper reads what they write
  (comments, reviews) into the inbox of the task that deals with the
  object, and writes as one participant among several. A write that
  goes out may need a person's acceptance.

Participating serves three cases with one mechanism, none of them
needed first: outside contributions to a project temper works on; a
plugged-in repository where temper may not merge, so that landing is
waiting for someone else's merge; and temper's own contributions to
upstream projects (forge.md, sections 13 and 14).

### 7.3 Plugging temper in

- **temper adopts systems as they are.** Making them (a repository, its
  webhooks, its protection, its labels) is an optional convenience above
  that, never something temper assumes.
- **Nothing is required of a system** beyond the access temper is given.
  temper's rules are its enforcement; whatever protection a system has
  is a second net.
- **What temper may do there is read at adoption,** and the project's
  policy keeps within it (connectors.md, section 11).

### 7.4 Drift

Another party may change what temper relies on: a person pushes or
merges by hand while temper is down, or when temper, developing itself,
has broken itself; a maintainer merges an outside contribution; someone
closes temper's pull request. temper supports no workflow built on this,
but it must be safe:

- facts are read afresh before each decision; pushes are fast-forwards,
  never forced; merges are made at exactly the head the decision saw;
- a landing temper did not make is news like any other landing;
- a change to an object temper owns (its pull request closed, its branch
  moved) holds the task that relies on it, and the web says why.
  Nothing is written over (connectors.md, section 10).

## 8. People

People are parties, as tasks are. Each has an inbox in the web, and
creates, messages, amends, cancels, releases and accepts within the
authority their role in a project gives them (people.md):

- **A chat** is an agent's task whose requester is a person: each of
  their words wakes it, and it resumes its transcript. It is how a
  person usually starts work: the chat answers, makes a small change
  itself, delegates, or proposes a goal.
- **A person's inbox** holds what waits for them: questions from the
  tasks they asked for, proposals they may accept, held tasks escalated
  to them, tasks whose executor they are.
- **The web is generic:** tasks as a tree, conversations, what waits
  for a person, and live runs to watch or stop. It knows the five
  primitives, not the shapes of work built from them.

## 9. A goal, as tasks

A person, P, reaches a plan for adding OAuth login to temper:

```
P, a person
├─ T0  a chat with P about temper          agent, a chat's charter
└─ T1  add OAuth login                     agent, a coordinator's charter; tracked;
   │                                       T0 proposed it, P accepted it as P's own
   ├─ T2  spike: library X                 agent, read-only: a report
   ├─ T3  spike: a hand-rolled flow        agent, read-only: a report
   ├─ T4  choose       after T2, T3        person P: a choice
   ├─ T5  design       after T4            the forge's change, a design review as its gate
   ├─ T6…T9  changes   T5 as their input   the forge's change, made by T1
   │                                       once it read T5's result
   │     T7 amended while live: a memory-safety review as a gate
   └─ T10 explore      after T6…T9         agent, with a test environment
```

- **T1 is a proposal.** Its authority (a budget, landing into main) is
  beyond T0's, so P accepts it. It asked to be requested by its
  accepter, so it is P's, funded from what P's role may spend in the
  project, and outlives T0, which keeps a reference to it. Within T1's
  authority, its own delegations need nobody's acceptance.
- **T1 is a goal.** Its issue shows the plan as T1 revises it; its
  changes queue for main at its priority; landings in temper are news
  for it.
- **Results flow along dependencies:** T2's and T3's reports reach T4,
  which P answers in the web; T4's choice reaches T5's brief.
- **T6…T9 are a revision,** made by T1 once T5 landed, within its
  authority; T5 has ended, so its result reaches them as an input, not
  a dependency.
- **The memory-safety review is one amendment:** T1 adds a gate to T7.
  The change procedure runs a review task at each new head of T7, and
  holds the merge until it passes.
- **Under each change,** the procedure makes its own tasks: producing,
  repairs, reviews, resolving conflicts (forge.md, section 8).
- **Had P asked for a small fix,** T0 would have made the change itself
  and handed it to one change task, and there would be no T1.
- **Had P closed T0** while T1 ran, T1 would go on: it is P's.

How T6…T9 land while another goal lands into the same branch is
forge.md, section 15.

## 10. What temper promises

The worlds' referees hold temper to these, seen from outside (engine.md,
section 15):

- **Authority holds.** No effect is made, no task created and no budget
  spent beyond what the deployment's rules, the project's policy and the
  chain of authority above the task allow, except by a proposal a holder
  of that authority accepted.
- **Once.** Every keyed effect is made at most once, across restarts
  included; every task in a batch is made, or none is.
- **Nothing ahead of its commit.** Nothing a person was answered, a
  worker was assigned or acknowledged, or a system was asked to do,
  depended on state the store did not keep. What people watch live may
  run ahead of it, as views are best effort.
- **Order.** A task starts only after its dependencies are done and
  closed; one run at most writes a resource at a time; a merge lands
  exactly the head its decision saw; a task ends only after its
  delegates.
- **Nothing is lost.** A result reaches its requester; a person's words
  reach the task they were written to, or that task ends; a proposal is
  decided, withdrawn, or still waiting where a holder can see it.
- **Nothing is written over.** temper never forces a push, never writes
  to an object it does not own except as a participant, and never
  overwrites a change it did not make.
- **Bounded.** What the engine holds stays within its limits, whatever
  the history in the store; every story ends within a bound.

## 11. Open questions

- **Agents as identities** that serve many tasks, such as a project's
  maintainer. The model allows them; a long-lived task serves meanwhile.
- **Procedures written by agents,** deterministic workflows like a
  coding agent's scripts. Until they are wanted, procedures are Rust,
  owned by connectors or the core; dependencies stay "after it is done",
  and conditions are a coordinator's judgement.
- **Wake policies:** what wakes a task, whether overlap by files is
  enough, and whether a cheap model should triage news before a
  coordinator wakes. Tuned from use (tasks.md, section 13).
- **A person's approval as a rule:** whether temper alone enforces it,
  or posts the approval as the person, with the forge credential they
  signed in with, so that protection requires it independently
  (people.md, section 14).
- **Contributions in and out** (7.2): neither needed first.
- **Scaling out:** one engine per deployment until it binds.
