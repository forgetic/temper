# The engine's model layer

Provisional, 2026-10-02. What the temper engine does, as a model layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of `programming-style.md`; the worker it drives is
described in `worker-model.md`, and the agent in `agent-model.md`. Each
part's details are settled as it is built and kept in its crate's
documentation; this document keeps the decisions and their reasons.
What is still open is listed in section 15, and what is not built yet
in section 16. How the engine is tested is in `testing-pyramid.md`.

## 1. In one page

- **The engine is where work is decided.** It is the forge's only API
  client and the one place plans and rules live: it decides which run to
  start, what a run's outcome changes on the forge, when to try again and
  when to ask a person. Workers host runs; agents do the LLM work.
- **The forge is the truth, and the engine keeps only caches.** What must
  outlive the engine's process is on the forge: each item's record, the
  outcomes being applied, the transcripts. The engine's store holds
  snapshots of parked runs, traces and, later, a cache of live work, all
  rebuildable or expendable. A restart is a rescan of live work, never of
  the forge's history.
- **Everything is an item.** An item is an issue or pull request the
  engine tracks, with a record of its own, an inbox, and at most one live
  run. Chat sessions, tasks, goals and a plan's steps are all items; what
  differs is the step each one carries.
- **Plans are free; primitives and rules are fixed.** Agents compose plans
  as graphs of a few primitives: agent steps, changes, waits and
  sessions. The primitives are the engine's code. The rules are the
  deployment's, and a plan can add to them but never loosen them, which
  is what makes plans written by agents safe to run.
- **Temper learns.** Notes keep what runs and people learn, in the forge's
  wiki, scoped to the deployment, a repository or a goal (section 10).
- **Level-triggered.** What is due follows from the forge's state, not
  from the events that announced it. Webhooks are hints; polling is the
  backstop. Before writing, the engine reads afresh and decides again.
- **Every write is repeat-safe.** Creations are keyed, sets are written as
  sets, and an outcome is recorded before it is applied, so an
  interrupted application resumes where it stopped.
- **The model is complete** (programming-style.md, section 4): a world of
  models and fakes runs everything the engine does, with no protocol and
  no io (section 14).
- **One engine per deployment.** Scaling out is a later problem with known
  solutions. What depends on it now is that the engine is the only writer
  of its own records, so it never needs a compare-and-swap against itself.

## 2. The engine in the system

```
people   chat, accept proposals, steer, release held work: through the web or the forge
engine   the forge's only API client: items, plans, rules; starts runs, applies their outcomes
worker   execution host: checkouts, agent runs, pushing their changes; relays the rest
agent    LLM work: one run per agent process, reporting to the worker
forge    the truth: issues, pull requests, comments, branches, CI, reviews
```

- **Workers dial in** (worker-model.md, section 2). The engine assigns
  runs, sends inbound events and cancels, answers the forge reads and
  outlets a run asks for, and receives facts, parks and ends. It never
  connects to a worker.
- **The engine never reads a repository.** It works from forge state; what
  a checkout holds is for the run to find (agent-model.md, section 2).
- **People reach temper through the engine.** The web is its front door:
  a chat to hold, a proposal to accept, a run to watch or stop, held work
  to release. People also act on the forge directly: a comment, a review,
  a label a plan reads. Either way the engine learns of it as a request
  from a person or a change on the forge, and a person is a forge user.

## 3. Structure

```
temper-engine-model                  the engine loop's entry point: forge, workers, people; routing
├── temper-engine-model-work         items: waiting, due, claimed, running, applying, held, done
├── temper-engine-model-plan         plans: steps, dependencies, gates, wakes, envelopes
├── temper-engine-model-rules        the deployment's rules, which every run and write must satisfy
├── temper-engine-model-forge        the working set and the client: live work, reads, keyed writes, request budget
├── temper-engine-model-fleet        workers: slots, placement, attempts, contact, relaying
├── temper-engine-model-brief        a run's brief, from typed sections within a byte budget
├── temper-engine-model-notes        notes: scopes, entries, the index a brief carries
└── temper-engine-model-views        facts in; live streams and retained traces out
```

The tree follows programming-style.md, 4.5: each sub-model has its own
vocabulary, limits and world; a parent owns its children's state and
routes between them; siblings share no domain types. `work` is the hub,
as `host` is in the worker: it knows an item's lifecycle and nothing of
the forge's API, the workers' channels or a plan's vocabulary. `plan` and
`rules` are policy: decisions over data, which keep nothing between calls
but their configuration, so either can be tested alone. The others are
capabilities. `temper-engine-model` faces the protocol layer, and its
vocabulary is what crosses the model boundary: the forge's operations and
webhooks, the workers' channel, people's requests, and the engine's
store.

## 4. Items

### 4.1 What an item is

- **A tracked issue or pull request** in one of the deployment's
  repositories, from when the engine creates it or takes it in (4.6)
  until its step is done.
- **Its record** is the engine's state for it, in one comment the engine
  owns on the item: a readable panel with a typed record inside. Its parts
  have owners, which the top level composes when it writes a record and
  splits when it reads one: the step the item carries, its progress and,
  on a goal's item, the plan, are the plan's (section 5); the phase, the
  claim and the attempts per failure class, the hub's (4.2); the inbox
  position, and a nonce naming the write that made the record, the forge
  sub-model's (section 12), which knows where each record is and nothing
  of what it says; what it has spent and the relations the forge cannot
  hold, such as a parent in another repository, the top level's.
- **Nothing the forge holds makes the engine panic.** A record that does
  not decode, because a person mangled it, holds its item for a person;
  so does a goal's plan the engine could not have written, such as one
  edited into a cycle.
- **Labels are a projection.** The engine writes labels from the record,
  so the forge's lists show where things stand, and reads only those a
  plan declares as inputs. One label marks every item the engine tracks,
  the index it finds live work by (section 12). It adds and removes only
  the labels it is configured to project, never one a person set.
- **One live run at most:** a supervisor never races itself, and two runs
  never write the same branch.

### 4.2 Lifecycle

```
waiting ─► due ─► claimed ─► running ─► applying ─► waiting, or done
                               │
                               ├─ parks ──► parked ──── a wake ────► due
                               └─ fails ──► a retry after backoff ─► due
                                            or held for a person
```

1. **Waiting** on dependencies, gates or the inbox.
2. **Due:** the plan says the item has work now (5.3), a run or an engine
   action.
3. **Claimed:** the engine writes the claim, naming the run and its
   attempt, into the record before it assigns the run. Attempts count up
   across restarts, so a stale attempt stays fenced.
4. **Running** on a worker, its inbox events relayed and its calls
   served. A run that yields waits for its next event; one that parks
   hands over its snapshot, and the item waits for its next wake.
5. **Applying** the outcome (4.4); then waiting, or done.

A failure is retried after a backoff, within a number of attempts per
failure class, or the item is **held for a person**: it stops being due
until a person releases it. Held is the engine's state, which a label may
show; the engine reads no label for it.

The `work` sub-model, the hub, keeps the lifecycle. What building it
settled:

- **The hub is rebuilt from the records alone.** A record says waiting,
  parked, retrying (and the class), claimed, applying (and the outcome's
  comment), held (why, and the outcome it keeps) or done. Due and running
  are never written, nor is a backoff: what is due is decided afresh
  after a restart, a claimed run is running once a worker says so, and an
  item read retrying backs off again. Records claimed or applying are
  taken first: a claim read is adopted (section 8), an application read
  resumed.
- **Failure classes:** transient, permanent until something on the forge
  changes, the run's own, the agent's, lost, and an outcome the engine
  judged invalid, each with its own retries and jittered backoff. A run
  refused before anything ran (the worker busy, its workstream held, the
  attempt a duplicate) is no failure: the item claims again after a
  pause.
- **An answer is acknowledged once it is durable,** once the record says
  it is being applied, parked or failed. Until then the worker keeps it,
  and its slot. If that record cannot be written, the item is held, still
  keeping the answer, and the slot stays taken until a person releases
  it.
- **Holds keep their outcome.** An outcome not wholly applied (waiting for
  acceptance, its writes failed, its record not written) is applied again
  on release. A hold's reason is typed in the record: the plan's, as a
  code that means the same after a restart; failures of a class; a
  person's stop; acceptance; writes; the record. Held items stay in the
  working set, and count against its capacity.
- **A person's stop** cancels the run and holds the item once the run has
  answered; an outcome it answered with is still applied first (what
  landed is the truth).
- **A mangled record** loses its attempt count: the item counts on from
  the highest attempt its outcomes on the forge name and a worker lists.

### 4.3 The inbox

An item's inbox is what has changed on the forge since its record's
inbox position, among what its step cares about: comments on it, such as
a person's message; its dependencies and children finishing; CI and
reviews on its pull request; the items it subscribes to; a timer.

- **Derived, not queued,** from forge state, so it survives anything that
  happens to the engine. All that is held is where to read from, so what
  does not fit the working set's bounded inbox stays on the forge. The
  position moves only past what a run took, so a failed run's events go
  again to its retry.
- **Relayed or woken.** A live run gets inbox events as they arrive;
  events the fleet could not deliver yet are kept and relayed again once
  the run is placed. An item without a run is woken by them, as its
  step's wake rule says (5.4).
- **What is news,** as built: comments after the position on the item and
  its pull request, save the engine's own; and its pull request's head,
  CI, state and verdicts, as a level against what the position took. A
  dependency the engine does not track leaves no news: it is read afresh
  when a decision needs it.

### 4.4 Outcomes

- **Recorded, then applied.** A run's outcome is posted on its item first,
  a readable account with a typed block inside: the audit trail and the
  durable intent at once. Its words (a report, a reply, an escalation, a
  verdict's reasons) are there; the writes carry none of their own.
- **Applied against fresh state.** The engine reads afresh what the
  outcome touches, asks the plan what it writes, checks each write
  against the rules, and makes them. The run judged the outcome against
  its spec (agent-model.md, 4.4); the engine judges it again against the
  forge as it is now, and one that no longer fits is stale and not
  applied.
- **Keyed, with one commit point.** Every creation carries a key derived
  from the outcome, so a second attempt finds what the first made; sets
  (labels, reviewers, dependencies) are written as sets. The record's
  update comes last and is the commit point: until it lands, the outcome
  is still being applied, and a restart resumes it.
- **Held for acceptance** when the rules say so (section 7): nothing of a
  proposal is applied until a person accepts it.
- **What may have happened is found, not repeated.** Only a request that
  provably never reached the forge did nothing; any other failure may
  still land until a configured lifetime after it went out. Such a write
  is looked for before it is tried again (a creation by its key, a merge
  by its head, a record by its nonce), its item's writes wait out the
  lifetime, and a new engine makes no call until a lifetime after it
  starts. After a restart, an outcome's keys are looked for from after
  its comment, the one bound an earlier life leaves.
- **Records and wiki pages are read before they are written,** and one a
  person changed since is not written over. Each carries a nonce of its
  write, so a read after an uncertain write tells its own landing from
  someone else's change.

### 4.5 Engine actions

Some steps need no LLM: merging a pull request whose gates hold, closing
an item whose children are done, opening the pull request for a pushed
change. The engine takes these itself, as writes planned and checked like
an outcome's, with the same keys and commit point. As built: opening a
change's pull request (keyed by its branches: the newest pull request for
them, open or not, is the one made), reopening it, merging it at exactly
the head the decision saw, closing an item and deleting a branch.

### 4.6 Taking work in

Work starts in three ways: a person opens a session from the web; a
person hands an issue to temper, with a label the deployment names (later
a mention); or the engine creates the item itself, applying a plan or an
outcome. An issue handed in becomes a session (section 6), whose agent
answers it, makes the change, or proposes a plan. From then on the record
says what an item is; labels never decide it again.

## 5. Plans

### 5.1 Primitives

A step is one of a few primitives, which are the engine's code:

- **Agent step:** a run with a charter (agent-model.md, 4.1). Its outcome
  is data, such as a verdict or a report, and may add steps to the plan,
  within its envelope (5.2).
- **Change:** a change to one or more repositories, through the
  pull-request lifecycle. A run produces it and the worker pushes it; the
  engine opens the pull request; CI runs on its head; it is reviewed, by
  an agent step or a person as the step says, never below the rules; runs
  repair it when CI fails, a review asks for changes or its base moves;
  and the engine lands it when its gates hold.
- **Wait:** on other steps, on a person's decision, on a time.
- **Session:** an agent with an inbox, which yields and parks instead of
  ending (section 6).

Gates are conditions over what the forge shows: CI passing on an exact
head, the reviews a step asks for, dependencies landed, a person's
acceptance, a checks-only run passing on a head. As built, a step's own
gates are a number of people's approvals on the exact head and a
person's acceptance of the step; the rules check them, and have gates of
their own (read-only, green CI, a review at a permission, a run's and a
goal's spend), which the top level derives from the step.

### 5.2 A plan

- **A graph of steps** under a goal, each with its primitive, its spec,
  its dependencies and the gates it adds. A step's brief carries the
  outcomes of the steps it depends on, which is how results flow.
- **Written by agents,** as an outcome: a session proposes one, an agent
  step extends one. Nothing requires work to follow a fixed process.
- **Checked before it exists:** known primitives only, no cycles, a
  bounded size, writes only to the deployment's repositories, an estimate
  within budget. A failed check is feedback for the run that wrote it.
- **Accepted when the rules say so** (one that lands on a protected
  branch, or spends past a threshold); then its items are made in one
  repeat-safe application, each with its step in its record.
- **An envelope** says how far an accepted plan may grow on its own: how
  many steps of which primitives, into which branches and repositories.
  Growth within it needs no new acceptance; growth beyond it does.
- **Templates** are plan shapes and step charters that have worked, which
  an agent may reuse or ignore: they gate nothing.

An example, as a session might propose it once it has agreed the work
with a person:

```
plan: add OAuth login                       goal: the session's issue; budget: 3M tokens
  spike-a   agent    "prototype with library X; report fit and risks"   read-only
  spike-b   agent    "prototype a hand-rolled flow; same report"        read-only
  decide    wait     after spike-a, spike-b: the session compares, a person chooses
  design    change   after decide: "design doc under docs/"             review: a person
  build     agent    after design: "split the design into changes"
                     may add: 1 to 5 changes into feat/oauth, with dependencies
  e2e       agent    after build's changes: "run the login scenario"    checks only
  land      change   feat/oauth into main, after e2e                    review: a person
```

The two spikes run in parallel; the session wakes when both have
reported, and asks. `build` adds four changes, within its envelope, so
nobody is asked again; each goes through CI, review and landing, and one
whose CI fails gets a repair run whose brief holds the failing output.
Landing on main waits for green CI on the exact head and a person's
review, because the rules say so, whatever the plan says.

What building the plan settled:

- **A step is done only once the steps it added are,** so what comes after
  it waits for those too, and the checks order the whole graph, added
  steps included. A plan's items are made dependencies first, and a step
  waits until every dependency it names is found.
- **Growth is repeat-safe.** It writes the goal's record and the growing
  step's, which may land in either order across a restart: applied again,
  growth finds its steps already joined to the goal (the same names,
  after the same steps, added by the same run) and asks for the same
  writes.
- **An accepted growth widens the envelope** by what it added. A
  supervising session's tasks join its goal's plan within its envelope and
  budget, so tasks are no way around it.

### 5.3 What is due

The `plan` sub-model is pure: given an item's record and what the working
set holds about it and its relations, it says what is due now, whether an
inbox event wakes the item, what a run's charter is, and what an outcome
writes. Everything it needs is in the record and the forge, so a restart
loses nothing it knew. What building it settled:

- **A change's way to landing.** No branch yet: a run produces it. A
  branch and no pull request: the engine opens one. Open: a conflict or a
  moved base asks for a rebase run; failed CI or changes asked for on the
  head, a repair run; CI pending or a mergeability not yet known waits;
  a clean head whose reviews and gates hold is merged at exactly that
  head. Merged, the step is done; closed unmerged, it is held.
- **Repairs and rebases are bounded apart,** each held past its limit;
  the rebases' is higher, since a busy base moves often and that is no
  fault of the change.
- **Waits on the forge are bounded:** a change that waits for CI, a review
  or mergeability past a configured stall, counted from its head's push
  or its last release, is held, its goal's session told first.
- **A release lifts what it releases:** repairs and rebases count again
  from it, and a decision made before it counts for nothing, so the item
  is not held again at once for the same cause.
- **A claim says why.** A run is claimed with a progress write recording
  why it runs (an agent step's work, producing a change, a repair and its
  cause, a review at a head, a session's turn) and when, so applying its
  outcome, and a session's next wake, need nothing kept in memory. Every
  outcome applied clears it, last.
- **Sessions end** with an outcome that finishes them, or when a person
  closes their item; a supervising session is done once its goal's steps
  are. An item a person closes is done, whatever its step.
- **Time** in the record and in what the plan reads is the engine's clock;
  the forge's times are mapped onto it as they are read (section 13).

### 5.4 Wakes

Each step says what wakes it: its own changes, its dependencies or
children, the items it subscribes to, a person's message, a timer. A wake
rule may batch, letting events through once there are N or the oldest is
older than T, so a supervisor watching twenty changes wakes once for a
burst. As built, only sessions have wake rules; every other step is asked
what is due whenever what it reads changes. A person's message is never
batched, since a person waits for the answer, nor lost behind events
from sources the rule does not name.

## 6. Sessions

A session is an item whose step is a session: a conversation with a
person, and, once a plan it proposed is accepted, the supervisor of that
plan's goal.

- **Chatting.** One live run holds the conversation in its agent process:
  it yields after each reply and waits for the next message. Idle past a
  threshold, it parks: its snapshot goes to the engine and its slot is
  freed. The next message resumes it from the snapshot, on any worker.
- **Supervising.** The session subscribes to the goal's items and wakes on
  what needs judgement: a step held for a person, an escalation, the
  budget crossing a threshold, a stall, a person's message. By default
  each wake is a fresh run whose brief is the goal, the plan's status,
  what changed since its last turn, and the goal's notes (section 10). It
  acts within the plan's envelope, asks a person beyond it, notes what its
  next turn should know, and ends.
- **Resume or start fresh** is the engine's choice at each wake; the run
  cannot tell. A snapshot is a cache the engine never looks inside: a
  lost, oversized or stale one means starting fresh from the forge, which
  always works. The session's step says which to prefer: by default a
  chatting session resumes and a supervising one starts fresh; either may
  be told always or never.
- **Commitments at once, conversation per turn.** A task, a plan or a
  change is an outcome, applied as in 4.4. The transcript is on the
  session's issue turn by turn; tool calls link to traces.
- **Escalations go to the supervisor first:** a step that needs a decision
  is held, and wakes its goal's session, which holds the conversation's
  context, before any person is asked; the session may release it.
- **A session's outcomes,** as built: a reply (its comment is the reply),
  a plan proposed, steps added to its goal's plan, tasks, the release of
  one of its goal's held steps, and a last turn that finishes it.
- **A session may change code itself,** within its grants: its change is a
  change step like any other.

## 7. Rules

The rules are what no plan can loosen. They are configured per
deployment, from a vocabulary fixed in the engine's code, and checked on
every run the engine starts and every write it makes, whatever the plan
says. A plan's gates are added to them, never subtracted. For example:

- nothing lands on a protected branch without green CI on its exact head
  and a person's approving review;
- writes go only to the deployment's repositories;
- a plan beyond a size or a budget needs a person's acceptance;
- a note scoped wider than a goal needs a person's acceptance, where the
  deployment wants it;
- spending is bounded per run, per goal and per deployment;
- who may accept, steer, cancel or release follows their permission on
  the repository, as the forge reports it.

What building them settled:

- **Four answers,** each saying what clears it: allow; wait, for facts the
  forge has yet to report (green CI, an approving review, on the exact
  head), which no person clears; a person's acceptance, at a permission
  on the repository; or refuse. A check answers the strictest of what it
  found, and why, so a protected landing without its CI or review waits,
  or is refused once CI has failed, and never asks a person instead.
- **Checks are pure,** over facts the top level gathers: before a run
  starts (where it may push, its budget on top of what was spent), before
  a write, and before a person's request is acted on.
- **Reviews** count from a person with write permission at least (more on
  protected branches if the deployment asks), never from the engine's own
  user; a request for changes stands, whatever head it was on, until the
  same reviewer approves.
- **Plans** need acceptance past a size, past an estimate, or when they
  land on a protected branch; an estimate counts on top of its goal's
  spend, which is counted in one unit of the deployment's.

## 8. The fleet

The fleet sub-model knows the workers and the runs they host.

- **Workers** dial in with their slots and the workstreams they hold
  checkouts for; a claimed run goes to a worker with a free slot,
  preferring one that holds its item's workstream, so the runs of one
  piece of work find their checkout cached. A worker that refuses as busy
  gets nothing more until it frees a slot, and the attempt is placed
  again: a refusal is no failure.
- **Attempts are fenced.** Everything a worker sends is dropped unless its
  attempt is the item's live claim, but a cancelled attempt's answer. No
  attempt of an item is placed while a worker may still host another.
- **Answers are acknowledged once durable** (4.2), or once fenced off. A
  worker keeps an answer and its slot until then, and sends it again
  after every hello; a copy meanwhile is dropped unacknowledged, and one
  for an attempt the engine no longer knows is acknowledged again.
- **Room** is kept for the runs workers list: a hello is turned away only
  when there is no room for another worker, and a listing beyond the room
  is cancelled, not refused.
- **Losing contact.** A lost worker's runs are kept for a grace; one that
  reconnects says what it hosts, and the engine keeps or cancels each.
  Past the grace the runs are presumed lost, and their items retry. An
  assignment in flight when the channel drops is presumed lost only at
  the grace's end.
- **Restarting.** The engine adopts the claims its records hold, then says
  it has read them all; until then it starts nothing new, and it never
  cancels a claim before adopting it. A claim no worker reports within
  the grace is lost. A run a worker lists that no claim adopts is told to
  the engine (it names the attempts of a mangled record, 4.2), and is
  cancelled once a grace has passed since the reading ended and since it
  was first listed.
- **Every start and adoption ends once:** answered, lost, withdrawn (the
  engine cancelled it), or refused (busy, the workstream held, a
  duplicate).
- **Relaying:** inbox events and cancels down, host calls up and their
  answers back, each call answered once. An inbox event that reaches no
  worker comes back to the engine, which keeps it; one a worker bounces
  comes back without saying which it was (section 15).

## 9. Briefs

The engine renders the brief of every run (agent-model.md, 4.1) from
typed sections the step selects: the item and its lineage, the comments
since the run's last turn, its dependencies' outcomes, the CI failures on
its head with their output, review comments, its pull request against its
base, its earlier attempts and why they failed, the plan's status, the
index of the notes in its scope (section 10), the template it follows.
What the LLM needs reaches it as sections, never folded into its
instructions. What building the brief sub-model settled:

- **Sections follow why the run runs,** and those it runs for are
  required: the item always, and what a repair repairs (CI for failed CI,
  reviews for changes asked for, the pull request for a moved base or a
  conflict). The rest are optional: the comments, attempts and notes for
  every run; dependencies and a template where it has them; the plan for
  a run that may grow it and a supervisor's turn; reviews for a review. A
  brief without a required section fails, saying which and why (its read
  failed, brought more than a read may, or came too late), and the engine
  decides whether the run waits for another try; a missing optional
  section is marked so, and why.
- **Budgets and cuts.** Each section has a byte budget, and so has the
  brief, which the long sections share evenly once the short ones fit
  whole. What does not fit is cut in its kind's order (the oldest
  comments first, a CI failure's output keeping its end), never inside a
  UTF-8 sequence, and each cut says how much: `[N bytes cut]`.
- **A section is read as it is cut,** asking for no more than it keeps,
  and a brief gathers within one deadline of its own: what has not
  arrived by then is missing.

## 10. Notes

Notes are what temper learns and passes on: a convention a reviewer keeps
asking for, why an approach was dropped, which test is flaky, what a
person prefers. They are to runs what memory is to a coding agent at a
terminal.

- **An entry** has a one-line description, a body, a scope (the
  deployment, a repository or a goal), references to items, closed ones
  included, and who wrote it: a person, or a run and its item.
- **In the forge's wiki,** like everything that must last and belongs to
  no one item (section 12), where people can read, correct and delete
  entries: a page per entry, whose first line is its description; a
  repository's in its wiki, a goal's under the goal's path in its
  repository's wiki, the deployment's in its home repository's. Notes are
  hints: where a note and the forge disagree, the forge is right.
- **Written through an outlet.** A run writes a note with its `note`
  outlet, which the engine applies like an outcome (4.4): keyed,
  repeat-safe and checked by the rules, which may want a person to accept
  a note of wide scope. People write notes directly.
- **Read in two steps.** A brief carries the index of the notes in its
  scope, a line per entry, saying how many more there are when they do
  not all fit; a run asks for the entries it wants with a `recall` read,
  by name or by a search of their descriptions.
- **Curated** by a session on a slow timer, through the same outlet. A
  supervisor's memory is its goal's notes: what one turn leaves for the
  next.
- **Knowledge of the code itself** belongs in the repository, as
  `AGENTS.md` does, versioned and reviewed with the code; a note that
  proves durable can move there through a change.

The notes sub-model knows scopes, entries and indexes, and nothing of
plans. What building it settled:

- **Indexes are kept per scope in use,** learned from the scope's wiki one
  operation at a time, and evicted least recently used unless a call
  waits on them. A search matches their descriptions; a recall reads each
  page afresh, and answers with it as the wiki served it.
- **A note revised** names the revision its run recalled, and is refused
  as moved if the page changed since. The wiki's API has no conditional
  edit, so between the read before the edit and the edit a person's
  change can still be lost.
- **The wiki's pages** are read and written by the forge sub-model, in the
  same request budget as everything else (section 12).

## 11. Views

What runs report (agent-model.md, section 7) reaches the engine through
the workers, best effort. The views sub-model streams it live to the
people watching a run, an item or a repository's board; keeps traces for
a retention period, with content as the run's capture policy says
(nothing, its shape, or its content, per kind of report); and shows
items' phases as they change. Nothing the engine decides depends on a
fact arriving. What building the views settled:

- **A watch starts from a snapshot** the engine gives, delivered first.
  Reports and traces carry the run's attempt, so a retry's never mix with
  an earlier attempt's.
- **A slow watcher never holds the engine up:** one delivery in flight,
  whose time the engine bounds, and a bounded backlog; what overflows is
  dropped, and the watcher told how much it missed. A watch past the
  limits is refused.
- **Traces** are batched and appended to the store, which takes its
  operations in order; expiry is a periodic sweep by time, so what an
  earlier engine kept is forgotten too. The views' clock starts again
  with the engine; the store maps it to wall time. A batch the store fails
  to take is dropped and counted: traces are expendable.

## 12. The forge

The forge sub-model is the engine's knowledge of the forge and its only
way to change it. It holds live work, not the forge: what it holds, and
what it costs to start, grow with the work in progress, never with the
forge's history.

- **Items and the wiki.** What belongs to an item is on the item: its
  record, outcomes and transcript, which end with it. What belongs to no
  one item and has no lifecycle, such as notes, is in the wiki.
- **A working set, not a copy,** of the items not done: where each record
  is and the inbox position it names, labels, pull requests' heads and
  bases, CI and verdicts on those heads (each reviewer's latest, the
  engine's own excluded), and inboxes (4.3). An item enters when the
  engine creates it or takes it in, and leaves once it is closed or gone.
  Its capacity is a limit: when it is full, new work waits on the forge,
  where its labels find it again, and nothing taken in is dropped.
- **Everything else on demand:** long threads, a finished dependency's
  outcome, a closed item new work refers to, what a run asks to read;
  fetched within a budget, and not kept.
- **Starting** reads what is live: the open items with the engine's label
  or the hand-in label (4.6), and their records; cold for now (section
  16).
- **Keeping up** reads what changed since the last read, sooner when a
  webhook hints; polling is the backstop, so a lost webhook costs only
  latency, and the cost follows the rate of change, not the number of
  items. What moves no updated time on Forgejo (CI reporting, a base
  moving) is polled per pull request, on a backoff of its own. A slow
  pass over every open item finds one whose label a person removed,
  reading a few unlabelled items per page for a record of the engine's.
- **The forge's clock, not the engine's,** for every time a listing
  compares. Listings follow Forgejo's (updated at or after a time, to the
  second), and list again from the newest time seen, inclusive: an item
  that moves during a listing costs a duplicate, never a miss.
- **Writes** are typed operations, made after a fresh read, serialised per
  item, retried when transient, keyed so every creation is findable.
- **A request budget** keeps the engine within the forge's rate limits:
  fresh reads first, a write's own checks among them, then writes, then
  keeping up, then the slow pass, with a share of each window kept for
  the last two. A refusal for the rate stops every call until its reset.
- **Leaving.** The engine is told why an item left: closed, or gone. One
  the forge keeps refusing as forbidden is retried after a backoff, and
  the engine told once the retries are spent.

## 13. Below the model

What the protocol and io layers owe the model, to be designed after it:

- **The forge:** each provider's HTTP API, Forgejo first; pagination;
  webhooks and their signatures; the record and outcome blocks inside
  comments, decoded into model entities; credentials. As the forge
  sub-model's vocabulary fixes it: listings least recently updated first
  (Forgejo's default is newest first), each page saying the forge's time
  as it was made; comments paged by id, reviews and statuses by number; a
  revision for each comment, changing whenever its body does; markers
  carrying a creation's key, the person a message is written for, and a
  record's or a wiki page's nonce; and a failure is unavailable only when
  the request provably never reached the forge, anything later being a
  timeout.
- **Times:** the forge's, mapped onto the engine's clock as the plan reads
  them, and the times records keep, mapped so that they mean the same
  after a restart, whose clock starts again.
- **Workers:** the authenticated, framed channel of worker-model.md.
- **People:** the web: HTTP, live streams, and signing in through the
  forge.
- **The store:** snapshots, traces, and a cache of the working set, on
  disk, taking its operations in order and mapping the views' times to
  wall time.
- **Configuration:** a deployment's repositories, rules, templates and
  limits.

## 14. The world

The engine's world runs the model against fakes that share none of its
types (programming-style.md, section 11):

- **a forge** with issues, pull requests, comments, labels and wikis; CI
  that passes, fails or never reports; reviews; merges that conflict;
  webhooks that come late or not at all; requests that fail or hit a rate
  limit; and people who comment and edit;
- **workers** hosting scripted runs: outcomes, failures, yields, parks,
  lost contact and reconnection;
- **people** who chat, accept and reject plans, release held work, and
  write and correct notes.

The engine, workers and agents meet in a larger world, a system world
(testing-pyramid.md, section 2.3), with the fake LLM provider in place of
a real one. The fake forge and fake people are described in
testing-pyramid.md, sections 4.2 and 4.4: the forge keeps the git that
workers push to in one store with its API, and both fakes grow into
services as the engine's protocol and io layers are built.

Each sub-model has a world of its own, `tests/integration/engine/<name>`,
its parent and neighbours scripted in it and its scenario's expectations
in a referee (testing-pyramid.md, 5.2); the forge sub-model's runs
against the fake forge. The rules, decisions over data with no state to
drive, are tested by their step tests, with a sweep over many cases
checked against an independent statement of the protected-landing rule.
The engine's own world, with the top level, is being built.

## 15. Open questions

- **People on the forge:** whether a person's messages are written with
  their own forge credentials or by the engine on their behalf. The model
  supports the second: a message the engine posts for a person names
  them, and is news from them.
- **Bounced inbox events:** a worker's bounce says why, not which event
  it was, so the engine cannot tell which of the events it relayed a run
  did not take, while the inbox position must move only past what the
  run took (4.3; worker-model.md, section 10). A bounce naming the event,
  or an answer saying how many the run read, would settle it.
- **An item closed under a live run:** the hub learns an item is done
  only by asking what is due, once its run has answered; whether a
  person's close should cancel the run at once.
- **Notes in the wiki:** how a search reaches entries when there are
  many; how the engine reaches a wiki on a forge whose API has none
  (GitHub's wikis are git repositories only, and the engine otherwise
  never touches git); whether a call should hold a scope's index
  explicitly rather than keep it least recently used; and what the engine
  does when a note's keyed write, tried again, finds its page made
  already.
- **Where templates live:** deployment configuration first; later the
  wiki, as state that belongs to no one item.
- **Spend:** its unit, and whether the deployment's bound has a window.
- **Keeping up:** whether a pull request's base moving should be news in
  its inbox, rather than level state its plan reads; and whether the
  engine should be told when an item it was refused as forbidden is
  readable again.
- **Briefs:** whether a brief's forge reads count against a budget of
  their own; cancelling a brief while it gathers; how a missing section
  shows in the charter the agent reads.
- **Views:** a limit on watchers per subject.
- **Snapshots:** their size limit and how long the engine keeps them
  (worker-model.md, section 10).
- **Checks without an LLM:** a checks-only run on an exact head, used as
  a gate (worker-model.md, section 10).
- **Saved-work branches:** their names and when the engine deletes them,
  with the worker.
- **Provisioning a repository** (labels, webhooks, CI): an engine command,
  later.

## 16. Not built yet

The sub-models run everything above in their worlds. What the model does
not do yet, each to be designed before it is built:

- **The top level and the engine's world** (sections 3 and 14): being
  built.
- **A warm start** (section 12): a cache of the working set in the store,
  so that a restart reads only what changed; every start is cold.
- **Subscriptions:** a step names no items it subscribes to, and the
  working set holds no items the engine does not track, so a session's
  wake rule can name the source but nothing feeds it.
- **Labels as a plan's inputs** (4.1): the plan reads no labels, and asks
  for none to be written.
- **Asking for reviews:** the forge sub-model writes a pull request's
  reviewers as a set, but no step asks for one, so a person whose review
  a step waits for is not asked.
- **Checks-only runs, and gates on them** (5.1).
- **Landing a branch other steps built,** as the example's `land` does
  (5.2): a change's branch is always the one its own runs push.
- **A CI failure's output** in a brief (section 9): the forge sub-model
  reads statuses, their descriptions and links, not the output behind
  them.
- **The newest comments first:** the forge's reads page from the oldest,
  so the newest comments, which a brief's comments section keeps, are
  reached only through all the older ones.
- **Reading traces back** for the web: the views keep them, and nothing
  reads them.
- **Templates in the wiki,** and provisioning a repository (section 15).
