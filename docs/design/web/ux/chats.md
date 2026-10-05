# Chats

Provisional, 2026-10-05. Conversations with agents: how a person starts
one, what the conversation shows, what the chat does in it, and how it
pauses and ends. A chat is an agent's task whose requester is the
person (`domain/people.md`, section 7); its page is a task page
(tasks.md) whose activity, the conversation, comes first. What is still
open is listed in section 11.

## 1. In one page

- **How a person usually starts work.** A chat answers, makes a small
  change itself, delegates, or proposes a goal; all of it shows in the
  conversation as it happens.
- **The conversation is the transcript.** Each turn is committed as it
  ends (`domain/engine.md`, 7.2), so what the web shows is what the
  store keeps; what streams in before a turn ends is provisional.
- **Words reach the chat at once:** a running chat hears them at its next
  turn, a waiting one wakes, a parked one resumes on any worker. The
  person never manages workers or parking.
- **What it does shows inline:** the tasks it delegates as chips, a goal
  it proposes as a proposal card, a change it hands on as a change card,
  each live and each deciding in place what it would in the inbox.
- **Stop, close and cancel are different** (section 7): stop holds it to
  be released; close asks it to finish, and asks the person what becomes
  of its live delegates; cancel ends it and them now.

## 2. The chats

The person's chats, across projects:

- **Each shows** its name, its project, what it is doing in words
  (section 4.3), whether it has replies not yet read, its live
  delegates, and what it has spent.
- **Ordered** by last activity: words, a reply, a delegate's result.
- **Filtered** by project, and by live or closed; closed chats are paged
  from the store below the live ones, their conversations still
  readable.
- **A chat's name** is its first words until the person names it
  (README.md, section 10).

## 3. Starting a chat

- **From the chats:** a project, and the first words. The chat's charter
  is the project's for chats, and its budget the role's default for
  chats (`domain/people.md`, 5.1); the form says both, and asks neither.
- **From what it is about:** a task's page, a held task's card, a goal,
  a finding, each offers to start a chat about it, with the thing named
  in its first words and in its spec, so the chat's brief carries it.
- **Answered** with the chat's number once durable; its page opens, its
  first run queued.
- **Refused,** it says why: the person's pool has less than a chat's
  budget, the project has too many live tasks, a limit.

## 4. The conversation

### 4.1 What it shows

Turn by turn, across every run the chat has had:

- **The person's words,** whole, each saying where it is: being sent;
  sent, once committed to the chat's inbox; read, once a turn that took
  it is committed.
- **The chat's replies:** what the LLM wrote.
- **Its work,** folded to a line each, opened for detail: a tool call
  (what it read, ran or changed, and the outcome), its sub-agents
  (smith's `run.md`, 5.3), a delivery (the branch pushed, and its head).
- **Its calls to the engine, as cards:** a `delegate` as the chips of the
  tasks it made; a `propose` as its proposal card; a `message` to a
  delegate; an `effect` and what it made; a `note` and a link to it; a
  `decide` and what was decided (`domain/engine.md`, 7.3).
- **What it heard besides the person:** its delegates' results as result
  cards, and the news it was woken by.
- **Where its runs begin and end,** as quiet lines: parked; resumed on a
  worker; started fresh from a brief carrying the transcript's tail
  (`domain/engine.md`, 7.2), which tells the person the chat may have
  lost detail of what came before.

Turns are the agent's own vocabulary, rendered below the domain
(`domain/agent.md`, section 7); the web shows what renders and what each
turn spent.

### 4.2 Live and long

- **What streams in** (the text of a turn being written, a tool call
  being made) shows as it comes, marked as in progress, and is replaced
  by the turn once committed; a turn lost with its worker is gone from
  the page, and the chat's next run starts after the last committed one
  (README.md, section 7).
- **Long conversations** load from their end, a page at a time as the
  person scrolls back (`domain/engine.md`, 5.3).

### 4.3 What it is doing

A line always says, in words, where the chat is (`domain/tasks.md`,
5.2):

| The chat is | It says |
|---|---|
| due, preparing or claimed | starting |
| running, writing | writing |
| running a tool | which tool, and for how long |
| running, waiting with its slot (`wait`) | waiting for you |
| idle, parked | parked; your words resume it |
| backing off | retrying, when, and why: "worker lost, try 2 of 3" |
| held | why, with its card (section 8) |
| closing, then closed | closing; closed, and when |

### 4.4 Beside the conversation

The chat's context, as a panel of its page: its project; its delegates,
live and ended, as a small tree of chips; the goals it proposed, now the
person's, by reference; its spend and what it has left; the repositories
its runs work in.

## 5. Words

- **At any time** the chat is not closing or ended: while it runs, they
  are relayed to its run once committed, which reads them at its next
  turn; while it waits, they wake it; parked, they resume it.
- **They do not interrupt** the turn being written. To stop the chat now,
  the person stops it (section 7).
- **Whole:** a person's words are never cut (`domain/tasks.md`, 7.2), and
  are refused only when the chat's inbox is full, which the person is
  told.
- **While it is held,** words wait in its inbox, and its next run hears
  them once it is released.
- **Sent is sent:** words committed are not edited or withdrawn; the
  person writes again.

## 6. What a chat does

As the conversation shows it (`domain/core.md`, 6.3; `domain/people.md`,
section 7):

- **Answers,** in its replies.
- **Makes a small change itself:** it edits its workspace, delivers the
  change to its run's own branch, and delegates a change task that is
  handed the branch (`domain/forge.md`, 8.1). The change card in the
  conversation follows it through CI, its gates and its queue to landing
  (forge.md, section 2), and the chat carries on meanwhile.
- **Delegates:** "review pull request 42 for memory safety" becomes a
  chip in the conversation and in its tree; the delegate's result
  arrives as a message that wakes the chat, which tells the person.
- **Proposes a goal:** a proposal card in the conversation, the same as
  in the inbox (inbox.md, 4.1), saying the goal will be the person's.
  Accepted in either place, the goal is the person's own: it is on the
  board, it outlives the chat, and the chat keeps a reference to it
  (`domain/tasks.md`, section 8).

## 7. Stop, park, close, cancel

- **Parking needs nothing from the person.** A chat waiting for words
  past its charter's threshold parks, freeing its worker
  (`domain/engine.md`, 7.4); the next words resume it from its
  transcript. The web says "parked", and nothing else changes.
- **Stop** holds the chat, now. Say it is reading code in the wrong
  direction, tool call after tool call. The person stops it: its run is
  cancelled at once, what it committed stays, unfinished changes in its
  workspace are saved (`domain/engine.md`, 7.4), and the chat is held,
  stopped by the person. Its delegates go on. The person writes "look at
  the people domain instead"; the words wait, and releasing the chat
  starts its next run, which resumes the conversation and reads them. In
  a chat the person stopped, sending words also releases it: two
  requests, the words first.
- **Close** ends a chat that has done its job, letting it finish. Say T0
  delegated T11, "review pull request 42 for memory safety", still
  running, and proposed T1, the OAuth goal, which the person accepted.
  Closing T0, the web first asks about T11, its live delegate: cancel
  it, or make it the person's, so that T11 goes on, funded from their
  pool from now ("reserves 6 from your pool"), and its result comes to
  their inbox (`domain/tasks.md`, section 6). T1 is listed as unaffected:
  it is already the person's. Then T0 is asked to finish, which wakes it
  to write a last reply, and is cancelled if it has not within a grace.
  Its conversation stays readable.
- **Cancel** ends the chat and every delegate below it now, without a
  last reply: for a chat gone wrong. The web says what it would end (T11
  and its live run) before sending (tasks.md, section 8).

## 8. Held, and spend

- **Held,** the chat's card shows at the end of the conversation, and in
  the inbox, since the person is the chat's requester and the first
  holder of its escalation (inbox.md, 4.2): out of tries, its last
  failures and release; out of budget, what it spent and release with a
  wider budget; stopped by the person, release.
- **Spend** shows beside what the chat is doing: spent, and what is left
  of its budget, warning before what is left falls below the least a run
  may start with (`domain/authority.md`, 8.3).

## 9. The first slice

What the domain builds today (README.md, section 9) gives a chat of one
exchange: the person's first words start it, its contract a report; its
page shows its words, what it is doing, and its report once it ends; a
held chat shows its card, to release or leave held. Words after the
first, streaming and the conversation as a transcript follow as their
routes are built.

## 10. From the domain

| The web | The domain |
|---|---|
| start a chat | start a chat (`domain/people.md`, 5.1) |
| words | write to a task (`domain/tasks.md`, 7.1) |
| stop | stop a run (`domain/people.md`, 5.1) |
| release | release (`domain/tasks.md`, 5.5) |
| close | an amendment asking it to finish, a cancel after a grace (`domain/people.md`, section 7) |
| make a delegate the person's | make themselves a task's requester (`domain/tasks.md`, section 6) |
| cancel | cancel (`domain/tasks.md`, section 6) |
| the conversation | the transcript (`domain/engine.md`, 7.2) and a run's stream (`domain/engine.md`, section 11) |

## 11. Open questions

- **Naming a chat:** by the person, or proposed by the chat itself;
  neither is a request yet.
- **A chat about a live task:** whether a chat the person starts may hold
  a reference to a live task (`domain/tasks.md`, 7.5), to message it,
  rather than only naming it in its words.
- **Words beyond text:** files and images; the domain's words are text.
- **A chat with no repository** (`domain/agent.md`, section 10): the
  worker refuses an empty workspace today, so every chat works in one.
