# temper's web

Provisional, 2026-10-05. The user experience of temper's web: how it is
organised, what a person finds where, and what they do there. It is
built on the domain of `docs/design/domain/`, which it takes as given,
and it says nothing of visual design: no layouts, styles or markup, only
the places the web has, what each shows and the actions each offers. It
assumes one person for now (section 3). What the web needs that the
domain does not yet give is section 10; what is still open, section 11.
How the web is built is `../architecture.md`.

## 1. In one page

- **Three things people do.** They **talk** to agents (chats.md),
  **decide** what waits for them (inbox.md), and **watch and steer** the
  work (tasks.md, projects.md). The web is organised around these, not
  around temper's parts.
- **The task page is the one universal page.** Every task, whatever its
  executor (an agent, a procedure, a person), has a page of the same
  shape: what was asked, where it stands, what it did, what it spent,
  and what may be done to it (tasks.md). A chat and a goal are tasks,
  and their pages are task pages with more.
- **The core is generic; connectors add panels.** The web knows the five
  primitives (`domain/core.md`, section 3), not the shapes of work made
  from them. What a connector's resources and procedures look like (a
  change on its way to landing, a landing queue, a repository's health)
  the connector contributes as panels, as it contributes sections to a
  brief (`domain/engine.md`, section 9). The forge's are forge.md.
- **One object, one card, many places.** A proposal shows in the inbox,
  inline in the chat that made it, and on the task page of who proposed
  it: the same card with the same actions, deciding the same thing once.
- **Every request is a decision.** It is pending until the commit that
  answers it is durable, never shown as done before; refused, it says
  why; beyond what the person may do, it offers to propose instead; sent
  twice, it is made once.
- **The store is the truth; streams are provisional.** What runs say as
  they go is streamed live, and replaced by what was committed. A page
  that falls behind says so and catches up.
- **Held is where problems surface.** A held task says why in words, and
  offers the actions that fit its reason.
- **People steer at the level of goals, plans and outcomes.** Code stays
  on the forge: the web links to diffs and logs, and never asks a person
  to review code to keep work moving unless a rule says so.
- **Bounded, like the engine.** Lists are paged, large trees fold into
  counts, history is loaded when asked for.

## 2. Reading order and conventions

1. **README.md** (this document): principles, the map, cards, live and
   committed, a goal as the web shows it, the first slice.
2. **inbox.md:** what waits for a person, and deciding it.
3. **chats.md:** conversations with agents.
4. **tasks.md:** the task page, plans as trees, holds, steering.
5. **projects.md:** projects, goals and their priorities, findings,
   notes, settings, spend, and the system's own page.
6. **forge.md:** the forge connector's panels.

- **A bare file name** names a document of this directory: `tasks.md` is
  the task page here. The domain's documents are named by their
  directory: `domain/tasks.md`.
- **"The web"** is temper's web user interface; **a person** is someone
  signed in to it.

### 2.1 Names

| Name | What it is |
|---|---|
| page | a place in the web with its own address: the inbox, a chat, a task, a project's board |
| card | how one domain object shows wherever it appears: a proposal, an escalation, a question, a person task, a result (section 6) |
| chip | a task named in passing: its number, its title, its phase, kept live |
| entry | a card in a person's inbox, waiting for them |
| panel | a part of a page that a connector contributes for its resources or procedures |
| board | a project's goals, in priority order (projects.md, section 3) |
| stream | a live view a page holds open (`domain/engine.md`, section 11) |
| pending | a request sent and not yet answered durably |

## 3. One person, for now

The web is designed for one person: the deployment's first owner
(`domain/people.md`, section 3), owner of every project, who sees
everything, the system's page included. Deferred until there are several:

- **who may see what:** whether a member reads another's chats and
  transcripts, what an observer sees, read privacy in general;
- **sharing what is addressed to a role:** taking a person task so that
  two people do not answer at once, and handing it back
  (`domain/people.md`, section 8), designed lightly in inbox.md;
- **people and roles in settings,** beyond showing the one owner;
- **guests** and **notifications beyond the web.**

What stays with one person, and is designed in full:

- **Things change under the person.** Two open tabs decide the same
  proposal; a proposal is withdrawn while it is read; a task ends while
  it is amended. The first commit decides; the page that lost says what
  happened, by whom, and how.
- **Authority holds for the person too.** An owner's role is within the
  project's policy and their pool within the project's spend, so a
  request may still be beyond them, and still be proposed or refused.
- **Agents decide too.** A coordinator decides what its delegates propose
  and releases what they hold before anything reaches the person; the
  person sees it on the task pages, not in the inbox.

## 4. The map

```
Inbox            what waits for the person, across projects            (home)
Chats            their conversations; a new one
Project ▸
  Board          the goals, by priority: progress, spend, what is held
  Changes        the forge's panel: changes in flight, a queue per landing branch
  Tasks          the project's live tasks, by phase, executor or goal
  Findings       issues agents filed; taking one up
  Notes          by scope; reading, correcting, deleting, writing
  Settings       repositories, policy, spend, people and roles
Task N           the universal page, from anywhere
System           workers, LLM accounts, connectors, capacity
```

- **Everywhere:** the inbox's count of what needs the person; the
  project they are in, and a way to switch; going to a task by its
  number; and whether the web is live, behind, or offline (section 7).
- **Links are by number.** Every task, wherever it is named (a chip, an
  entry, a history line, a transcript), links to its page; a task's
  number is its address, and never reused (`domain/tasks.md`, section 3).
- **Home is the inbox,** since what waits for the person is what work
  waits on.

## 5. Principles

### 5.1 Generic core, connector panels

The web knows tasks, executors, messages, authority and connectors, and
nothing of plans, coordinators, chats or reviews beyond what charters
and roles say. Where a page shows something of a system (a change's
pull request, CI on its head, a branch's landing queue, a repository
that sends no webhooks), it is a panel its connector contributes for the
resources a task names, or for a project's resources. A task page has
the core's sections and its connectors' panels; a second connector (test
environments, `domain/connectors.md`, section 14) adds panels and changes
no page.

### 5.2 One object, one card

Each domain object that asks something of a person (a proposal, an
escalation, a question, a person task) and each result has one card,
shown wherever it is relevant, deciding the same thing:

- a goal a chat proposes shows inline in the conversation and as an
  entry in the inbox; accepted in one, it is gone from the other;
- a held task shows on its page, in its parent's plan, on its goal's
  board line, and in the inbox of whoever its escalation reached.

### 5.3 Requests are decisions

Every action a person takes is a request (`domain/people.md`, 5.1):

- **pending** from when it is sent until it is answered, which is after
  its commit is durable; the page shows it as asked, not as done;
- **keyed** by the page, so a double click, a retry after a lost
  connection, or a reload sends the same request and gets the same
  answer;
- **refused, it says why:** the authority it lacked, a limit, a task
  since ended, a state since changed;
- **beyond the person's authority** but within what someone could
  accept (a goal past their allotment), it offers to propose it instead;
- **decided by another first,** it says by whom and how.

There is no undo. What can be reversed is reversed by another request (a
stopped task released, an amendment amended back); what cannot (a
cancel, an acceptance, a rejection) says what it will do before it is
sent.

### 5.4 Committed and live

What a page shows comes from two places (section 7): the store, through
the engine, which is the truth; and streams, which are best effort and
run ahead of it. A run's streaming text is shown as it comes and
replaced by its turn once the turn commits; a phase changes on a page as
its commit is made.

### 5.5 Held is the exception surface

A held task is the one thing that waits for a decision about the work
itself. Its card says, in words, why it is held (out of tries, out of
budget, past its deadline, a dependency failed, a pull request closed by
someone else, stopped by the person, stalled, a procedure that needs a
decision), what releasing it would do, and offers only what fits:
release; amend, then release; cancel; or, for a budget, widen it and
release in one. Each reason is tasks.md, section 7.

### 5.6 Outcomes, not code

People set goals, accept plans, choose between approaches, approve where
a rule asks, and read what came of it. Reviewing code is the agents'
business (`domain/core.md`, section 1). The web shows a change's state
(its pull request, CI, its gates' verdicts) and links to the forge for
its diff and its logs; it has no code review of its own.

### 5.7 Stop, cancel and close

Three different requests, which the web keeps apart in name and in what
it says before they are sent (tasks.md, section 8; chats.md, section 7):

- **Stop** halts a task's live run now and holds the task, for the
  person; nothing else changes, and releasing it carries on from where
  it was. Seeing a producing run go round in circles, the person stops
  it, amends its instructions, and releases it.
- **Cancel** ends a task and everything below it, for good: live runs
  cancelled, proposals withdrawn, effects not yet made dropped, pull
  requests closed. Abandoning a goal is cancelling it.
- **Close** is how a chat ends: it is asked to finish, then cancelled if
  it does not within a grace. A chat whose delegates are still live asks
  the person first what becomes of each: cancelled, or made theirs.

### 5.8 Bounded

The engine holds live work only, and so does what a page shows live:
lists of ended work are paged from the store, a plan with many tasks
folds into counts per phase until opened, a transcript is loaded a page
at a time, history on demand (`domain/engine.md`, 5.3 and 11).

## 6. Cards

Cards are the web's shared vocabulary. Each is described where it
mostly lives:

| Card | Shows | Actions | In depth |
|---|---|---|---|
| task chip | number, title, phase, executor kind, live | opens the task | tasks.md, 2 |
| proposal | who proposes, what, why, what it needs, who would fund it | accept, reject with a reason, pass up | inbox.md, 4.1 |
| escalation | the held task, why it is held, who it waits for | release, amend and release, leave held, cancel, pass up | inbox.md, 4.2; tasks.md, 7 |
| question | which task asks, its words, its context | answer | inbox.md, 4.3 |
| person task | what is asked, the results it depends on, its contract | answer: a choice, an approval at a head, words | inbox.md, 4.4 |
| result | the task ended: done, failed or cancelled, with its result by its contract | read; open the task | inbox.md, 4.5 |
| refusal | what was asked, why it was refused | propose instead, where it can be | section 5.3 |
| change | a change's way to landing, at a glance | opens its panel | forge.md, 2 |

## 7. Live and committed

- **A page opens from a snapshot,** then follows a stream: a run (its
  streaming text, its tool calls, its turns as they commit), a task's
  tree (phases as they change), a project's goals, a person's inbox
  (`domain/engine.md`, section 11; `domain/people.md`, section 9).
- **History is loaded:** a task's ended delegates, its transcript, its
  revisions, a goal's landings, a page at a time, as the person asks.
- **Three states of a page,** shown everywhere:
  - *live:* its streams are flowing;
  - *behind:* a stream dropped what it could not deliver (a slow
    watcher, `domain/engine.md`, section 11), and the page reloads its
    snapshot; nothing the person did is lost, since requests are keyed;
  - *offline:* the engine is unreachable; requests pending stay pending,
    and are sent again with their keys when it is back.
- **A restart is invisible** but for a moment offline: sign-ins survive
  it (`domain/people.md`, section 3), requests are answered once, and
  what streamed but was never committed is gone from the page as it
  reloads, which is why streamed text is only ever provisional.

## 8. A goal, as the web shows it

The goal of `domain/core.md`, section 9, adding OAuth login to temper,
as P, the one person, meets it:

| Step | Where | P sees | P does |
|---|---|---|---|
| 1 | Chats | a new chat in project temper | writes "I'd like the web to support OAuth login" |
| 2 | the chat, T0 | T0's replies stream in; it reads code, asks two questions | answers in the conversation |
| 3 | the chat, and the inbox | a proposal card: T1 "add OAuth login", a coordinator, a budget from P's pool, landing into main, to be P's own | accepts it in the chat; the inbox entry goes with it |
| 4 | Board | T1 on the board, at the priority P gave it, linked to its issue; its plan filling: spikes T2, T3 running, T4 and T5 waiting | nothing; watches |
| 5 | Inbox | a person task, T4: choose between library X and a hand-rolled flow, with T2's and T3's reports side by side | chooses X, with a line of why |
| 6 | T5's page | the design change's panel: produced, opened, CI passed, a design review approving, queued, landed | nothing |
| 7 | T1's page | History: T1's revision, T6…T9 made, with T1's reason | reads it |
| 8 | T7's page | the change panel, a link to its diff on the forge | amends T7: adds a blocking memory-safety review; T7 leaves the queue until it has its verdict |
| 9 | T1's plan | T8 held, out of tries, then released by T1 | nothing: T1 decided it; it never reached P |
| 10 | Inbox | a result: T1 done, with its report; its issue closed | reads it |

Had P closed T0 at step 5, T1 would have gone on: it is P's (chats.md,
section 7).

## 9. The first slice

What the domain builds today (`domain/people.md`, 12.1;
`domain/tasks.md`, section 15; `domain/engine.md`, 7.6 to 7.8) gives a
first web of a few pages:

- **signing in** through the forge;
- **a new chat:** a project and words, sent as one keyed request,
  answered with the chat's number once durable;
- **the chat's page,** minimal: its words, its phase, and its result, a
  report, once it ends, read as a named result after a reconnect;
- **a held chat's escalation,** read and decided: release, or leave held
  with a reason (passing it to the project's escalation role means
  nothing while the one person is both).

Setting a project's roles is built too, and waits for there being
several people (section 3).

Streaming, words to a live run, the tree, the inbox as a derived list
and the board follow as their domain routes are built. The first slice
needs one thing the domain does not give yet: a listing of the person's
chats, since there is no inbox route to find them by (section 10).

## 10. What the web asks of the domain

What these documents rely on that `docs/design/domain/` does not have
yet, or leaves open:

- **Listings:** a person's chats; a project's live tasks by phase,
  executor or goal; a tree's ended tasks; a person's inbox as
  `domain/people.md`, section 6 designs it, but across projects
  (`domain/people.md`, section 14, which the web takes as decided).
- **Streams beyond those of `domain/engine.md`, section 11:** a person's
  inbox; the system's workers, accounts, connectors and capacity
  (projects.md, section 8).
- **Requests not in `domain/people.md`, 5.1:** writing a note (people
  may edit and delete one, and the domain lets a person be its author);
  taking up or dropping a finding; making, changing and ending a
  recurring task; naming a chat.
- **What a run was told:** a run's brief, kept, if people are to read
  why an agent did what it did (tasks.md, section 5).
- **A procedure's steps,** kept as history, for a change's log
  (forge.md, section 2).
- **A deployment-level role** for the system's page, once there are
  several people.

## 11. Open questions

- **Who may see what,** with several people (section 3).
- **A deployment-level role,** an operator, for the system's page.
- **Notifications beyond the web** (`domain/people.md`, section 9).
- **Search:** over tasks' specs and results, notes and transcripts; the
  domain has a search only over notes' descriptions
  (`domain/engine.md`, section 10).
- **Guests** (`domain/people.md`, section 14).
