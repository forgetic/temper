# temper's core

Draft, 2026-10-04. What temper is beneath its parts: a generic engine
that runs work done by agents, procedures and people, and drives
external systems through connectors, of which the forge is the first.
It records the direction agreed in a design discussion on 2026-10-04.
Until it is adopted, the per-part documents (`engine-domain.md`,
`worker-domain.md`, `agent-domain.md`, `forge.md` and the rest) describe
what is built; section 8 says what of them this would replace. The
mechanics are those of skein's `docs/foundation/programming-model.md`.
What is still open is listed in section 9.

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
  them.
- **Structure belongs to the work.** Plans and coordinators are not
  mechanisms: a plan is the tasks one task has delegated, and a
  coordinator is an agent whose charter is to delegate and supervise
  (section 4). Agents make structure with tools, as a coding agent
  spawns sub-agents; the engine makes it durable, schedules it, and
  holds it to its authority.
- **The store is temper's durable storage,** assumed here to exist and
  to do what temper needs. Projects, tasks, their results, transcripts,
  the history of plans, notes and the outbox are temper's own, and live
  in it. The engine process holds bounded working sets, rebuilt from the
  store and the connectors after a restart.
- **External systems are connectors** (3.5). Each gives
  resources, effects, events, procedures and tools, and the core knows
  none of them. The forge is the first (section 6); test environments
  are a likely second.
- **What temper does on a system belongs to temper** (section 2). temper
  writes only to what it owns, lands into shared branches, and takes
  part as one participant in what others own. Anything else that changes
  is drift, found by reading afresh and reconciled, never coordinated
  with.
- **temper is where people work.** People talk to agents, accept
  proposals, steer and watch through temper's web. They may read the
  forge, which temper keeps readable, but need not act there. Feedback
  on code is the agents' business; people steer at the level of goals,
  plans and outcomes.
- **Mechanics are code, judgement is an LLM's.** Procedures carry out
  what needs no judgement, such as a change's way to landing or a branch
  brought up to date. An LLM is woken for what does: a conflict, a
  failure past its repairs, a plan to revise.

## 2. temper and external systems

### 2.1 What belongs to temper

- **The rule.** What temper does on an external system belongs to
  temper: the pull requests, branches, issues, comments, reviews and
  statuses it makes. temper writes only to what it owns, with two
  exceptions: it lands changes into shared branches, such as a
  repository's main, and it takes part as one participant in objects
  others own (2.2).
- **Ownership is the store's.** The store records every object temper
  makes, with the key it made it under. On the forge, ownership shows as
  authorship, by temper's own user, and as temper's branch prefix (such
  as `temper/`), which branch protection can reserve to it. Labels are
  for display only: anyone with write permission can remove one, a
  plugged-in temper may not be allowed to create them, and Forgejo
  silently drops a label it does not know (forge.md, 3.5).
- **Projections are written, never read back.** What temper writes for
  people to read (an issue's body, a milestone comment, a review's text)
  is a projection of the store's state: keyed, made through the outbox,
  and never parsed back as state, beyond finding temper's own creations
  by their keys.
- **What the system owns is read afresh.** On the forge: refs and
  commits; pull requests' heads, states and mergeability; CI; branch
  protection and permissions. Each is read again before a decision that
  depends on it, as level-triggered state; events are hints
  (engine-domain.md, section 1).

### 2.2 Owned and participating

Each object temper deals with is one of two kinds:

- **Owned,** the default: temper is its only writer. What comes back
  from it is only the system's facts: CI on a head, a merge, a state.
- **Participating:** others write too. temper reads what they write
  (comments, reviews) into the inbox of the task that deals with the
  object, and writes as one participant among several. A write that
  goes out may need a person's acceptance.

Participating serves three cases with one mechanism, none of them
needed first:

- outside contributions to a project temper works on, whose issues and
  pull requests temper may triage and review;
- a plugged-in repository where temper may not merge, so that landing
  is waiting for someone else's merge;
- temper's own contributions to upstream projects (6.8).

The comment inbox that the forge child domain has today
(engine-domain.md, 4.3) is this mode's machinery. Owned work does not
need it; it is kept, not designed out.

### 2.3 Plugging temper in

- **temper adopts repositories as they are.** Making them (creating a
  repository, its webhooks, its protection, its labels) is an optional
  convenience above that, never something temper assumes.
- **Webhooks are optional.** Polling is the backstop (forge.md,
  section 6); webhooks only cut the latency. The web shows a repository
  that has none.
- **Nothing on the forge is required of a repository:** no label, no
  protection, no CI. temper's rules are its enforcement, and the
  protection a repository has is a second net. A repository without CI
  gates its changes on checks its project configures instead.
- **Permissions are read at adoption,** and plans keep within them. Read
  access is enough for a repository a workspace only reads; write access
  lets temper push its branches and open pull requests; protection that
  keeps temper from merging makes landing a wait for someone else's
  merge (2.2).
- **A project's repositories have roles:** owned or adopted, where temper
  writes and lands; a fork of an upstream, where temper owns the fork
  and takes part upstream (6.8); or context, checked out and never
  written.

### 2.4 Drift

Another party may change what temper relies on: a person pushes or
merges by hand while temper is down, or when temper, developing itself,
has broken itself; a maintainer merges an outside contribution; someone
closes temper's pull request. temper supports no workflow built on this,
but it must be safe:

- facts are read afresh before each decision; pushes are fast-forwards,
  never forced (worker-domain.md, section 5); merges are made at exactly
  the head the decision saw (forge.md, 3.2);
- a landing temper did not make is news like any other landing (6.4);
- a change to an object temper owns (its pull request closed, its branch
  moved) holds the task that relies on it, and the web says why. Nothing
  is written over.

## 3. The five primitives

### 3.1 Task

A unit of work someone asked for:

- **its spec,** in words and in typed parameters, and its **result
  contract**, which says what counts as done: a report, a verdict from a
  closed list with a contract per verdict, a change landed, a person's
  choice (as an outcome spec does today, agent-domain.md, 4.1);
- **its requester:** the task, or the person, that asked for it.
  Requesters link tasks into a tree;
- **its dependencies:** the tasks it starts after. A dependency means
  only "after it is done", and its result reaches the task: an agent's
  brief carries it (engine-domain.md, 5.2). Dependencies link tasks into
  a graph without cycles;
- **its authority** (3.4) and **its executor** (3.2);
- **its lifecycle:** waiting on its dependencies, ready, claimed,
  running, applying, held for a person, then done, failed or cancelled,
  as the work hub's is today (engine-domain.md, 4.2);
- **its result,** delivered to its requester as a message, and kept.

Tasks are created in **batches.** A batch, with the dependencies among
its tasks, is checked whole (no cycles, within the limits and the
creator's authority) and made at once, so that a plan is never half
made.

### 3.2 Executor

Who carries a task out. A task has one:

- **An agent:** an LLM, set up by a charter: instructions, models,
  tools, the resources of its workspace, a budget (agent-domain.md,
  4.1). Its transcript belongs to its task. It is activated as a run on
  a worker (worker-domain.md, section 4), which ends by parking or
  finishing; between runs, nothing of it is live.
- **A procedure:** engine code, as pure as the plan child domain is
  today. Given the task's state and what it has heard, it says which
  effects to make, which tasks to create, and whether it is done.
  Procedures belong to connectors (section 6) or to the core, and cost
  no LLM turns.
- **A person,** who answers in the web: a choice, a decision, an
  acceptance, an answer.

All three can delegate, each within its authority: an agent with its
tools, a procedure in its code, a person in the web.

### 3.3 Message

Everything a task hears once it has started:

- results, questions and escalations from the tasks it delegated;
- amendments, cancels and answers from its requester;
- a person's words;
- events it subscribed to (5.2).

Each task has an inbox, durable in the store, and a **wake policy**,
which decides when its executor runs for what has arrived: a run for an
agent, a step for a procedure, a notification for a person. A wake costs
an agent an LLM turn, so news is always kept and only some of it wakes:
a person's message, a held task, an escalation, a failure past its
repairs, a landing that overlaps the task's work (6.4), the last of its
delegates done. A policy may batch, as wake rules do today
(engine-domain.md, 5.4). What does not wake a task waits in its inbox,
and its next run hears it. The details are tuned from use (section 9).

### 3.4 Authority

What a task may do:

- the tools its agent has, and the effects it may make, on which
  resources: push to which branches, land into which, deploy to which
  environment, create issues where;
- what it may spend: tokens, money, time;
- what it may delegate: which kinds of task, how many, with what
  authority.

**Delegation only narrows it:** a task gives its delegates at most what
it has. The root of every tree is the deployment's rules and the
project's policy, which no task loosens (engine-domain.md, section 7).

**Beyond its authority, a task proposes.** The action becomes a proposal
to whoever holds the authority, its requester or a person, who accepts
it, and it is made, or rejects it, and the task hears why. Today's
acceptance, envelope, grants and escalations are all this one
mechanism.

### 3.5 Connector

An external system, as temper sees it:

- **resources:** what has an identity there (a repository, a branch, a
  pull request, an environment), which a task names and a workspace may
  hold. A resource a task writes is held by that task alone while it
  writes, as one live run per item is today (engine-domain.md, 4.1);
- **effects:** what temper does there, made through the outbox (3.6);
- **events:** what changed there, delivered to the tasks that
  subscribed. They are hints: the state they point at is read afresh;
- **procedures:** the mechanics of working there, as executors (3.2);
- **tools:** what an agent may call to read it.

### 3.6 The store and the outbox

The store is assumed: it exists, keeps durably what temper gives it,
and commits several changes as one. What it is and how it is built is
not this document's concern.

- **The store keeps** projects, tasks and their results, transcripts,
  inboxes, the history of every task created, amended or cancelled and
  why, notes, and the outbox.
- **A decision commits once:** its change of state and the effects it
  asks for, in one store transaction. A projector then makes each
  effect, keyed, so that a write repeated after an uncertain failure
  finds what the first made (engine-domain.md, 4.4). Only the outbox's
  uncertain entries are looked for before they are tried again, and only
  they wait out a write's lifetime after a restart.
- **A restart reads the store,** then reads afresh, from the connectors,
  what they own of the live work.

## 4. Plans and coordinators

### 4.1 What a plan is

What today's `Plan` keeps in one value is three things:

1. **The strategy:** why this approach, in what order, what was dropped
   and why. It is text, and it is the LLM's: its transcript, its notes,
   the goal's issue (6.7). It changes freely.
2. **The structure:** tasks and their dependencies. It is the engine's
   state, and the engine runs it.
3. **The authority** to run it: the budget, the kinds and number of
   tasks, the branches and repositories. It is the task's authority
   (3.4).

Apart, **a plan is the subtree of tasks one task has delegated.**
Revising it is creating, amending or cancelling tasks, each with its
reason; a cancelled task's run is cancelled, and its delegates with it.
Planning everything up front is one batch (3.1): checked whole and,
beyond its creator's authority, one proposal for a person to accept.

### 4.2 What a coordinator does

A coordinator is an agent whose charter is to break a goal down,
delegate, watch its inbox, revise, and report. The engine knows nothing
particular of it: it is an agent's task with delegates, woken by what
its wake policy lets through. It may be the session a person talks to,
which keeps the conversation's context, or a fresh agent that session
delegates the goal to, which keeps a long chat out of the supervision.
The choice is the charter's.

A coordinator woken rarely and far apart starts fresh by default, from a
brief: its goal, the plan's state, its inbox and its notes, with the
reasons recorded at each revision carrying its reasoning from one run
to the next. A chat resumes its transcript, which the store keeps whole.
These are today's defaults (engine-domain.md, section 6), with the
transcript in the store instead of a snapshot that is only a cache.

### 4.3 Shapes of work

Not every goal needs a coordinator. The same primitives make each of
these:

1. **Done alone.** A person asks a session for a small fix. Its agent
   makes the change and gives it to the forge's change procedure to land
   (6.3). There is no plan and no coordinator.
2. **Delegated once.** "Review this pull request for memory safety": the
   session's agent creates one task, with a reviewer's charter, hears its
   verdict, and tells the person.
3. **A static plan.** Tasks with dependencies and no LLM over them.
   Results, failures and escalations go to the requester, a person or a
   session.
4. **Coordinated.** An agent supervises and revises, for the goals that
   need it.
5. **Recurring.** A timer creates tasks from a template: nightly
   benchmarks that file regressions, a dependency bump.

### 4.4 Two levels of delegation

An agent delegates in two ways, and chooses between them:

- **sub-agents within its run** (agent-domain.md, section 5): cheap,
  ephemeral, sharing its workspace, and ended with the call that asked
  for them;
- **tasks:** durable and scheduled, with an executor, a workspace and an
  authority of their own, surviving restarts.

### 4.5 An agent's tools for the engine

An agent reaches the core through delegated tools, which its outlets
already are (agent-domain.md, 4.3): delegate a batch of tasks; message;
amend and cancel its delegates; propose; subscribe; make an effect;
note and recall; wait; finish. Each is checked against its task's
authority.

## 5. Communication

### 5.1 Who talks to whom

- **Along delegation:** a requester and its executors exchange results,
  questions, escalations, amendments and cancels. An escalation goes up
  the tree until a task, or a person, has the authority to decide.
- **By reference:** a task may message any task it holds a reference to:
  its requester, its delegates, or one whose reference it was given, as
  a coordinator may introduce two of its delegates. There is no
  broadcast, so who may talk to whom stays visible and bounded.
- **People are parties.** Each has an inbox in the web, and creates,
  messages, amends, cancels and accepts within their authority. The web
  is generic for it: tasks as a tree, conversations, and what waits for
  a person.

### 5.2 Subscriptions

A task's standing interest in what changes: a connector's events (a
repository's landings, CI on a head), another task's state, a timer.
What arrives is a message, under the task's wake policy.

## 6. The forge connector

### 6.1 What it gives

- **Resources:** repositories, branches and pull requests; checkouts,
  which workers prepare (worker-domain.md, section 5).
- **Effects:** open, update and merge a pull request; post a review or a
  commit status; create, edit and close issues; comment; delete a
  branch.
- **Events:** CI on a head, a landing on a branch, a pull request's
  state.
- **Procedures:** the change (6.3) and the issues' projection (6.7).
- **Tools:** reading a pull request, its files, and its CI's output.

What it costs the forge stays within the request budget and the load
rules (forge.md, section 6).

### 6.2 What the forge holds

Git; pull requests as the unit of a change; CI; branch protection, as a
second net; and a record people can read. With no people writing there,
almost nothing comes back from it but CI and drift: no comment inbox for
owned work, no hand-in label, no messages written for a person, no
people's reviews or permissions read for a decision.

### 6.3 The change procedure

A change's way to landing, which the plan child domain decides today
(engine-domain.md, 5.3), as a procedure:

1. **Produce:** a task for an agent, which pushes its branch
   (agent-domain.md, 4.4).
2. **Open** its pull request into its landing branch.
3. **CI** runs on its head. A failure is a repair task, given the
   failing output.
4. **Gates:** the reviews it asks for, each a task at its exact head
   with a charter's lens (memory safety, performance, a design),
   blocking or advisory. They can be amended while the change is live,
   so a coordinator may add one. A gate's result may also be posted as a
   commit status, such as `temper/review/memory-safety`, which protection
   can require. A person's approval is an optional rule, not the
   default.
5. **Wait its turn** in its landing branch's line (6.4).
6. **Update** from its base, when its head lacks the base's tip (6.5).
7. **Merge** at exactly its head, squashing by default, and
   automatically: anything stricter is a rule.

Repairs, rebases and stalls stay bounded as they are today
(engine-domain.md, 5.3).

### 6.4 Landing, and landings as news

- **Changes ready to land in a branch form a line,** by plan priority,
  then by when each became ready. Only the first is brought up to date
  and merged.
- **Every landing is news for two kinds of goal:** the goal whose change
  landed, whose step is done; and every other active goal with changes,
  open or planned, in the same repository, whose base has moved.
  Concurrent plans meeting this way is the ordinary case, not an
  exception for outside activity. A goal hears of landings through a
  subscription to the repositories it works in.
- **Not every piece of news wakes.** A goal's own step landing wakes
  nobody: what comes after it is due mechanically. Another goal's
  landing wakes a coordinator when it overlaps its work, which the
  engine judges without an LLM: the files the landed pull request
  touched against those of the goal's own open pull requests (Forgejo
  lists a pull request's files), and, for steps not produced yet, the
  paths their specs name as a hint.
- **The landing branch's own CI.** A landing temper did not make may
  break the branch. Once CI fails on its head, landings into it are held
  and one task repairs it, rather than every change repairing a failure
  that is not its own.

### 6.5 Updates: lazy and mechanical

- **Lazy:** a change takes its base when it is first in its line, or
  when it is about to get a run for another reason, so that a repair
  starts from current code. It never does only because its base moved.
- **Mechanical:** the update merges the base into the change's branch,
  with no LLM. Forgejo makes it itself (`POST R/pulls/<n>/update` with
  `style=merge`, whose handler v15 has), so it is an engine action, as a
  merge is. A merge rather than a rebase: landing squashes, so the
  branch's merge commits never reach the base, and the branch only moves
  forward.
- **A conflict** makes Forgejo refuse the update. A task for an agent
  resolves it: the worker prepares the merge in progress, the base
  merged in and conflict markers in the files; the agent edits them,
  with no git writes of its own; the worker commits the tree as the
  merge, with both parents, and pushes it.
- **A semantic conflict,** a clean update whose CI fails, is a repair,
  whose brief names the base merged in and what landed in it.
- **Reviews carry over** a clean update temper made, since the change's
  own diff is unchanged; a rule may ask for them again. After a conflict
  an LLM resolved, they are asked for again.

This replaces the rebase run a moved base starts today
(`Repair::BaseMoved`). As far as the code shows, that run cannot take
its base in: it starts from the change's own branch
(`crates/temper-engine-domain/src/runs.rs`), its agent has no git
writes, and the worker commits on top of the branch's head, with one
parent. What it achieves is a new head, which clears `base_moved`
(`crates/temper-engine-domain/src/translate.rs`) and runs CI again on a
head that still lacks the base; and a run with nothing to change fails
to push.

### 6.6 A branch per goal

Whether a goal's changes land into a branch of its own, which then lands
whole, or each into the shared base, is the plan's choice like any other
(`ChangeSpec.base` names the branch today). Landing each into the base
is the usual answer: plans meet early and in small pieces; a goal that
needs another's work has it once that lands; and repositories that pin
each other's commits (temper locks skein's in `Cargo.lock`) are not
re-pinned after a squash. A goal's own branch suits work likely to be
dropped; a goal checked whole before it lands (in an environment, by an
end-to-end run, by a review of the whole); intermediate states that
would break the base; and upstream contributions (6.8). Such a branch
stays short-lived, and takes the base's landings that touch its area as
they come. A rule may require or forbid it.

### 6.7 Issues

Issues stay, for what they show people reading the forge and for the
history they keep if temper is unplugged:

- **One per goal,** not one per task. Its body is the goal and the plan
  as a task list, which the forge renders with its progress, rewritten
  by temper as the plan changes. A goal across repositories keeps its
  issue in its project's home repository.
- **Milestones as comments:** the design agreed in its session (a
  summary, not the transcript), the plan accepted, each revision and its
  reason, each change landed or held, each report worth keeping ("why
  not library X"), the goal finished.
- **Pull requests refer to it** ("Part of ai/temper#12"), so the forge
  links each change to its goal, across repositories too.
- **Findings as issues:** a bug found while exploring, a follow-up a
  review raised, a flaky test. Their state (triage, priority, the goal
  that takes one up) is the store's.

All of them are projections (2.1). Issues others open in a plugged-in
repository are participating objects, which may become intake later.

### 6.8 Upstream contributions

Not needed first, and kept possible:

- **A fork,** on the upstream's forge where it can be, so that upstream's
  CI runs on it unchanged and the pull request is native there. Its main
  follows upstream's, fast-forward only, and temper's own changes land
  into a work branch in the fork: a goal's own branch (6.6).
- **The submission is a task:** brought up to date with upstream, its
  commits shaped to upstream's conventions, described and signed off,
  and accepted by a person before anything is posted, often under that
  person's own identity.
- **Once posted,** its pull request is a participating object (2.2): a
  task reads the maintainers' reviews, updates the branch, and drafts
  replies a person approves.

## 7. Worked examples

### 7.1 A goal, as tasks

The plan of engine-domain.md, 5.2, as a person might reach it today:

```
P, a person
└─ T0  a chat with P about temper          agent, a session's charter
   └─ T1  add OAuth login                  agent, a coordinator's charter;
      │                                    T0 proposed it, P accepted
      ├─ T2  spike: library X              agent, read-only: a report
      ├─ T3  spike: a hand-rolled flow     agent, read-only: a report
      ├─ T4  choose       after T2, T3     person P: a choice
      ├─ T5  design       after T4         the forge's change, a design review
      ├─ T6…T9  changes   after T5         the forge's change, made by T1
      │                                    once it read T5's result
      │     T7 amended while live: a memory-safety review as a gate
      └─ T10 explore      after T6…T9      agent, with a test environment
```

- **T1 is a proposal.** Its authority (a budget, landing into main) is
  beyond T0's, so P accepts it. Within T1's authority, its own
  delegations need nobody's acceptance.
- **Results flow along dependencies:** T4's choice reaches T5's brief.
- **T6…T9 are a revision,** made by T1 once T5 landed, within its
  authority.
- **The memory-safety review is one amendment:** T1 adds a gate to T7.
  The change procedure runs a review task at each new head of T7, and
  holds the merge until it passes.
- **Under each change,** the procedure makes its own tasks: producing,
  repairs, reviews, resolving conflicts.
- **Had P asked for a small fix,** T0 would have made one change task,
  and there would be no T1.

### 7.2 Landing, step by step

One repository, temper, with main at M0, and two goals in flight:

- **goal A:** change A1 is ready to land (CI green on its head a1,
  reviewed), and A has steps still to come;
- **goal B:** change B1 is ready to land (CI green on b1, reviewed), and
  change B2 is being produced by a run that started from M0.

A1 and B2 both change `limits.rs`; nothing else overlaps.

| When | Who | Does | Afterwards |
|---|---|---|---|
| t0 | Engine | A1 is first in main's line, and its head holds M0: merges A1 at exactly a1, through Forgejo | main is M1 |
| t0 | Engine | news for A (its own step landed) and B (A1 landed in temper, overlapping neither B1 nor anything B has pushed): neither wakes | B1 and B2 untouched |
| t1 | Engine | B1 is first, and its head lacks M1: asks Forgejo to update it | |
| t1 | Forgejo | merges M1 into B1's branch, cleanly | B1's head is b1′ |
| t1–t2 | CI | passes on b1′ | |
| t2 | Engine | B1's review carries over the clean update; merges B1 at exactly b1′ | main is M2 |
| t2 | Engine | news for B (its own step) and A (B1 overlaps nothing of A's): neither wakes | |
| t3 | Agent, worker | B2's run finishes; the worker pushes it | B2's head is b2, built on M0 |
| t3 | Engine, CI, agent | opens B2's pull request; CI passes on b2; a review task approves it | B2 is ready |
| t4 | Engine | B2 is first, and its head lacks M2: asks Forgejo to update it | |
| t4 | Forgejo | refuses: B2 and A1 both changed `limits.rs` | a conflict |
| t4 | Engine | news for B (the conflict, which does not wake it while a task resolves it); starts that task | |
| t4 | Worker, agent | the worker prepares the merge in progress; the agent resolves `limits.rs` and runs the checks; the worker commits the merge, with both parents, and pushes it | B2's head is b2′ |
| t5 | CI, agent | CI passes on b2′; a review, asked again since an LLM resolved a conflict, approves it | |
| t5 | Engine | merges B2 at exactly b2′ | main is M3 |
| t5 | Engine | news for B (its last step: it wakes) and A (no overlap: it does not) | |
| t5 | Coordinator B | its one run: reads its inbox (A1 and B1 landed, the conflict resolved) and writes goal B's closing report on its issue | goal B is done |

Today's design does more for less: at t0 the plan starts an LLM run on
B1 for its moved base, though nothing conflicts, and B2 gets one too
when B1 lands (6.5); and each coordinator wakes on news it needs no
judgement for.

## 8. What carries over, and what goes

### 8.1 Domain by domain

- **The engine's top level** (`temper-engine-domain`) changes most:
  records composed into comments and split from them, and the restart
  contract built on them, become the store's tasks and outbox.
- **work,** the hub, becomes the task lifecycle, rebuilt from the store
  rather than from records.
- **plan** splits. Dependencies, batches and their checks become the
  core's scheduling; the change's way to landing becomes the forge's
  change procedure (6.3), with the updates of 6.5. What an outcome may
  do grows from adding steps to amending and cancelling them.
- **rules** become authority's checks (3.4): acceptance, the envelope and
  grants become its proposals and its narrowing.
- **forge** keeps the working set of what the forge owns of live work,
  keyed writes, keeping up, webhooks and the request budget. It loses
  records' locations, nonces, mangled records, the wiki, the hand-in
  label, and the tracking label as the index of live work. Its comment
  inbox stays, for participating objects.
- **brief** becomes how an agent's context is built, its sections given
  by the core (the task, its inbox, its dependencies' results, notes)
  and by connectors (a pull request, CI's output).
- **notes** stay, their entries moving from the wiki to the store. A
  note that matures may be proposed as a change to a repository's
  `AGENTS.md`.
- **fleet, views and accounts** stay as they are.
- **The worker** stays. It prepares the resources connectors name, and
  gains a merge in progress as a place to start (6.5).
- **The agent** stays. Its outlets become the tools of 4.5, and its
  long-lived channel (inbound events, parking) matters more.

### 8.2 What it would replace in the documents

- **engine-domain.md:** section 1's "the forge is the truth, and the
  engine keeps only caches" and "everything is an item"; records in
  comments and labels (4.1); the inbox derived from comments (4.3); the
  hand-in label (4.6); plans as typed graphs of fixed primitives within
  an envelope (section 5); sessions as items (section 6); notes in the
  wiki (section 10); the working set of records and the label as its
  index (section 12); the store as a cache (section 13).
- **forge.md:** the engine's blocks inside comments and wiki pages
  (section 4), the wiki's operations, and reading comments for owned
  work.
- **protocol-implementation.md:** of the paused forge increment, the
  typed record blocks (`forge_blocks.rs`) and the wiki are not to be
  built. Forgejo's documents and webhooks, and the calls for pull
  requests, reviews, statuses, branches, issues and keyed creations,
  stay.
- **agent-domain.md and worker-domain.md** change little (8.1).

## 9. Open questions

- **Agents as identities** that serve many tasks, such as a project's
  maintainer. The model allows them; a long-lived task serves meanwhile.
- **Procedures written by agents,** deterministic workflows like a
  coding agent's scripts. Until they are wanted, procedures are Rust,
  owned by connectors or the core; dependencies stay "after it is done",
  and conditions are a coordinator's judgement.
- **Wake policies:** what wakes a task, whether overlap by files is
  enough, and whether a cheap model should triage news before a
  coordinator wakes. Tuned from use.
- **A person's approval as a rule:** whether temper alone enforces it,
  or posts the approval as the person, with the forge credential they
  signed in with, so that protection requires it independently
  (engine-domain.md, section 15, people on the forge).
- **Identity and roles:** people sign in through the forge; whether
  roles (who accepts, who steers) are temper's own per project, or read
  from the repositories' permissions.
- **Contributions in and out** (2.2, 6.8): neither needed first.
- **A line's throughput:** landings into one branch wait on CI one after
  another. If that binds, landings are batched: tested together, and
  bisected when they fail.
- **Conflicts found before either side lands:** temper sees every open
  change, and could try them against each other and warn both goals.
- **Names:** today's item, session and run, against task, agent and run.
- **Limits:** the depth of delegation, tasks per tree, an inbox's size,
  subscriptions per task, as the programming model asks of every entity.
- **Facts to check** on Forgejo (forge.md, section 13): the update's
  answer to a conflict, whether its push starts CI, and listing a pull
  request's files.
