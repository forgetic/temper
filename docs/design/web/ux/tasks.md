# Tasks

Provisional, 2026-10-05. The task page, the web's one universal page:
what it shows of any task, whatever its executor; a task's plan as a
tree and its history; what a held task says and offers; and the
requests that steer a task. Tasks are `domain/tasks.md`; what a person
may do to one, `domain/people.md`, section 5. What is still open is
listed in section 12.

## 1. In one page

- **Every task has a page,** at its number, of one shape: a header
  saying what it is and where it stands, then its overview, its
  activity, its plan, its history and its budget, then its connectors'
  panels. A chat's page leads with its conversation (chats.md), a goal's
  with its plan; neither is another kind of page.
- **Phases in words.** The domain's lifecycle (`domain/tasks.md`,
  section 5) is shown as what the task is doing and what it waits for,
  never as a bare state name.
- **The plan is the subtree.** A task's delegates, and theirs, as a tree
  that folds what is done and counts what is folded, so a held task deep
  down is never hidden.
- **History is the plan's story:** every task made, amended, held,
  released or cancelled, by whom, and why, in their words.
- **Held says why, and offers what fits** (section 7).
- **Steering is a handful of requests:** write, amend, stop, release,
  cancel, take over; each says what it will do before it is sent
  (section 8).

## 2. The task page

### 2.1 Its header

- **What it is:** its number, its title (its spec's first line), its
  project, whether it is a goal and its priority.
- **Who carries it out:** an agent and its charter ("a coordinator", "a
  reviewer, memory safety"); a procedure and its connector ("the forge's
  change"); a person.
- **Where it stands:** its phase in words (section 3), and whether the
  page is live (README.md, section 7).
- **Where it comes from:** its lineage, from the person or the
  deployment that rooted its tree down to it, each a chip:
  "P ▸ T1 add OAuth login ▸ T7 web flow ▸ T23 repair".
- **What may be done:** the requests of section 8 that fit its phase.

### 2.2 Its sections

| Section | Agent | Procedure | Person |
|---|---|---|---|
| overview: what was asked, its result, what it hears (section 4) | yes | yes | yes |
| activity: what it did (section 5) | its runs and transcript | its panel and its steps | the question, and the answer |
| plan: its subtree (6.1) | yes | yes: what it made | rarely: a person who delegates |
| history (6.2) | yes | yes | yes |
| budget and authority (section 9) | yes | yes | its funding only |
| connectors' panels | for the resources it names | its own procedure's panel first | for what its question names |

A section with nothing in it (a plan with no delegates) is not shown.

### 2.3 The chip

A task named anywhere (a plan, an inbox entry, a history line, a
conversation) is a chip: its number, its title, its executor's kind, its
phase, kept live while it shows, linking to its page.

## 3. Phases, in words

| The domain | The web says |
|---|---|
| waiting | waiting for T2, T3 (each a chip, with its phase) |
| active, an agent: idle | waiting for news; what would wake it (its wake policy in words) |
| due, preparing, claimed | starting |
| running | running: writing, or which tool, attempt 3, for how long |
| backing off | retrying at a time, and why: "worker lost, try 2 of 3" |
| active, a procedure | its panel's words: "checking: CI running on a1b2c3" |
| active, a person | waiting for the person's answer, since when |
| closing | closing, and what it waits for: "2 delegates closing, 1 merge settling" |
| held | held, and why (section 7) |
| done, failed, cancelled | ended so, when, and by whom for a cancel |

## 4. Overview

- **What was asked:** its spec's words, whole; the resources it reads and
  writes, by name; its inputs, as chips with their results; for an
  agent, its charter's instructions; for a procedure, its parameters (a
  change's repository, landing branch and gates); for a person, the
  question.
- **What counts as done:** its result contract in words: "a report";
  "a verdict: approve, or request changes with remarks"; "a change
  landed into main in ai/temper"; "a choice between library X and a
  hand-rolled flow".
- **Its result,** once it has one, rendered by its contract; for a task
  cancelled while closing, the result it had, with why it was cancelled
  (`domain/tasks.md`, 5.6).
- **Its dependencies,** each a chip with its phase: "after T2 (done) and
  T3 (running)".
- **What it hears:** its requester, its references, its subscriptions
  ("landings into main in ai/temper"; "T5's state"; "every night at
  02:00"), and its wake policy in words: "wakes on your words and its
  delegates' results; keeps landings that do not overlap its work".
- **What it holds:** the resources it writes alone, and which run holds
  the writer slot now ("branch temper/1/c7, written by T23's run").
- **Held,** its card first (section 7).

## 5. Activity

### 5.1 An agent's runs

The transcript, as a chat's conversation shows it (chats.md, section 4),
grouped by run. Each run says:

- **its attempt,** and **why it ran:** its first activation; woken by
  T2's and T3's results; news ("A1 landed in main, overlapping
  `limits.rs`"); a release; a retry after a failure;
- **how it began:** resuming the transcript, or fresh from a brief;
- **how it ended:** finished; parked; failed, with its class and reason
  ("the agent crashed", "its result broke its contract: no `title`");
  cancelled, and by whom;
- **what it spent.**

A run live now streams into the page (README.md, section 7). Runs that
failed are kept in the activity, folded, so the person sees what the
task went through before it succeeded or was held.

### 5.2 What a run was told

Each run's brief, its sections as the agent read them: the task and its
lineage, its inbox, its dependencies' results, its delegates, its
earlier attempts, its notes' index, its connectors' sections
(`domain/engine.md`, section 9). It answers "why did it do that?". It
needs each run's brief kept (README.md, section 10); until then, the
page shows what the brief was made from.

### 5.3 A procedure's steps

Its connector's panel first: for the forge's change, its way to landing
(forge.md, section 2). Under it, a log of its steps, each with what it
decided: effects asked, tasks made, a hold. The log needs each step's
decision kept (README.md, section 10); until then, the panel and the
history.

### 5.4 A person's answer

While it is active, the person task's card (inbox.md, 4.4). Once
answered: who answered, when, and what.

## 6. Plan and history

### 6.1 The plan

The task's subtree: its delegates, and theirs (`domain/core.md`, 6.1).

- **Each line** is a chip, with its executor, its phase, its spend, and
  its dependencies on its siblings ("after T2, T3"); a proposal waiting
  in the subtree shows on its proposer's line.
- **Folded:** an ended branch folds to its line; a level with many tasks
  folds into counts per phase, "12 done · 2 running · 1 held", opened on
  demand; a folded line carries the count of what is held below it, so
  nothing held is hidden by folding.
- **Filtered:** held only; live only.
- **Live** for what is open, as phases change (`domain/engine.md`,
  section 11); ended tasks are loaded from the store as their branches
  are opened.

```
T1 add OAuth login          coordinator   running        14.2 of 40
├─ T2 spike: library X       agent         done            2.1
├─ T3 spike: hand-rolled     agent         done            2.6
├─ T4 choose                 person P      done
├─ T5 design                 forge change  landed          3.0
├─ T6 config                 forge change  queued, 2nd     1.8
├─ T7 web flow               forge change  gating          4.1
│  └─ 2 done · 1 running
├─ T8 people domain          forge change  held: out of tries
└─ T9 docs                   forge change  producing       0.6
```

### 6.2 History

What the store keeps of a task and its subtree as it commits
(`domain/tasks.md`, section 1): each task made (with the batch it was
made in), amended (before and after), held (why), released, cancelled,
moved, with who did it (a person, a task, the deployment), when, and
the reason they gave. For a coordinator, it is the plan's story: each
revision with its reason, which is how a person reads why the plan is
what it is. Paged; shown for the task alone, or for its subtree.

## 7. Held, by reason

Every hold of `domain/tasks.md`, 5.5, as its card says it and what it
offers. A release returns the task where it was and judges the reason
afresh; where the reason would still hold, the card offers the
amendment first.

| Held because | The card says | It offers |
|---|---|---|
| out of tries | the class of failure, and each failed attempt's reason | release, tries counted again; amend its instructions, then release; cancel |
| stalled | what it waited for, and since when: a person's answer, a delegate, a check that never reported | release; for a person task, address it to someone else; cancel |
| a dependency failed or was cancelled | which, and why | amend it to drop that dependency, then release; cancel; start a chat about a replacement |
| drift | what changed on the system, by whom, when (its connector's panel, forge.md, section 6) | release, which does what the connector says; cancel |
| an effect failed for good | the effect, and the system's refusal | release, for its executor to decide afresh; cancel |
| out of budget | what it spent, and where: by run, by delegate | widen its budget, then release; cancel |
| past its deadline | its deadline | move the deadline, then release; cancel |
| stopped | by whom, and when | release; amend, then release; cancel |
| its procedure needs a decision | the procedure's reason: repairs past their limit, a queue's repairs past their bound | release, its counts reset; amend (a change's gates), then release; cancel |

- **Where its escalation waits** is on the card: "T1 has it" while a task
  above may decide it, the person once it reached them (inbox.md, 4.2).
  The person may act on a task in their tree either way, before the task
  above does; the first commit decides.
- **Held while closing,** the card shows the result it has kept;
  releasing it carries on closing.
- **Widening a budget** is from the task's funder: the person's pool for a
  task they fund, the task above otherwise, which the person amends in
  turn.

## 8. Steering

The requests a person makes on a task (`domain/people.md`, 5.1), each
offered where it fits, and each saying what it will do before it is
sent:

- **Write:** words to an agent's task, which wake it (chats.md,
  section 5). Not offered to a procedure, which decides on facts, nor to
  a person task, which is answered.
- **Amend:** its spec and parameters; its wake policy; its dependencies,
  removing some while it waits; its authority, narrowed freely and
  widened within the person's own (beyond it, proposed); its charter's
  instructions; a procedure's parameters, such as a change's gates
  (`domain/tasks.md`, section 6). The form shows before and after, and
  says what the task will do on hearing it: a live run hears it; a
  narrowing that takes a grant its run is using cancels that run, and
  the next starts narrower. Not offered once the task is closing.
- **Stop:** halts its live run now and holds the task, stopped by the
  person; its delegates go on, and what its run committed stays. Say
  T21, producing T7's change, has rewritten the same test five times:
  the person stops it, amends its instructions ("keep the existing test;
  fix the handler"), and releases it; the next run starts from the saved
  work.
- **Release:** a held task, by its reason (section 7).
- **Cancel:** ends the task and its subtree, deepest first
  (`domain/tasks.md`, section 6). Before it is sent, the web says what
  it ends: the tasks below, by phase; the live runs; the proposals
  withdrawn; the effects not yet made, dropped; those in flight, which
  settle (a merge already sent may land, and then its change ends
  landed); the pull requests closed and branches deleted. Cancelling T1,
  the OAuth goal, ends T2 to T10 as well.
- **Take over:** makes the person the task's requester, so it outlives
  the task that asked for it; its budget is reserved from their pool
  from then on, which the web shows ("reserves 6 from your pool"), and
  is refused if it does not fit. The task that asked keeps a reference
  to it (`domain/tasks.md`, section 6). Say T1 is to be cancelled but T3's
  spike is worth finishing: the person takes T3 over first.
- **Chat about it:** a chat naming the task (chats.md, section 3).

A goal's priority is changed on the board (projects.md, section 3).

## 9. Budget and authority

- **Its budget** (`domain/authority.md`, section 7), in words: given;
  spent by itself; spent by its ended delegates; reserved for its live
  ones; left. Who funds it: the task above, the person's pool for the
  period, or the project's period.
- **Where it went:** by run, and by delegate, largest first, so a
  person sees which part of a plan is expensive.
- **Its deadline,** if it has one.
- **Its authority** in words (`domain/authority.md`, section 3): the
  tools its agent has ("reads, edits, the shell, sub-agents; delegate,
  message, propose, note"); its grants ("push to its own branches in
  ai/temper; land into main"); what it may delegate (to which charters,
  tasks left of how many, how deep); the note scopes it writes.

## 10. Finding tasks

- **By number,** from anywhere (README.md, section 4).
- **From where they belong:** a goal's plan, a chat's delegates, an
  inbox entry, a history line.
- **A project's tasks:** its live tasks, filtered by phase (held,
  running, waiting, closing), by executor (a charter, a procedure, a
  person), by goal, or by whether the person asked for them; ordered by
  their last change. Ended tasks are reached through their goal or chat,
  their history, or their number.

## 11. From the domain

| The web | The domain |
|---|---|
| the page | a task (`domain/tasks.md`, section 3), its live tree's stream and its history, loaded (`domain/engine.md`, section 11) |
| activity | transcripts (`domain/engine.md`, 7.2); a run's stream |
| write, amend, stop, release, cancel, take over | `domain/people.md`, 5.1; `domain/tasks.md`, sections 5 and 6 |
| held, by reason | `domain/tasks.md`, 5.5 |
| budget and authority | `domain/authority.md`, sections 3 and 7 |

## 12. Open questions

- **A plan as a graph:** dependencies among siblings drawn as a graph
  rather than "after" on each line; whether it earns its place.
- **Bounding a large tree's stream** (`domain/engine.md`, section 17):
  the page streams only what is open.
- **Comparing attempts:** two runs of one task side by side, to see what
  a release or an amendment changed.
- **Briefs and steps kept** (sections 5.2 and 5.3).
