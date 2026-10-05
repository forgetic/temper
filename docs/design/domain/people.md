# People

Provisional, 2026-10-04. How people take part in temper: who they are,
what their role in a project lets them do, what they ask of temper and
what waits for them, the chats they hold with agents, the tasks they
carry out themselves, and how the web shows it all. People are parties,
as tasks are (core.md, section 8); this document is that party in
depth. What is still open is listed in section 14.

## 1. In one page

- **temper is where people work.** People talk to agents, set goals,
  accept proposals, answer questions, steer and watch through temper's
  web. They may read the forge, which temper keeps readable, but need not
  act there, and temper does not read what they write on what it owns.
- **A person is known by signing in through the forge.** temper keeps who
  they are; it does not act on the forge as them.
- **Roles are per project,** temper's own, seeded from the repositories'
  permissions when a repository is adopted. A role is an authority and
  the proposals it may decide (authority.md, section 6).
- **Requests are decisions.** Starting a chat, setting a goal, writing to
  a task, accepting a proposal, stopping a run: each is checked against
  the person's role, committed, and answered once durable, keyed so
  that a request sent twice is made once.
- **A person's inbox** holds what waits for them: questions, proposals,
  escalations, the tasks they carry out, results and replies. What is
  addressed to a role waits for every person in it until one acts.
- **A chat** is an agent's task whose requester is a person: each of their
  words wakes it, and it resumes its transcript. It is how a person
  usually starts work.
- **The web is generic.** It knows tasks as a tree, conversations, what
  waits for a person, and live runs; not the shapes of work built from
  them.

## 2. Structure

`temper-engine-domain-people` is a child domain of the engine's root
(engine.md, section 3), a capability. It keeps:

- **people:** the identities of those signed in or holding a role, and
  their sign-ins;
- **roles:** each live project's roles and who holds them, loaded at
  start and changed by commits;
- **requests in flight:** each person's pending requests, by their keys,
  until answered;
- **inboxes:** for each person with something waiting, what waits, by
  reference to the task or proposal it is (section 6);
- **the web's live connections,** to which it pushes what changes in
  their inboxes, through the views (engine.md, section 11).

It knows nothing of tasks beyond their numbers and what the root tells
it waits for whom; every request goes to the root, which routes it to
authority and to the tasks.

## 3. Identity and signing in

- **Signing in through the forge.** The web sends a person to the forge's
  OAuth authorisation and back; the protocol layer exchanges the code
  and reads who they are. The domain sees a person: the forge, their user
  id there, and their login and name for display.
- **A person record** is made the first time someone signs in, and kept.
  Signing in gives temper no right to act on the forge as that person:
  the token the exchange returns is used to read who they are, and
  dropped (section 14 keeps the alternative open).
- **Sign-ins** are kept in the store, so a restart signs nobody out:
  their tokens, or digests of them, are written by the web's protocol
  layer into the store's secret records, and the domain names a sign-in
  only by its number (engine.md, 5.4). Each expires after a configured
  time, and a person may end theirs.
- **The first owners** of a deployment are named in its configuration;
  everyone else gets a role by a project's owner, or by adoption's seed
  (section 4).

### 3.1 Person numbers and the initial owners

The root owns deployment numbers (`engine.md`, section 4). For a sign-in
it supplies a fresh candidate person number alongside the forge identity;
people uses it only for a new `(forge, user id)`. An identity already known
keeps its durable number. Unused candidates leave gaps, and numbers are
never reused. The root issues sign-in numbers; they likewise name one sign-in
only. Login and display name may change; neither identifies a person.

Configuration names the initial owners by `(project, forge, user id)`.
The project's roles are initialized before signing in. When an identity's
first person record is made, people grants those configured roles and
saves them with the person and sign-in in the same decision. It reserves
room for every matching project first: a full project refuses the sign-in
without making any record. Later sign-ins never regrant a role an owner
changed. Configuration bounds the matches, and `max_out` includes every
possible bootstrap role record.

Sign-in expiry is saved as a wall time. Admission computes its monotonic
deadline once; restoring computes it again from the saved wall time and
the startup environment. Requests reject expiry by either the saved wall
time or the current sign-in's monotonic deadline, so a backwards wall
clock movement cannot extend a sign-in within one process.

## 4. Roles

Each project has its roles, and its policy says what each may do
(authority.md, section 6). The default roles:

| Role | May |
|---|---|
| owner | everything a maintainer may; change the project's policy and roles; adopt repositories |
| maintainer | start and steer any task in the project; decide any proposal within the project's ceiling; release, cancel, stop |
| member | start chats and goals within the allotment their role gives; steer and cancel what they started; answer what is addressed to them |
| observer | read and watch |

- **A role is an authority** (authority.md, section 3): what a person in
  it may give a task they create, how much of the project's spend they
  may allot, and which kinds of proposal they may decide. The policy may
  narrow or widen each, within the deployment's rules.
- **Seeded at adoption.** When a repository is adopted, its collaborators
  are given roles from their permission on it: admin as maintainer, write
  as member, read as observer, unless they already have a role in the
  project. The person who made the project is its owner. After that,
  roles are temper's own: a permission that changes on the forge does not
  change a role (section 14).
- **Proposals past the root** of a tree go to the role the policy names
  for that kind of proposal (tasks.md, section 8), maintainers by
  default.

## 5. What a person does

### 5.1 Requests

Every request is keyed by the web (a person's double click makes one
request), checked against the person's role (authority.md, 8.4),
committed, and answered once durable (engine.md, 5.2):

| Request | Needs | Becomes |
|---|---|---|
| start a chat | member | a task with a chat's charter, its requester the person, its budget the role's default for chats (section 7) |
| set a goal | member, within their allotment; beyond it, a proposal | a tracked task, with the charter, budget and priority asked for |
| write to a task | the task in their tree, or maintainer | words in its inbox (tasks.md, 7.1) |
| answer | addressed to them or their role | an answer to a question, or a person task's result (section 8) |
| decide a proposal or an escalation | waiting for them, as the requester of its tree or in a role the policy names | accepted, if their authority covers it; rejected, with a reason; or passed up (tasks.md, section 8) |
| amend, cancel, release | the task in their tree, or maintainer | tasks.md, sections 5 and 6 |
| make themselves a task's requester | they may amend it | tasks.md, section 6 |
| stop a run | the task in their tree, or maintainer | its run cancelled and its task held for a person's stop |
| prioritise goals | maintainer | the goals' priorities amended |
| adopt a repository; change the policy or roles | owner | the project changed (5.2) |
| edit or delete a note | the note's scope within their role | the note revised (engine.md, section 10) |
| watch | observer | a live view (engine.md, section 11) |

- **A request refused** says why: the role, the authority it lacked, a
  limit, a task since ended.
- **Two people deciding one thing:** the first commit decides; the second
  is told it was decided, by whom, and how.
#### 5.1.1 Keyed admission and answers

Keys are scoped by person, across that person's sign-ins. A valid sign-in
asking again with the same key and the same typed request gets its saved
answer; current roles do not remake a decision already made. Reusing a
key for a different request is refused as a key conflict. An in-flight
copy joins a bounded group of reply destinations, with one route to the
root; a full group answers busy.

Admission reserves room for the eventual answered-key record before it
routes. Completed records plus reservations cannot exceed the limit, so
an outcome never fails halfway through for lack of key space. A request
refused by its role saves that outcome too. Admission busy, invalid sign-in,
oversized input and key-conflict answers make no record and change no
state. Busy admission may be retried.

The root routes and closes the decision in one step (`engine.md`, section
4), saving the task changes and the keyed outcome in one commit. People
emits the answer immediately; its parent holds it until that commit is
durable (`engine.md`, 5.6). A root temporarily waiting for a load keeps
the flight volatile and makes no external mutation until it can save the
outcome with the decision. A crash before durability loses both task and
key; one after durability loses neither, even if the reply had not left.

Increment 03a retains completed keys up to a configured capacity; timed
retention and paging follow in 03c. Its `Ask` contains `StartChat` only.
Other requests in 5.1 gain their full typed vocabulary and behavior in
the increments that implement them.

### 5.2 The project itself

A project, its repositories and roles, and its policy are records in the
store that owners change through requests like any other. A policy
change applies to what is decided after it; it never revokes what was
already given to a task, which its requester may amend.

## 6. A person's inbox

What waits for a person, newest first in the web, each entry naming the
task or proposal it is:

- **questions** from tasks they asked for, chats included;
- **proposals** routed to them, or to a role they hold;
- **escalations** that reached them: a held task nobody below could
  decide;
- **person tasks** addressed to them, or to a role they hold (section 8);
- **results** of the tasks they asked for, and chats' replies they have
  not read.

- **Derived from the tasks,** not kept apart: an entry is there while its
  proposal is pending, its person task is active, its question is
  unanswered, or its result unread. The only state of the inbox's own is
  each person's read position over results and replies, committed when
  they read.
- **What is addressed to a role** is in the inbox of every person holding
  it, until one of them acts, when it leaves every one.
- **Bounded:** the people child domain holds each person's entries up to a
  limit; the web pages through the rest, loaded from the store.

## 7. Chats

- **A chat is an agent's task** whose requester is a person and whose
  charter is a chat's (core.md, section 8): the project's instructions
  for chats, its models, the tools its role gives, and a wake policy that
  wakes on the person's words and on its delegates' results.
- **Turn by turn, live.** The person's words wake it, or reach its live
  run at once; its replies stream to the web as the run reports them, and
  each turn is committed as it ends (engine.md, 7.2), so the conversation
  in the web is the transcript in the store.
- **Waiting, then parked.** A chat waits for the next words holding its
  worker's slot; idle past its charter's threshold it parks, and the next
  words resume it from its transcript, on any worker.
- **What a chat does:** answers; makes a small change itself and hands it
  to a change task (core.md, 6.3); delegates; or proposes a goal, which
  its person accepts in the conversation, as a proposal in their inbox,
  and which is theirs once accepted, not the chat's (tasks.md,
  section 8).
- **Closing.** A person closes a chat, which asks it to finish (an amendment
  it is woken by) and cancels it if it has not within a grace. A chat
  with live delegates is closed only once the person says what becomes of
  them: cancelled, or made their own (tasks.md, section 6).

## 8. Person tasks

A task whose executor is a person, or a role (core.md, 3.2):

- **Made by** an agent's `delegate` (a choice between spikes' reports), a
  procedure (a person's approval as a gate, forge.md, 8.3), or another
  person.
- **Its contract** is a choice among the options its spec lists, an
  approval at a state its spec names (a pull request's head), or an
  answer in words.
- **In the web,** it shows its spec, the results it depends on, and what
  its contract asks; the person's answer is its result.
- **Addressed to a role,** it waits for any person holding it; one who
  takes it holds it, so two people do not answer at once, and may hand
  it back.
- **Stalled,** past its bound, it is news for its requester, which may
  amend, cancel or address it to someone else.

## 9. Notifications

- **In the web,** live: a signed-in person's inbox changes reach their
  open web connections through the views, as they commit.
- **Beyond the web,** later: e-mail or a chat system, each as a connector
  whose effects are messages to people, keyed and made once like any
  other (connectors.md). Until then, a person learns of what waits by
  opening the web.

## 10. People on the forge

- **The forge stays readable.** Issues show goals and their milestones,
  pull requests their changes (forge.md, section 12), for people who
  prefer to read there and for the history if temper is unplugged.
- **What people write on what temper owns is not read.** A comment on
  temper's pull request is not an input; the pull request's body says
  where to reach temper instead.
- **Outside contributors** are not temper's people unless they sign in.
  What they write on participating objects is heard by the task that
  deals with the object, as a participant's words (forge.md,
  section 13).

## 11. Below the domain

What the web's protocol layer owes this domain:

- **HTTP** for requests and pages, **live streams** (server-sent events)
  for views and inbox changes, and JSON both ways, on skein's machines.
- **Signing in:** OAuth with the forge as the authorisation server; the
  sign-ins' tokens, held by the protocol layer, named in the domain by a
  sign-in's number.
- **Rendering** what the domain carries as bytes: transcripts' turns,
  results, specs.
- **Keys** for requests, made by the web page, carried with each request.

## 12. The world

The people child domain has its world, `tests/engine/people`, with its
parent scripted. In the engine's world (engine.md, section 15), scripted
people sign in and act through the web: a member starts a chat, which
proposes a goal they cannot fund, routed to a maintainer who accepts
it; a person answers a choice between two reports; two maintainers
decide one proposal, and the second is told; a person stops a run and
releases it; an observer is refused a request; a request sent twice is
made once across a restart.

Its referee: every request is answered once, after its commit; no
request is acted on beyond the person's role; what waits for a role
leaves every inbox once one person acts on it; a person's words reach
their task, or the task ends.

### 12.1 What is built in increment 03a

`tests/engine/people` runs identities, sign-ins and expiry, authoritative
project-role updates, initial-owner bootstrap and keyed `StartChat`. Its
scripted root checks the routed role and authority, makes a task, commits
the task and people's saved answer together, and withholds replies until
durability. Scenarios restart independently before durability and after
durability before a reply, as well as during a transient root wait.

The referee observes client replies and durable task creations: one reply
per call, no reply before its commit, no observer routed and no key making
two durable tasks. The world checks bounded duplicate waiters, key
conflicts, capacity refusals, cold restoration, facts changing nothing,
replay and memory. Its fuzzy matrix reaches every configured ending.
Inboxes, person tasks, adoption and timed key retention remain later
increments.

Admission pressure reported by the root (`Busy` or `NotReady`) completes
the pending call and every duplicate waiter without saving a completed
answer. It releases the reserved answer slot, so the same key can retry
when tasks or another child has room. A decided result or permanent
refusal remains keyed and durable.

## 13. From today

- **No web exists below the domain today.** The engine's domain takes
  people's requests, but only the worlds make them: there is no HTTP
  server, live stream or signing in yet. People also reach temper on the
  forge today, with comments and labels.
- **People's requests** are each checked today against the person's
  permission read from the forge. Opening a session (today's chat) creates a forge
  issue; a message is written as a forge comment on the person's behalf;
  acceptance is of an item held for it. All of that becomes requests on
  tasks and proposals in the store, checked against roles, in a child
  domain of its own instead of the engine's top level.
- **The hand-in label** goes: handing temper an issue is a person
  starting a chat or a goal that names it.
- **New:** the people child domain, roles, inboxes, person tasks,
  notifications in the web.

## 14. Open questions

- **A person's approval as a rule:** whether temper alone enforces it, or
  posts the approval as the person with the forge credential they signed
  in with, so that protection requires it independently. The second
  needs temper to keep that credential, which today it drops.
- **Notifications beyond the web.**
- **People across projects:** a deployment-wide view of every inbox a
  person has, and roles that span projects.
- **Guests:** someone without a forge account, invited to one chat.
