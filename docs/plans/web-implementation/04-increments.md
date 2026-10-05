# Increments

Provisional, 2026-10-05. The increments of README.md, section 7, in
order: what each lands in the domain, the view and the worlds; the
stories that show it; what its joint-world stories need of the engine;
the design documents it updates. Each is a branch that passes the gate of
`docs/development/workflow.md` and an independent review before it
merges (`docs/plans/next-domain/README.md`, 5.6 and section 8), and adds
no vocabulary it does not use (README.md, section 6).

## W1 The domain's spine

**Lands** `temper-web-domain` with the parts every page stands on
(01-domain.md, section 2): `boundary`, `action`, `ask`, `address`,
`domain`, `frame`, `link`, `streams`, `reads`, `requests`, `drafts`,
`notices`, `saved`, `facts`, `limits`, and `pages` with `Starting`,
`SignIn`, `Missing` and `Chats`.

- **Vocabulary:** every `Event` and `Request`; `Action::Go`, `Edit`,
  `Submit`, `SignIn`; `Ask::StartChat` and `Outcome::Started`; the
  refusals `Role`, `Limit`, `Ended`, `Unknown`, `KeyConflict`; `Query::Chats`;
  `Watch::Person`; `Address::Inbox`, `Chats` and `Task` (a task's address
  shows `Missing` until W4); `FieldRef::NewChat`.
- **Behaviour:** starting from session storage (requests sent again with
  their keys, drafts restored) and opening the frame's watch; the link's
  states, heartbeat and backoffs; signed out to the sign-in page, requests
  parked; the chats page: its composer's draft, starting a chat as a keyed
  request kept before it is sent, answered with its number and going to
  its address; the person's chats as a window of lines, live or closed.
- **Tests:** the step tests of 01-domain.md, section 10, for these parts.
- **Updates** `architecture.md`, section 6: a request is kept in session
  storage before it is sent; the frame's watch is always open and carries
  the link's state.

## W2 The view's spine

**Lands** `temper-web-view` (02-view.md, section 2): `tree`, `builder`,
`binding`, `diff`, `render`, `limits`, `words` for what these pages say,
`frame`, `notices`, and `pages` with the starting, sign-in, missing and
chats pages, after `chats.html` and the mockups' top bar.

- **Tests:** the step tests of 02-view.md, section 10, for these parts;
  the diff's property over generated trees; decoding every binding.
- **Measures** building and diffing a chats page at its limits in a debug
  build, and records it here; if it is too slow for the worlds, the
  per-section build of README.md, section 8 is designed before W4.
- **Updates** `architecture.md`, section 4: the browser holds node ids,
  and bindings stay in the view's last tree; a press on a node gone from
  it is dropped as stale.

W1 and W2 run side by side: W1's first commit fixes `Action`, `FieldRef`,
`Address`, `Frame` and the chats page's state, which W2 reads.

## W3 The person and the client's world

**Lands** `testing/temper-fake-person` (03-testing.md, section 2: scripts,
the tree face, the random person) and `tests/web/domain` (section 3):
tabs, the shell's part, the model DOM, the scripted engine with people,
projects, chats and keyed starting, the referee's rules that apply so far
(safety 1, 2, 4, 6, 7; liveness 1, 2), and every fault but those of two
tabs and stale presses, which need cards (W4).

- **Stories** (`slice.rs`, `faults.rs`): signing in and starting a chat;
  starting one across an engine restart before durability (made once
  after the client sends it again) and after it (answered by its key); a
  reload while the request is in flight; a double press; the words of a
  composer kept across a reload; offline and back, the requests parked
  then sent; the sign-in expiring mid-request, signing in again, the
  request sent again with its key.
- **Also:** `referee.rs`, `memory.rs`, `fuzzy_web.rs` with the random
  person.
- **Measures** the world's focused and fuzzy cost, and sets each story's
  seeds from it (03-testing.md, section 7).
- **Updates** `docs/design/testing.md`, sections 4.4 and 7 (03-testing.md,
  section 8).

## W4 The first slice

`ux/README.md`, section 9: a chat of one exchange, and a held chat's
escalation decided.

**Lands** in the domain: `objects` with `Chip`, `Escalation` and
`TaskResult`; `Page::Task`, led by its conversation in its first form
(the chat's words, its phase, its report once it ends, its held card);
`Watch::Task`; `Query::Escalation` and `Query::Result`, for a page
opened after the task ended; `Ask::Decide` for escalations (release,
reject with a reason as leaving held, pass where there is a holder
above); `Outcome::Decided` and `DecidedBefore`; the refusals
`Authority`, `Moved`, `Standing`, `NoFurther`, `NeedsAmend`; the
confirmation (`Intend`, `Confirm`, `Dismiss`, `FieldRef::Reason`); cards'
states and their linger. In the view: `markdown`, `confirm`, the task
page's header, overview and conversation in their first form, and the
chip, escalation and result cards.

- **Stories:** a chat's page shows its words and phase, then its report
  once it ends; a held chat released from its page; left held with a
  reason; two tabs deciding it, the second told by whom (safety 3); its
  revision moving while the confirmation is open; a press on its card
  after it left; the report read after a reconnect; a result in Markdown
  shown in the subset, the rest as text.
- **The faults** of two tabs and stale presses join the sweep.
- **Updates** `ux/chats.md`, section 9, and `ux/inbox.md`, 4.2, with what
  the slice settles (passing up while the one person is both, the
  wording of a release).

## W5 The joint world

**Lands** `tests/web/engine` (03-testing.md, section 5): the translation,
the first slice's stories against the real root, the referee unchanged,
`fuzzy_slice.rs`.

**Needs of the engine,** beyond the routes it has (README.md, section 5):

- **who is signed in,** as a watch: the person, their projects and roles,
  the inbox's counts (zero until the inbox is built), for the frame;
- **a task's watch,** or at least its read: its chip, header and phase,
  its held escalation, its result once ended, for the chat's page.

W5 is scheduled when the engine has both. Should they come late, the
increment may start with the client's requests and reads alone, and the
frame's snapshot made by the translation from the root's sign-in answer
and the configured projects: a stand-in in the world, named as such, and
deleted when the route lands.

**Updates** `ux/README.md`, section 10, with anything the joint world
finds missing that it does not list.

## W6 The inbox

`ux/inbox.md`. **Lands** `Page::Inbox` (needs you, oldest first; updates,
newest first; filters by project, kind and goal), the proposal,
question and person task objects and cards, the refusal card,
`Ask::Decide` for proposals (accept, reject with a reason, pass up),
`Ask::Answer` (a choice with why, words; approvals at a head wait for
W10), `Ask::Read` for the read position, `Watch::Inbox`, `Query::Inbox`
for what is past the engine's limit, `Action::Choose`, `Filter` and
`More`, and the frame's counts. Home becomes the inbox.

- **Stories:** a proposal accepted in the inbox leaves the chat open in the
  other tab (one object, one card); rejected with a reason; a choice
  answered with why, its two reports side by side; decided elsewhere first
  (another tab, a task above), the card saying by whom and leaving; refused
  for a pool since spent, staying with its reason; beyond the person's
  authority and proposable, offering to propose; a proposal passed up
  while read; updates marked new until opened, the read position moving;
  paging past the window.
- **Needs of the engine:** the inbox as a watch, across projects
  (`ux/inbox.md`, section 9); deciding proposals; answering questions and
  person tasks; the read position.

## W7 The conversation

`ux/chats.md`, sections 4 to 8. **Lands** the transcript
(`task/transcript.rs`: a window of turns from the end, runs and their
headings, the provisional turn from `Watch::Run`, folds), words and
their states (sending, sent, read), each task's composer and its
draft, inline cards (a delegate's
chip, a proposal, a result), the chat's line in words, its spend;
`Ask::Write`, `Stop`, `Release`, `Close` (with each live delegate's
disposal), `Cancel`; words then release for a stopped chat, as two
requests; `Query::Transcript`. In the view, the conversation and the
activity-led page of `agent-task.html`.

- **Stories:** words sent, then sent, then read; a reply streamed, then
  replaced by its committed turn; a worker lost mid-turn, its streamed
  text gone; stopping a chat, writing, releasing; closing a chat with a
  live delegate, cancelled or made the person's; cancelling, told what it
  ends; scrolling back through earlier pages; typing in the composer while
  turns stream in, its value never written over (the model DOM shows no
  `Value` patch).
- **Needs of the engine:** a run's watch; transcripts read a page at a
  time; writing, stopping, releasing, closing and cancelling a task;
  making oneself a delegate's requester.

## W8 The task page

`ux/tasks.md`. **Lands** the universal page's parts: header with its
lineage, overview, activity grouped by run, the plan as a tree
(`task/tree.rs`: folds, counts per phase, held below, held only and live
only), history for the task or its subtree, budget and authority; which
requests fit the phase (`task/steer.rs`); `Ask::Amend` with its before
and after, `TakeOver`, amend then release; release by each hold's reason;
`Watch::Tree`, `Query::History`, `Query::Ended`. In the view, the
plan-led page of `task.html`.

- **Stories:** a task held for each reason of `ux/tasks.md`, section 7,
  saying why and offering what fits; amending, then releasing; a plan
  with many tasks folded into counts, a held task deep down counted on
  its folded line; an ended branch opened and loaded; history paged, for
  the task and its subtree; taking a delegate over before cancelling its
  parent; where a budget went.
- **Needs of the engine:** a tree's watch; history and ended delegates
  read a page at a time; amending and taking over; funders' numbers.

## W9 The board

`ux/projects.md`, section 3. **Lands** `Page::Board` (the period's
numbers, goals by priority, live, waiting and recently ended), raising
and lowering a goal as a pending request (`Ask::Prioritise`), setting a
goal (`Ask::SetGoal`, a form with `Action::Number`, beyond the person's
pool said before it is sent), `Watch::Goals`.

- **Stories:** a goal raised, pending, then in place; a goal set within
  the pool; one beyond it, refused with what is left, or proposed; the
  board's filters; a goal's held count following its plan.
- **Needs of the engine:** a project's goals as a watch; prioritising;
  setting a goal.

## W10 The forge's panels

`ux/forge.md`. **Lands** `forge.rs` (01-domain.md, section 9): a change's
body for its card and its panel (its way to landing, pull request, CI,
gates and the head each was given at, heads, bounds); `Page::Changes`
(each landing branch's queue and rule, not ready, held, the branch's
health, landings paged); the task page's `Panel` with its forge variant;
approvals as person tasks at a head, replaced when the head moves;
amending a change's gates; drift's card and what release does;
`Watch::Changes`, `Query::Landings`.

- **Stories:** a change followed step by step to landed; an approval at a
  head; the head moving while the card is open, the card saying it was
  replaced; the queue's order following a goal raised on the board; a
  paused queue and its repair; a drifted change released.
- **Needs of the engine:** the forge connector's reads of changes, queues
  and branches (`docs/plans/next-domain/04-forge-connector.md`); a
  project's changes as a watch; answering approvals.

## After W10

A project's tasks, findings, notes, recurring tasks, settings and spend,
and the system's page (`ux/projects.md`, sections 4 to 8), planned here
once the board is built. Several of their requests are not in the domain
yet (`ux/README.md`, section 10), so their plan starts with what the
engine will offer.
