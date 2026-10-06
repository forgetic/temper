# The inbox

Provisional, 2026-10-05. What waits for a person, and how they decide
it: the web's home page. It is the person's inbox of
`domain/people.md`, section 6, shown across every project, with the
cards that every other page reuses for the same objects (README.md,
section 6). What is still open is listed in section 10.

## 1. In one page

- **Home is the inbox,** because what waits for the person is what work
  waits on.
- **Two groups.** *Needs you:* proposals and escalations to decide,
  questions to answer, person tasks to carry out; each holds some work
  up. *Updates:* results of what the person asked for, and replies from
  their chats; nothing waits on them.
- **Across projects,** filtered by project, kind or goal, since one
  person works on several (`domain/people.md`, section 14, taken as
  decided).
- **Derived, not kept.** An entry is there while its object waits: a
  proposal pending, a question unanswered, a person task active, an
  update unread. Deciding it, in the inbox or anywhere else its card
  shows, takes it away.
- **Deciding is a request,** pending until durable, refused with a
  reason, decided once however often it is sent (README.md, 5.3).
- **Every card says what deciding will do** before it is sent: what an
  acceptance makes and who funds it, what a release starts again.

## 2. What waits

| Entry | Group | From | Asks |
|---|---|---|---|
| proposal | needs you | a task in the person's tree, through every holder that could not cover it (`domain/tasks.md`, section 8) | accept, reject with a reason, or pass up |
| escalation | needs you | a held task nobody below could decide | release, possibly after amending it; leave it held; cancel it |
| question | needs you | a task the person asked for, a chat included | an answer in words |
| person task | needs you | an agent's delegation, a procedure (a gate's approval) or the person | its contract: a choice, an approval at a head, an answer |
| result | updates | a task the person asked for, ended | reading it |
| reply | updates | one of the person's chats | reading it, in the chat |

- **Results only of what the person asked for:** their chats, their
  goals, the tasks they made themselves. What a goal's delegates finish
  goes to the goal, not to the person; it is on the goal's page.
- **What agents decide stays with them.** A proposal or an escalation
  reaches the person only once no task above it could cover it, so the
  inbox holds what only the person can decide.

## 3. Order and filters

- **Needs you,** oldest waiting first: each entry holds work up, and the
  longest waiting holds it up most. Each says how long it has waited and,
  where its stall applies, when it will pass up. This refines the
  "newest first" of `domain/people.md`, section 6, for what needs the
  person.
- **Updates,** newest first, with those not yet read marked as new.
- **Filters:** a project, a kind of entry, a goal (an entry belongs to a
  goal when its task is in the goal's subtree).
- **The count** the web shows everywhere is of what needs the person;
  updates have a quieter count of their own.

## 4. Entries

Each entry is its object's card (README.md, section 6), with a line of
context: its project, and its task's lineage up to its goal or chat
("temper ▸ T1 add OAuth login ▸ T7").

### 4.1 Proposal

What a task may not do alone, waiting for someone who may
(`domain/tasks.md`, section 8; `domain/authority.md`, section 9):

- **who proposes,** as a chip, and **why,** in its words;
- **the action,** by kind:
  - *a batch:* each task it would make, with its executor (a charter, a
    procedure, a person), its budget, its deadline, its dependencies
    among the others ("after T2, T3"), and its grants in words ("push to
    its own branches; land into main in ai/temper"); and whether the
    tasks would be the person's own, as a goal a chat proposes is;
  - *an effect:* what it would do and where ("comment on ai/skein#40"),
    with what it would write;
  - *a widening* of the proposer's own authority: what it has, what it
    asks for ("budget 10 → 25"; "also land into release-1.x");
  - *an amendment* beyond the amender's authority: the task, and the
    change, as before and after;
- **who would fund it:** what it needs (`domain/authority.md`, section 9)
  and from where: "funds 40 from your pool: 120 left this period, 80
  after";
- **where it has been:** passed up by whom, or past which holders that
  could not cover it, and why ("T1 may not land into main").

Its actions:

- **Accept,** when the person covers what it needs; otherwise not
  offered, and the card says what they lack. For the one owner, that is
  usually their pool, which settings can widen (projects.md, section 7).
- **Reject,** with a reason, which the proposer hears and may act on by
  proposing again.
- **Pass up,** when there is a holder above the person; at the people
  the policy names there is none, and it is not offered.

Accepted, the card shows what it made: chips for the tasks of a batch,
"T1 is yours" for a goal, what an effect made once it is made.

```
Proposal from T0, chat "OAuth login?"                         waiting 4 min
  Make T1 "add OAuth login", yours once accepted
    a coordinator; budget 40; land into main in ai/temper; priority 2
  Why: "It touches the web, the people domain and configuration; …"
  Funds 40 from your pool: 120 left this period, 80 after
  Accept · Reject…
```

### 4.2 Escalation

A held task whose escalation reached the person (`domain/tasks.md`,
5.5 and section 8):

- **the task,** as a chip, and **why it is held,** in words, with what
  tells the story: its last failures for one out of tries, its spend for
  one out of budget, who closed its pull request for drift (tasks.md,
  section 7);
- **what it had done:** its phase before the hold, its result if it was
  closing;
- **where it has been:** which holders it passed, and why.

Its actions follow its reason (tasks.md, section 7), from:

- **release,** saying what will happen ("its next run resumes its
  transcript; tries count again from zero");
- **amend, then release,** when releasing as it is would only hold it
  again: a budget widened, a deadline moved, instructions changed, a
  failed dependency dropped;
- **leave it held,** with a reason (the domain's rejection): the entry
  leaves the inbox, and the task stays held where it is shown, saying
  why;
- **cancel** it, and everything below it (tasks.md, section 8);
- **pass up,** when there is a holder above the person.

When the same person holds the current and next role, the engine skips
that self-pass and does not offer **pass up**. The card stays with that
person until they release or leave it held, rather than leaving and
reappearing under their other role. Releasing an out-of-tries hold starts
a new run and counts its tries from zero.

### 4.3 Question

A task the person asked for asks them something (`domain/tasks.md`,
7.1): a coordinator unsure of the scope, a chat that parked waiting for
an answer.

- **the task** and its words, whole, never cut;
- **its context:** the task's page, and what it did just before asking;
- **an answer,** in words, which is a message to the task and wakes it.

A question from a chat shows in the conversation too, answered in
either place.

### 4.4 Person task

A task whose executor is the person (`domain/people.md`, section 8):

- **what is asked:** its spec, and who asked (a task, a procedure, the
  person);
- **the results it depends on,** side by side when there are several: a
  choice between two spikes shows both reports;
- **its contract,** as the form to answer it:
  - *a choice* among the options its spec lists, with words to say why;
  - *an approval at a head:* the change's panel at that head (forge.md,
    section 4), approve, or ask for changes with remarks; the head is
    named, so an approval is never given to a head the person did not
    see;
  - *an answer* in words;
- **its stall:** when it will become news for its requester.

The answer is the task's result: the choice reaches the tasks that
depend on it, an approval the change that asked for it.

```
T4 Choose: library X or a hand-rolled flow           asked by T1, waiting 1 h
  T2's report: library X          │  T3's report: a hand-rolled flow
  "X covers PKCE and refresh; …"  │  "About 400 lines; we own …"
  ( ) library X   ( ) a hand-rolled flow       Why: […]
  Answer
```

### 4.5 Result and reply

- **A result** shows its task's ending and its result by its contract
  (`domain/tasks.md`, section 3): a report's words; a verdict, with its
  fields and the follow-ups it proposed; a change landed, with its pull
  request and merge commit; a choice, and who made it. A failure says
  why; a cancel says why, by whom, and what had been done before it.
- **A reply** names the chat and its first words, and opens the chat at
  the first reply not yet read.

## 5. Deciding

- **In place.** Each entry's actions are on its card; opening it gives
  the task's page with the same card, for more context.
- **Pending** once sent: the card says it is being decided, and stays
  until the answer comes, after the commit (README.md, 5.3).
- **Decided elsewhere first** (another tab, a task above that decided it
  in the meantime, a proposal withdrawn because what it named has
  ended): the card says what happened, by whom, and when, and leaves.
- **Refused:** the card says why, and stays: a reason too long, an
  inbox full, a budget since spent.
- **Changed while read.** A card follows its object live: a head that
  moves under an approval, a proposal passed up, a held task's reason
  that changed. An action names the state it was taken on, and one taken
  on a state since changed is refused, saying so, not applied to the new
  one (`domain/people.md`, 5.1.2: a decision names its exact revision).

## 6. Derived and read

- **An entry leaves** when its object stops waiting: decided, answered,
  withdrawn, its task ended or released, whichever page that was done on.
- **Reading updates** moves the person's read position over results and
  replies (`domain/people.md`, section 6), committed like any request:
  opening the updates moves it to the newest shown, and opening a chat
  moves it over that chat's replies.
- **Nothing else is the inbox's own.** There is no archive, no snooze
  and no flag: what is done is in the history of the task it was about.

## 7. Live, and bounded

- **Live:** an entry appears as the commit that makes it waits for the
  person, through the inbox's stream (README.md, section 7), and leaves
  as it stops waiting.
- **Bounded:** the engine holds each person's entries up to a limit
  (`domain/people.md`, section 6); the web pages through the rest,
  loaded from the store.

## 8. Several people, later

Designed lightly, since the web assumes one person for now (README.md,
section 3):

- **What is addressed to a role** is an entry for everyone holding it,
  saying so ("for maintainers"), until one acts.
- **A person task addressed to a role** is taken first: the person who
  takes it holds it, the others see who did, and it may be handed back.
- **Two people deciding one thing:** the first commit decides; the
  second is told who decided and how (section 5).

## 9. From the domain

| The web | The domain |
|---|---|
| accept, reject, pass up a proposal | deciding a proposal (`domain/people.md`, 5.1; `domain/tasks.md`, section 8) |
| release, leave held, pass up an escalation | deciding an escalation (`domain/people.md`, 5.1.2) |
| amend, then release | an amendment and a release, in two requests, the second sent once the first is answered |
| answer a question or a person task | answer (`domain/people.md`, 5.1 and section 8) |
| reading updates | the read position (`domain/people.md`, section 6) |
| the inbox across projects | `domain/people.md`, section 14, taken as decided |

## 10. Open questions

- **Amend and release as one request,** so that a budget is never
  widened for a task that is then not released.
- **A read position per chat,** so the list of chats shows each one's
  unread replies; the domain keeps one per person.
- **Digests:** a summary of what happened while the person was away,
  from the history of their goals, beyond the updates.
- **Taking a person task** with one person: whether it is needed at all
  before there are several.
