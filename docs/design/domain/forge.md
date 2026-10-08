# The forge connector

Provisional, 2026-10-04. How temper drives a forge, Forgejo first: the
forge as a connector (connectors.md), with its resources, effects,
topics, procedures and tools; adopting repositories; keeping up; a
change's way to landing; landing queues; updates and conflicts; issues;
and the ways temper takes part in what others own. Its protocol layer
is `docs/design/forge.md`, which section 17 says what this design
changes in. What is still open is listed in section 20.

## 1. In one page

- **What the forge holds:** git; pull requests as the unit of a change;
  CI; branch protection, as a second net; and a record people can read.
  With no people writing there, almost nothing comes back from it but CI
  and drift: no comment inbox for owned work, no hand-in label, no
  records, no wiki.
- **Live work comes from the store.** The connector holds a working set
  of what the forge owns of the resources live tasks name (pull
  requests' heads and states, CI on those heads, landing branches'
  tips), kept up by listing what changed, sooner on a webhook's hint.
  No label marks what temper tracks.
- **The change procedure** takes a change from produced to landed:
  produce, open, CI, gates, its turn in its landing branch's queue, an
  update from its base when it lacks the base's tip, and a merge at
  exactly its head. Agents are woken for what needs judgement: producing,
  repairs, reviews, conflicts.
- **Landing queues.** Changes ready to land in a branch wait in line, by
  their goal's priority then by when each became ready; only the first
  is brought up to date and merged.
- **Updates are lazy and mechanical.** A change takes its base when it
  is first in line, or about to get a run, through the forge's own
  update; an LLM only resolves a conflict, in a workspace where the
  merge is in progress.
- **Every landing is news** for every goal working in that repository;
  it wakes a goal's coordinator only when it overlaps the files of that
  goal's changes.
- **Issues stay, one per goal,** as a projection people can read: the
  goal, its plan as a task list, and its milestones. Pull requests refer
  to it.
- **Plugged in as it is.** temper adopts repositories with whatever
  access it is given; webhooks, labels, protection and CI are all
  optional.

## 2. What it gives

- **Resources,** named `forge` `<host>` `<owner>` `<repo>`, then:
  - `branch` and the branch's own segments: `… branch temper 7 c42`;
  - `pull` and a number; `issue` and a number;
  - nothing beyond, for the repository itself.
- **Effects:** open, edit, update, merge, close and reopen a pull
  request; post a review, a comment or a commit status; create, edit,
  close and reopen an issue; set reviewers; create and delete a branch
  (section 6). A run's push is the worker's, over git, under its task's
  write hold (worker.md, section 5).
- **Kinds,** for authority (authority.md, section 4): `read`, `push`,
  `open` (a pull request from a branch), `land` (into a branch), `review`,
  `status`, `comment`, `issue`, `branch` (create and delete); each on a
  prefix. `land` implies no other; `push` implies `branch` on what it
  covers.
- **Topics:** landings on a branch; CI on a head; a pull request's
  state; the comments and reviews on a participating object (section 7).
- **Procedures:** the change (section 8).
- **Tools,** reads an agent may call: a pull request, its files and its
  diff; CI on a head, and a failed job's output; an issue and its
  comments; the landings on a branch since a commit. Each answer is cut
  to its limit.
- **Brief sections:** the pull request against its base; CI's failures
  on its head, with their output; reviews' remarks; a conflict, with what
  landed in the base since.

## 3. Structure

```
temper-engine-domain-forge              the connector's top: resources, write holds, topics, adoption, routing
├── temper-engine-domain-forge-client   the working set, keeping up, fresh reads, the outbox's writes, the request budget
├── temper-engine-domain-forge-change   the change procedure and the landing queues: decisions over facts
└── temper-engine-domain-forge-issues   the goals' issues: what each projects, when to write it
```

- **The top** keeps the forge's resources as the project knows them
  (roles, what temper may do there), the write holds on branches, the topics
  and their subscribers, the changes' procedure states, and the issues'
  projection states. It routes between its children and translates to
  and from the root.
- **`client` is a capability,** today's forge child domain without
  records, the wiki or labels as an index: it knows the forge's API, the
  working set, the request budget and how a write is made once.
- **`change` and `issues` are policy:** given a change's state and the
  facts, or a goal's state, they say what is due, as `plan` does today.
  They keep nothing between calls; their states are the top's, committed
  in the store.

## 4. Adopting repositories

- **A repository joins a project with a role** (core.md, 5.1): owned or
  adopted, where temper pushes its branches, opens pull requests and
  lands; a fork of an upstream (section 14); or context, read only. A
  person whose role allows it adopts it in the web (people.md,
  section 5). A repository temper writes belongs to one project; others
  may adopt it as context.
- **What temper may do is read at adoption:** its user's permission on
  the repository; the branch protection that applies to temper's prefix
  and to each landing branch; whether temper may merge there. The
  project's ceiling follows (authority.md, section 6): read access is
  enough for context; write access lets temper push its branches and open
  pull requests; protection that keeps temper from merging makes landing
  a wait for someone else's merge (section 13).
  Forgejo 15 refuses protection reads to a write collaborator (section
  20): adoption records protection as unknown, never as absent. A refused
  merge narrows what temper may do, as connectors.md, section 11 says.
- **The project names, per repository:** its landing branches; temper's
  branch prefix, `temper/` by default, which protection can reserve to
  temper's user; the merge style, the repository's own by default,
  squash where it has none; and how changes are checked when the
  repository has no CI.
- **Nothing is required:**
  - no webhook: polling is the backstop, webhooks only cut the latency,
    and the web shows a repository whose hints never arrive;
  - no label: temper writes labels for display only, if the project asks,
    and reads none;
  - no protection: temper's rules are its enforcement, protection a second
    net;
  - no CI: a repository without it gates its changes on checks its
    project configures (section 8.3).
- **Provisioning** (creating a repository, its webhook, its protection)
  is an optional set of effects a person may ask for (connectors.md,
  section 11).

## 5. What it holds, and keeping up

### 5.1 The working set

For the live resources (connectors.md, 3.4), and nothing else:

- **per repository:** the forge's time of the last listing, the
  landing branches' tips, and the CI on each tip;
- **per pull request temper owns that a live task names:** its head and
  base, its state (open, closed, merged), its mergeability, CI on its
  head (each context's latest status, and the combined state), and,
  when a decision wants them, its files at its head;
- **per branch a live task holds:** its tip as temper last pushed or
  updated it, to tell drift from temper's own moves;
- **per participating object:** the position its comments and reviews
  have been read to, and those not yet given to its task (section 13);
- **per issue of a live goal:** its number, and what temper last wrote
  into it, by a digest, so an unchanged projection is not written again.

A resource enters when a live task names it, and leaves when none does.
Its capacity is a limit: a batch naming resources past it is refused.

### 5.2 Keeping up

- **A listing per repository** of what changed since the last, by the
  forge's time, inclusive, least recently updated first, paged; its cost
  follows the rate of change, not the number of pull requests held.
- **Per pull request,** what moves no updated time on Forgejo (CI
  reporting, its base moving) is read again on a backoff that doubles up
  to a ceiling, and at once on a hint naming its head or its base.
- **Landing branches' tips** are read on a hint naming them, and on a
  slow pace otherwise.
- **Hints** from webhooks name the repository and what changed (an item,
  a branch, a commit) and who caused it; temper's own are echoes,
  dropped.
- **A slow pass** reads again, at its own pace, every live resource the
  listings have not shown for a while, so that nothing waits on a hint
  that never came.
- **The forge's clock** for every time a listing compares.

### 5.3 Fresh reads

Before a decision depends on a fact, the change child domain asks for it
afresh: the head before a merge, the base's tip before an update, CI on
the exact head, the files of the pull requests an overlap compares. A
fresh read goes first in the request budget (5.4).

### 5.4 The request budget

Fresh reads first, a write's own checks and finds among them; then
writes; then keeping up; then the slow pass, with a share of each window
kept for the last two. Each call is charged what it cost the forge, its
requests counted. A refusal for the rate stops every call until its
reset.

## 6. Effects

Every effect is committed into the outbox and made by `client`, in
commit order per lane (a pull request, an issue, a repository) and one
at a time on each (connectors.md, section 4):

| Effect | What it is | Recovery class (jig's `connectors.md`, 4.3) | Found again after an uncertain failure |
|---|---|---|---|
| open a pull request | a creation: from a branch into its landing branch, with a title, a body and a key | conditional: only while no pull request for its branches is open; Forgejo keeps it, closed or not | by its branches: the newest pull request for them, open or not |
| edit a pull request's title or body | a set | idempotent | written again |
| update a pull request from its base | a transition: the base's tip merged into its branch, by the forge | unrecoverable | its head contains the base tip it asked for |
| merge a pull request | a transition, conditional on its exact head | conditional | merged, and at which commit |
| close, reopen a pull request | a set | idempotent | written again |
| post a review | a creation, with a key | unrecoverable | by its key, among the reviews after the entry's start |
| post a comment | a creation, with a key | unrecoverable | by its key, among the comments after the entry's start |
| post a commit status | a set: a context on a commit | idempotent | written again |
| create an issue | a creation, with a key | unrecoverable | by its key, among the issues changed since the entry's start |
| edit, close, reopen an issue | a set | idempotent | written again |
| set a pull request's reviewers | a set | idempotent | written again |
| create a branch at a commit | a creation, named | unrecoverable: another hand can delete it, and nothing then shows it was made | the branch exists at that commit |
| delete a branch | a set: temper's own branches only | idempotent | it is gone |

- **Classes follow what Forgejo offers.** It keeps no key it would refuse
  a second creation by, so nothing temper makes is keyed. A pull request
  is opened only while none is open for its branches, and Forgejo keeps
  every pull request, closed or not, so the state it leads to is always
  found: a first another hand closed is found closed, which is drift
  (jig's `connectors.md`, section 11). A branch is created only while
  none of its name exists, but another hand can delete it, and then
  nothing shows it was made: its creation is unrecoverable. Every class
  rests on a copy landing within its write's lifetime, or never (jig's
  `connectors.md`, 4.3).
- **An unrecoverable creation is looked for first:** an uncertain review,
  comment, issue, update or branch found by its key, or by the state it
  leads to, is made; one not found by its deadline holds its task for a
  person, and is never sent again by temper on its own.
- **Keys are carried** in what temper creates: a marker at the head of a
  body, a comment or a review, hidden where the forge renders it
  (`docs/design/forge.md`, section 4). A pull request's key is its
  branches, which temper alone names.
- **An entry's start** is the forge's time and the last item seen when
  it first went out, committed with it, so a find reads only what came
  after.
- **Conditions travel with the effect:** a merge names the head its
  decision saw, which the forge checks, and a stale head is refused, not
  merged. It names the base too: the pull request is read afresh first,
  and one retargeted since the decision is not merged.
- **Failures:** a merge refused for a conflict, an update refused for a
  conflict, a pull request with nothing to merge, a protected branch and
  a forbidden write are each typed, and go to the procedure that asked,
  never retried blind.

## 7. Topics, and landings as news

### 7.1 Topics

| Topic | Published when | Read by |
|---|---|---|
| landings on a repository's branch | a landing branch's tip moves | goals working in that repository; landing queues |
| CI on a head | a head's combined state changes | changes; tasks that subscribe |
| a pull request's state | opened, closed, merged, its head moved, its mergeability known | changes; tasks that subscribe |
| comments and reviews on a participating object | another party writes there | the task dealing with it (section 13) |

The change child domain reads its facts directly and needs no
subscription; topics are for tasks whose executors are agents, and for
goals.

### 7.2 Every landing is news

- **For two kinds of goal:** the goal whose change landed, whose step is
  done; and every other active goal with changes open or planned in the
  same repository, whose base has moved. Concurrent plans meeting this
  way is the ordinary case, not an exception for outside activity.
- **A goal hears of landings** through a subscription the engine keeps
  for it, to the landing branches its subtree's changes target, made as
  the first change into each is created and kept until the goal ends.
- **A landing temper did not make** is news like any other: what landed
  is read, a pull request or a range of commits.

### 7.3 Which landings wake

Not every piece of news wakes. The connector classifies each landing for
each subscribed goal (connectors.md, 5.3), without an LLM:

- **its own change landed:** kept; what comes after it is due
  mechanically, through the goal's dependencies;
- **another landing overlaps its work:** wakes. Overlap is the files the
  landing touched against those of the goal's own open pull requests at
  their heads, and, for changes not produced yet, the paths their specs
  name, as a hint;
- **no overlap:** kept, for its next run to read;
- **overlap unknown** (a landing too large to list, a read that failed):
  wakes, since a wasted turn is cheaper than a missed conflict.

The files are read from the forge (a pull request's files, or the
comparison of the branch's old and new tips), cached per head and cut at
a limit past which the overlap is unknown.
Forgejo 15 pages pull-request files, but its comparison ignores paging
(section 20). The protocol bounds the comparison's bytes, files and
commits before domain allocation; reaching a bound, an unusable response
or uncertain completeness makes overlap unknown and wakes every affected
subscriber. It never fetches another comparison page as a continuation.
Pull-request pages are accepted only while fresh reads bracket them with
the same head; a head change invalidates that listing.

## 8. The change procedure

### 8.1 What a change is

A change task asks for one change to one repository, landed into one
branch. Its parameters:

- **the repository and the landing branch;**
- **the branch,** held by the task: one it is handed, already pushed
  (a chat's fix, core.md, 6.3), or its own, named after its tree
  (section 11), which a producing task pushes;
- **what to produce,** when nothing is pushed yet: the spec and charter
  of its producing task;
- **its gates** (8.3);
- **the pull request's title and body,** or what to make them from, and
  the goal it is part of;
- **its bounds:** repairs, resolutions, and the stall of each wait.

A goal across repositories is several change tasks, ordered by their
dependencies: skein's change lands before temper's that bumps its lock.

### 8.2 Its way to landing

```
producing ─► opening ─► checking ─► gating ─► queued ─► first ─► landing ─► landed
                          ▲   │        │                  │
                          │   └────────┴──► repairing ────┤
                          │                               ├─ lacks the base's tip ─► updating
                          └───────────────────────────────┤                          │
                                                          └──── a conflict ◄─────────┘
                                                                      │
                                                                  resolving
```

1. **Producing:** a task for an agent, with the charter the change
   names, whose workspace starts from its landing branch (or its saved
   work) and pushes the change's branch (agent.md, section 6). Its result is
   the head it pushed. An attempt whose answer was lost may have pushed
   all the same: its writer slot is freed only once the branch has been
   read afresh (connectors.md, 3.3), and a head found there then is the
   change's own (the branch is held by this change alone), so the
   change cancels its producing task, whose work is done, and goes on to
   open it, in the same commit that frees the slot, before any retry
   could claim it. A change handed a pushed branch starts at the next
   step.
2. **Opening:** its pull request into its landing branch, keyed by its
   branches, its body naming its goal's issue ("Part of
   `ai/temper#12`").
3. **Checking:** CI runs on its head. A failure is a repair (8.4): a task
   given the failing output. A repository without CI is checked by its
   project's checks (8.3).
4. **Gating:** each gate without a verdict valid at this head runs: a
   task at the exact head. A blocking gate that asks for changes is a
   repair, given its remarks.
5. **Queued:** ready (CI passed and every blocking gate holds, at its
   head) and waiting its turn in its landing branch's queue (section 9).
6. **First:** its turn, which it keeps through what follows unless it
   leaves the queue (section 9). If its head lacks the base's tip, it is
   **updated** (section 10): cleanly, and its new head is checked again,
   its gates carried over; or in conflict, and it leaves the queue to be
   **resolved**, its new head checked again and its gates asked again.
7. **Landing:** merged at exactly its head, in the repository's merge
   style, squash by default, automatically: anything stricter is a rule
   (authority.md, section 10). Merged, the change is **landed**: its
   result is the landing (the pull request, the merge commit), and it
   closes, its branch deleted as its write hold is released
   (section 11).

Lazy, too, for runs: before a repair or a resolution is created, the
change takes its base if its head lacks the base's tip, so a repair
starts from current code. It never updates only because its base moved.

### 8.3 Gates

- **A gate** is a verdict a change asks for before it lands, at its exact
  head:
  - *a review:* a task for an agent with a lens's charter (memory safety,
    performance, a design), whose verdict is approve or request changes,
    with remarks;
  - *a person's approval,* by a role in the project, through a person
    task in the web (people.md, section 8);
  - *checks:* the project's checks for a repository without CI, or
    another connector's verdict, later (connectors.md, section 14).
- **Blocking or advisory.** A blocking gate's verdict holds the landing;
  an advisory one's is read, and reported, and holds nothing.
- **Asked for by the change, the project and the plan.** A change's own
  gates are its parameters, amended while it is live (a coordinator
  adds a memory-safety review to one change); the project's landing
  rules add theirs to every change into a branch (authority.md,
  section 10). A gate added while a change is queued takes it out of the
  queue until it has its verdict.
- **Valid at a head, and carried over clean updates.** A verdict is given
  at a head. A review's stays valid at every head reached from it by
  updates temper made that merged cleanly, since the change's own diff is
  unchanged; so does a person's approval, unless the project's rule asks
  for the exact head. Any other new head (a repair, a resolved conflict)
  asks for every gate again. The change keeps, in its state, the heads
  its clean updates led through, which is the fact a requirement reads
  (authority.md, section 10).
- **Posted, if the project wants:** a gate's verdict as a commit status,
  such as `temper/review/memory-safety`, which protection can require
  independently of temper.
- **When they run:** once CI passes on the head, by default, so a head
  CI fails costs no review; the change may ask to run them at once.

### 8.4 Repairs, resolutions and stalls

- **A repair** is a task for an agent whose workspace starts from the
  change's branch, its brief carrying why: CI's failing output on the
  head; a gate's remarks; or, for a clean update whose CI then fails (a
  semantic conflict), the base merged in and what landed in it. It pushes
  a new head, and the change is checked again.
  The target is Forgejo v16.0.5. The client reads the failed job's
  plaintext log through the API, bounded by the repair brief's byte budget
  (section 20). It pins the job attempt and reports truncation; a failed
  read is recorded explicitly alongside the status description and link.
- **A resolution** is a task for an agent whose workspace starts from a
  merge in progress: the base merged into the branch, the conflicting
  files marked. The agent edits them and runs the checks, with no git
  writes of its own; the worker commits the tree as the merge, with both
  parents, and pushes it (worker.md, 4.1).
- **Bounded apart:** repairs past their limit, resolutions past theirs,
  hold the change, saying why, and its requester hears.
- **Stalls:** each wait on the forge (CI that never reports, a
  mergeability never known, a gate's task stuck) is bounded by a stall
  counted from the head's push or the last release; past it, the change
  is held.
- **A release** lifts what it releases: repairs and resolutions count
  again from it, and the change decides afresh.

### 8.5 Drift

While its branch's writer slot is taken (connectors.md, 3.3), by a run
of one of its delegates, by a run that handed it the branch and has not
yet answered, or by an update it asked for, the change decides nothing
on its branch's head: forward moves by temper's identity are taken as
that writer's (connectors.md, section 10), and the change reads the head
afresh once the slot is freed. Otherwise:

- **Its pull request closed** by someone else: held, saying who and
  when. A release reopens it; a cancel closes the change.
- **Its branch moved** while nothing of temper's writes it, or by another
  identity, or not forward: held, the head kept as it is. A release
  takes the new head as the change's and checks it, gates asked again.
- **Its branch deleted:** held. A release creates the branch again at
  the head temper last knew, which the forge keeps under the pull
  request, and reopens the pull request.
- **Its pull request retargeted** to another base: held. A release
  retargets it back, or takes the new base as its landing branch if its
  requester amends the change so.
- **Merged by someone else:** landed, like any other landing.

Nothing is written over: a held change makes no effect until it is
released.

### 8.6 Cancelled

A cancelled change closes as every task does (tasks.md, 5.1): its live
delegates close first, their runs ended; an entry already sent, such as
a merge, settles, and what landed counts; then its write hold is
released: its pull request closed with a comment saying why, and its
branch deleted, its commits still readable under the closed pull
request. A change whose merge landed meanwhile ends landed, not
cancelled.

## 9. Landing queues

One per landing branch that has changes ready. A repository belongs to
one project (another may read it as context), so its goals' priorities
are comparable.

- **In the queue:** a change is in the queue once it is ready (CI passed
  and every blocking gate holds, at its head). It keeps the time it
  first became ready while it stays live, so leaving and coming back
  does not send it to the back.
- **Ordered by priority,** its goal's (core.md, 5.2), a change outside
  any goal taking its project's default, **then by that time.**
  Priorities move with the goals'. **Aging bounds starvation:** a change
  ready for longer than the project's window goes before every change
  that became ready after it, whatever their priorities.
- **The first owns the queue.** Only the first is brought up to date and
  merged; the rest wait. It keeps its turn through a clean update, CI on
  its new head and its merge, so no CI run is wasted by a change
  overtaking it. It **leaves the queue** on a conflict, on a check that
  fails (CI, a gate asked again), on a gate added, or when held or
  closing, and the next becomes first; it comes back when it is ready
  again, at its place by its first time. A resolution never holds the
  queue.
- **Derived, not kept.** A queue is the ready changes into a branch,
  their priorities and their times; nothing of it is stored beyond the
  changes' own states, so a restart rebuilds it from them.
- **The branch's own CI.** A landing temper did not make may break the
  branch. Once CI fails on the branch's tip, the queue pauses and one
  repair is made: a change task into that branch, first in its queue,
  requested by the deployment on the project's behalf with the
  authority its policy gives such repairs (tasks.md, section 3), and news
  for every goal with changes waiting there. Every change in the queue
  waits for it rather than repairing a failure that is not its own. The
  pause ends when CI passes on the tip; repairs are made one at a time,
  up to a bound, past which the queue is held for a person, who may also
  release it as it is (a flaky check).
- **Where temper may not merge,** the queue still orders what is ready,
  for people to see; each change's landing waits for someone else's
  merge (section 13).

## 10. Updates: lazy and mechanical

- **Lazy:** a change takes its base when it is first in its queue, or
  about to get a run for another reason (8.2). It never does only
  because its base moved.
- **Mechanical:** the update merges the base into the change's branch,
  with no LLM. Forgejo makes it itself (its pull request update, with
  the merge style), so it is an effect, as a merge is. A merge rather
  than a rebase: landing squashes, so the branch's merge commits never
  reach the base, and the branch only moves forward.
- **A conflict** makes the forge refuse the update, and the change is
  resolved (8.4).
- **A semantic conflict,** a clean update whose CI fails while CI passes
  on the base's tip, is a repair, whose brief names the base merged in
  and what landed in it. While CI fails on the base's tip, the failure is
  not the change's: its queue is paused (section 9), and the change
  waits for the branch's repair instead.
- **Reviews carry over** a clean update temper made (8.3); after a
  conflict an LLM resolved, they are asked for again.

## 11. Branches

- **Named after the tree.** Every branch temper makes is under the
  project's prefix (`temper/` by default, one per deployment on a
  repository, connectors.md, section 11) and its tree's root task:
  - a change's own branch: `temper/<root>/c<task>`;
  - a task's saved work: `temper/<root>/s<task>`, never the change's
    branch, so it starts no CI and moves no pull request;
  - a run's own branch, for a task that pushes outside a change, as a
    chat making a small fix does (core.md, 6.3):
    `temper/<root>/r<task>-<run>`, which its task holds from that run's
    claim and hands to the change task that lands it.

  They are siblings under `temper/<root>/`, so no name is the prefix of
  another as git refs forbid, and one grant covers a tree's branches;
  a task's own branches are named in its batch symbolically and resolved
  once it has its number (authority.md, section 4).
- **Held by the change** that lands it, and written by its delegates one
  at a time (connectors.md, 3.3).
- **Made by temper only as an effect.** A landing branch must exist when
  it is adopted or named; a goal's own branch (11.1) is created at its
  base's tip by the change that lands it, as an effect. The worker never
  creates one (worker.md, section 5).
- **Deleted as their tasks close,** as the release of their write holds:
  a landed change's branch, a cancelled one's, and a task's saved work. A
  failed change's branch is kept for a person to read, and deleted as
  its tree's root closes.

### 11.1 A branch per goal

Whether a goal's changes land into a branch of its own, which then lands
whole, or each into the shared base, is the plan's choice like any
other: a change's landing branch is its parameter. Landing each into the
base is the usual answer: plans meet early and in small pieces; a goal
that needs another's work has it once that lands; and repositories that
pin each other's commits (temper locks skein's in `Cargo.lock`) are not
pinned again after a squash. A goal's own branch suits work likely to
be dropped; a goal checked whole before it lands (in an environment, by
an end-to-end run, by a review of the whole); intermediate states that
would break the base; and upstream contributions (section 14). Such a
branch is short-lived: it is a change of its own, landed by the change
procedure like any other, and it takes the base's landings that touch
its area as they come, by an update. A project's rule may require or
forbid it.

## 12. Issues

Issues stay, for what they show people reading the forge and for the
history they keep if temper is unplugged. They are projections
(core.md, 7.1), written and never read back:

- **One per goal,** in its project's home repository, created when the
  task becomes tracked. Its body is the goal and its plan as a task
  list, which the forge renders with its progress, rewritten as the plan
  changes, at most once per interval and only when what it projects
  changed.
- **Milestones as comments,** each once: the plan accepted; each revision
  and its reason, from the tree's history; each change landed or held;
  each report the goal's agents mark as worth keeping ("why not library
  X"), and the design agreed, in their words; the goal finished, after
  which the issue is closed.
- **Pull requests refer to it** ("Part of `ai/temper#12`"), so the forge
  links each change to its goal, across repositories too.
- **Findings as issues:** a bug found while exploring, a follow-up a
  review raised, a flaky test, created by an agent's `effect` tool
  (connectors.md, 4.5), keyed. Their state (open, taken up by a goal,
  dropped) is the store's, shown in the web; taking one up is a goal
  that names it.
- **A projection, not a procedure** (connectors.md, section 7). The
  `issues` child domain is fed each goal's state, its plan and its
  milestones by the root, as the engine keeps a goal's landing
  subscription for it (core.md, 5.2): it is part of tracking, not work
  anyone asked for, so it has no task of its own. Its effects are keyed
  by the goal and what they write, made with the authority the project's
  policy gives its goals' issues, in its home repository; the last ones,
  the closing milestone and closing the issue, are made as the goal
  closes.
- **Issues others open** in an adopted repository are participating
  objects, which may become intake later.

## 13. Participating objects

Not needed first, and kept possible with one mechanism (core.md, 7.2):

- **Outside contributions** to a project temper works on: their issues
  and pull requests, which a task may triage and review. A task given
  one subscribes to its comments and reviews; what others write reaches
  its inbox as news, in order, from a position the connector keeps.
- **A repository where temper may not merge:** a change there is ready
  when its CI and gates hold, and then waits for someone else's merge,
  which is a landing like any other. People's reviews on its pull request
  are news for the change; a request for changes is a repair, given its
  remarks.
- **What temper writes** there (a reply, a review) is a participant's:
  drafted by an agent's task, and, where the project's policy says so,
  proposed to a person before it is posted.

## 14. Upstream contributions

Not needed first, and kept possible:

- **A fork,** on the upstream's forge where it can be, so that upstream's
  CI runs on it unchanged and the pull request is native there. Its main
  follows upstream's, fast-forward only, and temper's own changes land
  into a work branch in the fork: a goal's own branch (11.1).
- **The submission is a task:** brought up to date with upstream, its
  commits shaped to upstream's conventions, described and signed off,
  and accepted by a person before anything is posted, often under that
  person's own identity.
- **Once posted,** its pull request is a participating object
  (section 13): a task reads the maintainers' reviews, updates the
  branch, and drafts replies a person approves.

## 15. Landing, step by step

One repository, temper, with main at M0, and two goals in flight:

- **goal A:** change A1 is ready to land (CI green on its head a1,
  reviewed), and A has steps still to come;
- **goal B:** change B1 is ready to land (CI green on b1, reviewed), and
  change B2 is being produced by a run that started from M0.

A1 and B2 both change `limits.rs`; nothing else overlaps. A has the
higher priority.

| When | Who | Does | Afterwards |
|---|---|---|---|
| t0 | Engine | A1 is first in main's queue, and its head holds M0: merges A1 at exactly a1, through Forgejo | main is M1 |
| t0 | Engine | news for A (its own step landed) and B (A1 landed in temper, overlapping neither B1 nor anything B has pushed): neither wakes | B1 and B2 untouched |
| t1 | Engine | B1 is first, and its head lacks M1: asks Forgejo to update it | |
| t1 | Forgejo | merges M1 into B1's branch, cleanly | B1's head is b1′ |
| t1–t2 | CI | passes on b1′ | |
| t2 | Engine | B1's review carries over the clean update; merges B1 at exactly b1′ | main is M2 |
| t2 | Engine | news for B (its own step) and A (B1 overlaps nothing of A's): neither wakes | |
| t3 | Agent, worker | B2's producing run finishes; the worker pushes it | B2's head is b2, built on M0 |
| t3 | Engine, CI, agent | opens B2's pull request; CI passes on b2; a review task approves it | B2 is ready |
| t4 | Engine | B2 is first, and its head lacks M2: asks Forgejo to update it | |
| t4 | Forgejo | refuses: B2 and A1 both changed `limits.rs` | a conflict |
| t4 | Engine | B2 is resolving: a task for an agent; news for B, which does not wake it while the task resolves it | |
| t4 | Worker, agent | the worker prepares the merge in progress; the agent resolves `limits.rs` and runs the checks; the worker commits the merge, with both parents, and pushes it | B2's head is b2′ |
| t5 | CI, agent | CI passes on b2′; a review, asked again since an LLM resolved a conflict, approves it | |
| t5 | Engine | merges B2 at exactly b2′ | main is M3 |
| t5 | Engine | news for B (its last change landed: its coordinator's policy wakes on the last of its delegates done) and A (no overlap: kept) | |
| t5 | Coordinator B | its one run: reads its inbox (A1 and B1 landed, the conflict resolved) and finishes goal B with its report, which the issue's last milestone carries | goal B is done |

Today's design does more for less: at t0 its plan starts an LLM run on
B1 for its moved base, though nothing conflicts, and B2 gets one too
when B1 lands; and each coordinator wakes on news it needs no judgement
for.

## 16. Restart

At a restart (connectors.md, section 13) the connector is given, from
the store, the projects' repositories and what temper may do there, the
live tasks' resources and holds, the changes' and issues' states, and
its outbox's entries; then:

1. reads each repository's landing branches' tips and each live pull
   request afresh (head, state, mergeability, CI), before any change
   steps;
2. looks for each outbox entry: a pull request by its branches, a merge
   by its pull request's state, a creation by its key after the entry's
   start, a set written again;
3. steps every change once, and every goal's issue.

Its listings start from the newest time the store kept for each
repository, inclusive, so what changed while the engine was down is
read once.

## 17. Below the domain

What `docs/design/forge.md`, the forge's protocol layer, changes for
this design; the rest of it (translation only, no retries, every
request counted, paged and decoded as it streams, webhooks verified and
answered first, the fake forge's protocol layer, the load rules) stands.

- **Goes:** the record and outcome blocks inside comments, the notes'
  pages and the wiki's operations, nonces, `Mark::Record` and `Mangled`,
  and listings by label as the index of live work.
- **Stays:** keys' markers at the head of what temper creates; every read
  and write of its section 3.2 but the wiki's; webhooks' hints, with who
  caused them.
- **Goes too:** the person a key's marker names, since people's words
  no longer pass through the forge, and the wiki's webhook.
- **New calls,** with the groundwork facts and fallbacks in section 20:
  - a pull request's files and its diff at its head, and a comparison's
    files and commits, for the landings on a branch since a commit;
  - updating a pull request from its base, with the merge style, and how
    it answers a conflict;
  - posting a commit status;
  - editing a pull request's or an issue's title and body;
  - branch protection, read at adoption;
  - creating a branch at a commit;
  - a failed CI job's description and link from commit statuses; its
    output only when the provider has a supported log read (section 20);
  - the repository's settings at adoption: its merge styles, its
    default branch; and its collaborators with their permissions, to seed
    people's roles (people.md, section 4).
- **Repositories are adopted at runtime,** not read once at startup:
  what the protocol layer keeps per repository (its object format, merge
  style, webhook secret) is given as a repository is adopted, and a
  project's repositories may be on several forges, with one API token
  each.

## 18. The world

The connector's world, `tests/engine/forge`, runs its subtree with the
fake forge below it and the root scripted above it. Its stories: a
change produced, opened, checked, gated, queued and landed; CI failing,
repaired; a gate asking for changes, repaired, its verdict given again;
a gate added while queued; two changes into one branch, the second
updated cleanly and its review carried over; a change that leaves the
queue for a conflict and comes back at its place; a low-priority change
aging past the window ahead of later ones; a conflict resolved from a
merge in progress; a semantic conflict repaired; the branch's own CI
broken by a landing temper did not make, the queue paused and repaired;
its pull request closed by someone else, held, released and reopened; a
landing that overlaps a goal's change, and one that does not; a goal's
issue written, revised and closed; a merge whose answer is lost, found
merged after a restart; a forge preloaded with ten times the history,
idle at the same cost.

Its referee: nothing lands without CI passed and every blocking gate's
verdict valid at the exact head merged, and the head up to date with its
branch where the rule asks; merges land exactly the heads decided; keyed
effects are made once across restarts; temper never writes to what it
does not own, nor forces a push; a held change makes no effect; a
landing wakes a goal exactly when it overlaps; no change waits in a
queue past the aging window behind changes that became ready after it;
temper's own pushes are never taken for drift; the calls made while
idle are bounded by the limits.

## 19. From today

- **The forge child domain** becomes `client`, without records: its
  calls and request budget (fresh, write, keep, slow, costs settled when
  answered), its reads, its write lanes (per item and per repository),
  keyed creations found again, and merges at a head stay; its listing of
  changes since, its per-pull-request polling, its hints and dropped
  echoes stay. The lifetime stays for uncertain entries only: today a new
  engine makes no call at all until a lifetime after it starts. What
  goes: finding records in comments and their nonces, mangled records,
  the tracking and hand-in listings and the slow pass's probes for
  unlabelled records, the wiki's pages, reading verdicts on pull requests
  temper owns, and labels as anything but display. Where a find starts,
  today an outcome's comment, becomes the outbox entry's start
  (section 6). Admitting an item by finding its record becomes admitting
  a resource a live task names.
- **The plan's way to landing** (the current `engine-domain.md`, 5.3)
  becomes the change procedure. What it keeps: repairs and rebases
  bounded apart, stalls, a merge at exactly its head, a merge refused for
  a conflict taken as a moved base. What changes: a moved base or a
  conflict no longer starts an LLM run on the branch, which, as the code
  stands, cannot take its base in (its run starts from the change's own
  branch and the worker commits on top with one parent); a branch
  another party deleted is drift, held, instead of made again from its
  base.
- **Reading a change's branch** before producing it again, so a lost
  attempt's push is found, stays (8.2); refusing to merge a pull request
  retargeted since its decision stays (section 6).
- **Gates:** today a change has one review at its exact head, by an agent
  or a person, and may ask for approvals and a person's acceptance. That
  review becomes one gate among several, blocking or advisory, amendable
  while the change is live, and carried over clean updates.
- **The comment inbox** stays for participating objects only.
- **Issues per step** become one issue per goal, a projection; the
  hand-in label goes.
- **Code that goes:** in the engine's protocol layer, the committed but
  unexported `forge_blocks.rs` and `forge_cursor.rs`; in
  `temper-forge-forgejo`, wiki pages, listing by label, label writes as
  anything but display, and the wiki's webhook; in the fake forge, its
  wiki. The engine world's codecs for records and wiki pages go with them.
- **The fake forge grows:** merges with two parents, updates from a base
  that may conflict, commit statuses, branches made at a commit, pull
  requests' files and diffs, comparisons, CI's output, branch protection,
  repository settings and collaborators; the fake checkout grows merges
  that conflict.
- **New:** landing queues, mechanical updates, resolutions from a merge
  in progress, several gates with their carry-over, landings as news
  with overlap, adoption at runtime, branch naming after the tree.

## 20. Forgejo facts and open questions

### 20.1 Target version and groundwork observations

The target is **Forgejo v16.0.5**. Job logs are readable through its
supported API; the client and repair procedures assume this capability.
The tagged [v16.0.5 API specification](https://codeberg.org/forgejo/forgejo/src/tag/v16.0.5/templates/swagger/v1_json.tmpl)
defines:

- `GET /repos/{owner}/{repo}/actions/runs/{run_id}/jobs` to name a run's
  jobs and attempts;
- `GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs?attempt={attempt}`
  for one job's plaintext log, with HTTP byte ranges (`200` or `206`);
- `GET /repos/{owner}/{repo}/actions/runs/{run_id}/logs` for a ZIP of the
  latest attempts' logs. Repair briefs use the bounded per-job read.

A log read names the exact job attempt associated with the failing head;
its owned bytes count against the client's reply and brief limits. Missing,
forbidden or failed reads are explicit outcomes, not an unsupported-API
fallback. Tagged-source inspection establishes the API contract; the v15
capture below is historical evidence and does not verify v16 responses,
permission behavior or byte-range handling. The v16 protocol conformance
work must retain real exchanges for these routes.

Checked against the verified Forgejo 15.0.0 binary, in disposable SQLite
state on loopback, with a non-admin write collaborator and an isolated
Forgejo runner v3.5.1. The shell-only workflow prints a fixed marker and
exits unsuccessfully; it downloads no actions and performs no checkout.
The actual exchanges and signed update webhooks are in
`tests/forge/forgejo/fixtures/v15/next-domain-observations.json`, captured
by `tools/forgejo-conformance/run.py --next-domain`. Each case below names
an exchange there. These are facts about this version and configuration,
not a claim about every Forgejo deployment.

| Question | Observed fact and consequence | Evidence cases |
|---|---|---|
| Conflicting merge-style update | `POST .../pulls/{n}/update?style=merge` returns `409`, with message `merge failed because of conflict`; the head is unchanged. This specific refusal is a conflict resolution. Other failures still require a fresh read before deciding what happened. | `00a-conflicting-update`, `00a-conflict-head-after` |
| Clean update and CI | Update returns `200` with an empty body, moves the head, emits push and synchronized pull-request webhooks, and starts the configured push workflow on exactly that new head. Its deliberate failure appears in both Actions and the combined commit status, with a description and job link. CI must be checked again at that head. | `00a-clean-update`, `00a-clean-pull-after`, `00a-runs-after-update`, `00a-updated-head-status`; both retained webhooks |
| File and commit listing | Pull-request files page correctly: with `limit=1`, three pages contain distinct files and the fourth is empty; their commit URLs name the current head. Comparison returns all three files and commits on every `page=1..4&limit=1`, even with `MAX_RESPONSE_ITEMS=2`; paging is ignored. Apply the bounded, conservative handling of section 7.3. | `00a-pull-files-page-1` through `-4`, `00a-comparison-page-1` through `-4` |
| Failed Actions log | A real failed job and its status are readable with the write token. The running binary's REST Swagger has no job-log route; attempted run `jobs` and `logs` routes both return `404`. This establishes the absence of a supported REST log read on v15 only. It is superseded for the v16.0.5 target by the API contract above; repair briefs read job logs (8.4). | `00a-run-at-updated-head`, `00a-tasks-after-update`, `00a-probe-jobs`, `00a-probe-logs`; extracted `next_domain.actions_api_paths` |
| Branch at a commit | `POST .../branches` with `old_ref_name` equal to a full commit id returns `201` and creates the branch at exactly that commit. No worker push is needed to create or recreate it. | `00a-main`, `00a-create-commit-origin` |
| Protection with write permission | Both listing protections and reading the existing `main` protection return `403`: `user should be an owner or a collaborator with admin write of a repository`. Adoption records protection as unknown (section 4). The token's user is non-admin and its repository permission is `write`. | `00a-writer-user`, `00a-writer-permission`, `00a-protect-main`, `00a-protection-list-as-writer`, `00a-protection-as-writer` |
| Conditional merge and retarget | A merge naming the old head returns `409`, with message `head out of date`. After retargeting, merging the current head returns `200` with an empty body, records the retargeted base and merge commit, advances only that base and leaves `main` unchanged. Read the current base and head before landing. | `00a-merge-stale-head`, `00a-retarget-pull`, `00a-merge-retargeted-current-head`, `00a-retargeted-merged-pull`, `00a-retarget-base-after`, `00a-main-after-retarget` |

Every prerequisite of step 00a is resolved by an observation or its
specified fallback for the probed v15 configuration. Large comparison
completeness and other workflow configurations are not inferred from this
small fixture. The v16.0.5 log routes require their own conformance
exchanges. The retarget probe changes the base before the merge; it
does not establish an atomic condition on both base and head, or behavior
when a person retargets concurrently with the merge.

### 20.2 Open questions

- **A queue's throughput:** landings into one branch wait on CI one after
  another. If that binds, landings are batched: tested together, and
  bisected when they fail.
- **Conflicts found before either side lands:** temper sees every open
  change, and could try them against each other and warn both goals.
- **Checks without CI:** what a project's checks are, and where they run
  (a checks-only run on a worker, as the current `worker-domain.md`
  leaves open).
- **A person's approval as a rule,** posted as the person so that
  protection requires it independently (core.md, section 11).
- **Webhooks through the API** at adoption, which needs admin rights on
  each repository.
- **Intake:** issues others open as a source of goals.
- **A resource refused as forbidden:** whether the connector should hear
  when it is readable again, rather than read it on a backoff.
- **GitHub,** later: the same connector over another protocol layer
  (`docs/design/forge.md`, section 11), its pull request update and
  merge queue mapped onto the same effects.
