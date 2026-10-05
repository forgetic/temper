# Projects

Provisional, 2026-10-05. A project as the web shows it: its page, its
goals in priority order, the findings agents filed, its notes, its
recurring tasks, its settings and its spend; and the system's own page,
for the deployment as a whole. Projects are `domain/core.md`, 5.1; their
policy, `domain/authority.md`, section 6. What is still open is listed
in section 10.

## 1. In one page

- **A project is where work is steered:** its goals, by priority; what
  its agents found; what temper has learnt about it; and the rules its
  work follows.
- **The board is the project's goals in order.** Moving a goal up moves
  its changes up every landing queue it is in.
- **Most goals come from chats** (chats.md, section 6); the board can
  also set one directly.
- **Settings say in words what each rule does,** and what a change to it
  will do: it applies to what is decided after it, and takes back
  nothing already given.
- **The system's page** is the deployment's: its workers, LLM accounts,
  connectors and capacity, for the one person now and an operator later.

## 2. A project's page

What needs a glance, each part opening onto its own page:

- **what needs the person** in this project (inbox.md, filtered);
- **its goals,** the top of the board (section 3);
- **its changes,** in flight and queued (forge.md, section 3);
- **its spend** this period (7.3);
- **what is wrong,** from its connectors: a repository whose webhooks
  have gone quiet, a landing branch whose CI fails at its tip, a queue
  paused or held (forge.md, section 5).

## 3. The board

### 3.1 The goals

The project's goals (`domain/core.md`, 5.2), live ones by priority, each
a line:

- its chip, and its phase in words;
- its progress: its plan's tasks by phase, and its changes landed and in
  flight;
- its spend against its budget, and its deadline;
- held below it, as a count (tasks.md, 6.1);
- its issue on the forge (forge.md, section 7).

Waiting goals follow, then those ended recently, paged.

### 3.2 Priorities

- **Moving a goal** up or down the board is a request (prioritise goals,
  `domain/people.md`, 5.1), pending like any other.
- **The board says what priority does:** it orders the goals' changes in
  each landing queue, a change outside any goal taking the project's
  default, and a change ready for longer than the project's window goes
  first whatever its priority (`domain/forge.md`, section 9).

### 3.3 A goal's page

A task page (tasks.md) with its plan first, and more:

- **its priority,** and its place on the board;
- **its issue,** and when it was last written (forge.md, section 7);
- **landings as news:** the landings in its repositories since it began,
  each marked as its own, overlapping its work (it woke the goal), or
  not (kept) (`domain/forge.md`, section 7);
- **its notes,** of its scope (section 5).

### 3.4 Setting a goal

From the board, for a goal the person knows well enough to skip a chat:

- **what it is,** in words;
- **who carries it out:** one of the charters the deployment offers for
  goals, a coordinator by default (`domain/engine.md`, section 14);
- **its budget,** from the person's pool, saying what is left;
- **its deadline,** if any, and its **priority,** as a place on the
  board;
- **where it may land:** the repositories and landing branches it may
  change, chosen within the project's ceiling and said in words.

Beyond what the person's pool has left, the form says so; for the one
owner, widening their pool is a policy change (7.2). Accepted, the goal
is on the board and its coordinator starting.

## 4. Findings

What agents filed as issues (`domain/forge.md`, section 12): a bug found
while exploring, a follow-up a review raised, a flaky test.

- **Each** shows its title, its repository and issue, the task that
  filed it, when, and its state, which is the store's: open, taken up by
  a goal, dropped.
- **Taking one up** sets a goal that names it (3.4, with its words
  filled in), or starts a chat about it (chats.md, section 3).
- **Dropping one** gives a reason.

Taking up and dropping are requests the domain does not have yet
(README.md, section 10).

## 5. Notes

What temper has learnt (`domain/engine.md`, section 10), by scope: the
deployment, the project, each repository, each goal.

- **Each note** shows its one-line description, its body, who wrote it
  (a person, or a task as a chip), the tasks it refers to, and when it
  last changed.
- **Correcting one** names the revision the person read; if it changed
  since, the request is refused as moved, and the web shows the newer
  revision to correct instead.
- **Deleting one,** and **writing one,** such as a preference agents
  should know of ("never squash migrations together"); writing is a
  request the domain does not have yet (README.md, section 10).
- **Searching** by description, as `recall` does.
- **Notes are hints:** the page says so, and that knowledge of the code
  belongs in the repository, where a note may become a change.

## 6. Recurring tasks

The project's recurring tasks (`domain/tasks.md`, section 9), each a
line: its template in words, its period, its budget per period, its last
batch (a chip, with how it ended), and when it next runs. Each is a task,
with its page; cancelling it ends it. Making and changing one are
requests the domain does not have yet (README.md, section 10); until
then, they come from configuration.

## 7. Settings

### 7.1 Repositories

The project's repositories, each with its role (owned, a fork, context),
its landing branches, temper's branch prefix there, its merge style, how
its changes are checked without CI, and its health. Adopting one, and
what its health shows, is the forge's panel (forge.md, section 5).

### 7.2 Policy

The project's policy (`domain/authority.md`, section 6), in words, each
part saying what it does:

- **its ceiling,** the most any task may do, which follows from its
  repositories and is shown, not edited: "push under temper/ and land
  into main in ai/temper; read ai/skein";
- **its spend per period;**
- **its roles:** for each, what a person in it may give a task, may
  allot per period, and may decide;
- **its landing rules,** per landing branch: CI on the exact head, up to
  date with the branch, and the gates every landing needs, each blocking
  or advisory: reviews by a lens, approvals by a role and how many
  people (`domain/authority.md`, section 10);
- **who decides what:** the role each kind of proposal and escalation
  goes to past its tree.

A change shows before and after, and says that it applies to what is
decided after it and never takes back what a task was given
(`domain/people.md`, 5.2). Beyond the deployment's rules, it is refused,
naming the rule.

### 7.3 Spend

- **The project's period:** its spend, what is spent, reserved and left,
  and when the period ends.
- **The person's pool** in it, likewise.
- **Where it went:** by goal and by chat, largest first, each opening
  onto its budget (tasks.md, section 9).
- **Past periods,** paged.

### 7.4 People and roles

The one owner, for now (README.md, section 3). With several people:
who holds which role, seeded from the repositories' collaborators at
adoption (`domain/people.md`, section 4), and changed by an owner.

## 8. The system

The deployment's own page, for the one person now and an operator later
(README.md, section 11):

- **Workers** (`domain/engine.md`, section 8): each connected or lost
  (and for how long its grace still holds), its slots used and free, the
  runs it hosts, as chips, and the workstreams it has cached; a worker
  refused at its hello, and why.
- **LLM accounts** (`domain/engine.md`, section 12): each usable or not,
  and why; the runs using it; the tasks waiting for it to start, which
  their own pages say too ("starting: waiting for its account").
- **Connectors:** for each forge, its request budget and what uses it, a
  pause for its rate and until when, its outbox (effects waiting, and
  those uncertain, being looked for), and when each repository's
  webhooks were last heard (`domain/connectors.md`, sections 4 and 12).
- **Capacity:** live tasks against their limits, for the deployment and
  each project; pending proposals; what was refused for want of room,
  recently, since a full deployment refuses new batches
  (`domain/tasks.md`, section 10).

Each needs a stream the domain does not have yet (README.md,
section 10).

## 9. From the domain

| The web | The domain |
|---|---|
| a project, its policy and its roles | `domain/core.md`, 5.1; `domain/authority.md`, section 6; `domain/people.md`, section 4 |
| the board, priorities | goals (`domain/core.md`, 5.2); prioritise goals (`domain/people.md`, 5.1) |
| setting a goal | set a goal (`domain/people.md`, 5.1) |
| findings | `domain/forge.md`, section 12 |
| notes | `domain/engine.md`, section 10; edit or delete a note (`domain/people.md`, 5.1) |
| recurring tasks | `domain/tasks.md`, section 9 |
| changing the policy | change the policy or roles (`domain/people.md`, 5.1 and 5.2) |
| spend | funders' numbers (`domain/authority.md`, section 7) |
| the system | the fleet, accounts, connectors' load, limits (`domain/engine.md`, sections 8, 12, 13) |

## 10. Open questions

- **Tracking a task that exists:** any task may be a goal
  (`domain/core.md`, 5.2), but no request makes a live task tracked; a
  chat's delegate that grew into a goal would need one.
- **Across projects:** a board of every project's goals, for a person
  with several.
- **A period's window,** rolling or reset (`domain/authority.md`,
  section 13), which the spend page shows.
- **An operator's role** for the system's page.
