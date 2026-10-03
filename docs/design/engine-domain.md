# The engine's domain layer

Provisional, 2026-10-02. What the temper engine does, as a domain layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of skein's `docs/foundation/programming-model.md`;
the worker it drives is described in `worker-domain.md`, and the agent in
`agent-domain.md`. Each part's details are settled as it is built and kept
in its crate's documentation; this document keeps the decisions and their
reasons. What is still open is listed in section 15, and what is not built
yet in section 16. How the engine is tested is in `testing.md`, which
applies skein's `docs/foundation/testing-strategy.md` to temper.

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
- **The domain is complete** (programming-model.md, section 4): a world of
  domains and fakes runs everything the engine does, with no protocol and
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

- **Workers dial in** (worker-domain.md, section 2). The engine assigns
  runs, sends inbound events and cancels, answers the forge reads and
  outlets a run asks for, and receives facts, parks and ends. It never
  connects to a worker.
- **The engine never reads a repository.** It works from forge state; what
  a checkout holds is for the run to find (agent-domain.md, section 2).
- **People reach temper through the engine.** The web is its front door:
  a chat to hold, a proposal to accept, a run to watch or stop, held work
  to release. People also act on the forge directly: a comment, a review,
  a label a plan reads. Either way the engine learns of it as a request
  from a person or a change on the forge, and a person is a forge user.

## 3. Structure

```
temper-engine-domain                  the engine loop's entry point: forge, workers, people; routing
├── temper-engine-domain-work         items: waiting, due, claimed, running, applying, held, done
├── temper-engine-domain-plan         plans: steps, dependencies, gates, wakes, envelopes
├── temper-engine-domain-rules        the deployment's rules, which every run and write must satisfy
├── temper-engine-domain-forge        the working set and the client: live work, reads, keyed writes, request budget
├── temper-engine-domain-fleet        workers: slots, placement, attempts, contact, relaying
├── temper-engine-domain-brief        a run's brief, from typed sections within a byte budget
├── temper-engine-domain-notes        notes: scopes, entries, the index a brief carries
└── temper-engine-domain-views        facts in; live streams and retained traces out
```

The tree follows programming-model.md, 4.5: each child domain has its own
vocabulary, limits and world; a parent owns its children's state and
routes between them; siblings share no domain types. `work` is the hub,
as `host` is in the worker: it knows an item's lifecycle and nothing of
the forge's API, the workers' channels or a plan's vocabulary. `plan` and
`rules` are policy: decisions over data, which keep nothing between calls
but their configuration, so either can be tested alone. The others are
capabilities. `temper-engine-domain` faces the protocol layer, and its
vocabulary is what crosses the domain boundary: the forge's operations and
webhooks, the workers' channel, people's requests, and the engine's
store.

The top level keeps a table of the items it holds: each one's record
parts, the job the hub asked for, the run in flight and its inbox. What
building it settled, beyond what the sections below record of each flow:

- **Records are composed and split:** composed as they are written, from
  the hub's lifecycle, the plan's step as last committed and the top
  level's relations, and split into them as they are read (4.1). An
  application commits the plan's writes to the step only once every
  write is made, so a record written meanwhile carries the step as it
  was. Nothing durable is ever a token.
- **The restart contract.** A restart is a cold start that rebuilds from
  the forge and the store everything a decision reads, and makes nothing
  twice. The new engine makes no call until a lifetime after it starts
  (4.4). Each item found is taken into the hub as its record says, once
  the record is found sound, as the plan would have written it (its step,
  its goal's plan, its relations within the limits), or held as mangled.
  A claim is adopted at once, with the grants its step gives the run, and
  a record's pull request is linked again. The fleet hears that the
  reading is done only once every claim read has reached it; nothing new
  starts before, nor does an application resumed, which reads its goal's
  and relations' records. An item that did not fit the working set is
  not waited for (its run is a stray, section 8). Every item read waiting
  is asked what is due afresh, a relation found done counting as news
  again. What an earlier life may have made is looked for before it is
  made: an adopted attempt's outcome and comments after its claim's
  inbox position, an outcome's writes after its comment (the one bound an
  earlier life leaves), an action's creations and a person's keyed
  request anywhere. An adopted attempt takes nothing of the new life's
  inbox, so what it may not have seen goes to the next run. The commit
  point stays the record's update after an outcome's writes (4.4).

## 4. Items

### 4.1 What an item is

- **A tracked issue or pull request** in one of the deployment's
  repositories, from when the engine creates it or takes it in (4.6)
  until its step is done.
- **Its record** is the engine's state for it, in one comment the engine
  owns on the item: a readable panel with a typed record inside. Its parts
  have owners: the step the item carries, its progress and, on a goal's
  item, the plan, are the plan's (section 5); the phase, the claim and
  the attempts per failure class, the hub's (4.2); the inbox position,
  and a nonce naming the write that made the record, the forge
  child domain's (section 12), which knows where each record is and
  nothing of what it says; what it has spent and the relations the forge
  cannot hold, such as a parent in another repository, the top level's.
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

An item is **due** when the plan says it has work now (5.3), a run or an
engine action. It is **claimed** when the engine writes the claim, naming
the run and its attempt, into the record before it assigns the run;
attempts count up across restarts, so a stale attempt stays fenced. A
**running** run that yields waits for its next event; one that parks
hands over its snapshot, and the item waits for its next wake.

A failure is retried after a backoff, within a number of attempts per
failure class, or the item is **held for a person**: it stops being due
until a person releases it. Held is the engine's state, which a label may
show; the engine reads no label for it.

The `work` child domain, the hub, keeps the lifecycle. What building it
settled:

- **The hub is rebuilt from the records alone.** A record says waiting,
  parked, retrying (and the class), claimed, applying (and the outcome's
  comment), held (why, and the outcome it keeps) or done; never due,
  running or a backoff, which are found afresh after a restart. Records
  claimed or applying are taken first: a claim read is adopted, an
  application read resumed.
- **Failure classes:** transient, permanent until something on the forge
  changes, the run's own, the agent's, lost, and an outcome judged
  invalid, each with its own retries and jittered backoff. A run refused
  before anything ran (busy, its workstream held, a duplicate) is no
  failure: the item claims again after a pause.
- **An answer is acknowledged once it is durable,** once the record says
  it is being applied, parked or failed; until then the worker keeps it,
  and its slot. If that record cannot be written, the item is held, still
  keeping the answer and the slot, until a person releases it.
- **Holds keep their outcome.** An outcome not wholly applied (waiting for
  acceptance, its writes failed, its record not written) is applied again
  on release. Held items stay in the working set, and count against its
  capacity.
- **A hold says why,** typed in the record: failures of a class, a
  person's stop, acceptance, writes, the record, or a code the top level
  gives, which means the same after a restart: zero for a record that
  carries no step; one to six for the plan's reasons (rejected, repairs,
  rebases, its pull request closed, escalated, stalled); seven and eight,
  the top level's own, for a run the rules want a person to accept
  first, or refuse (section 7).
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
  happens to the engine. All that is held is where to read from, so news
  that does not fit the working set's bounded inbox stays on the forge,
  told again once a run's answer has made room. The position moves only
  past what a run took, so a failed run's events go again to its retry.
- **Notices of its relations** (one done or held, a decision) merge with
  one alike, given to a run or not, and keep to a share of the inbox of
  their own, beyond which they are dropped: they never crowd out news.
- **Relayed or woken.** A live run gets inbox events as they arrive;
  events the fleet could not deliver yet are kept and relayed again once
  the run is placed. An item without a run is woken by them, as its
  step's wake rule says (5.4).
- **What is news,** as built: comments after the position on the item and
  its pull request, save the engine's own; and its pull request's head,
  CI, state and verdicts, as a level against what the position took. A
  dependency the engine does not track leaves no news: it is read afresh
  when a decision needs it.
- **A person's message reaches a run.** One from the web is written on
  the item for them, keyed by their request, and is news from them as a
  comment on the forge is. A live run has it relayed, naming the comment;
  an item without one is woken, a session at once. A run's brief
  requires people's waiting messages, oldest first and whole, as many as
  fit (section 9); its answer takes the inbox only up to the first one
  the brief did not carry and no relay gave it.

### 4.4 Outcomes

- **Recorded, then applied.** A run's outcome is posted on its item first,
  a readable account with a typed block inside: the audit trail and the
  durable intent at once. Its words (a report, a reply, an escalation, a
  verdict's reasons) are there; the writes carry none of their own.
- **Applied against fresh state.** The engine reads afresh what the
  outcome touches, asks the plan what it writes, checks each write
  against the rules, and makes them. The run judged the outcome against
  its spec (agent-domain.md, 4.4); the engine judges it again against the
  forge as it is now, and one that no longer fits is stale and not
  applied.
- **Keyed, with one commit point.** Every creation carries a key derived
  from the outcome, so a second attempt finds what the first made; sets
  (labels, reviewers, dependencies) are written as sets. The record's
  update comes last and is the commit point: until it lands, the outcome
  is still being applied, and a restart resumes it.
- **Held for acceptance** when the rules say so (section 7): nothing of a
  proposal is applied until a person accepts it. A decision names what it
  is on, the outcome a hold keeps (by its comment) or the step itself,
  and is taken only by an item held for one or a step that waits for
  one; a held item is released, to apply its outcome again with it. An
  acceptance of the step lasts until it is released or done, covering
  its runs, actions and outcomes' writes, but not a plan proposed or
  growth; the permission an acceptance wants is kept in the record.
- **What may have happened is found, not repeated.** Only a request that
  provably never reached the forge did nothing; any other failure may
  still land until a configured lifetime after it went out. Such a write
  is looked for before it is tried again (a creation by its key, a merge
  by its head, a record by its nonce), its item's writes wait out the
  lifetime, and a new engine makes no call until a lifetime after it
  starts (section 3).
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
the head the decision saw, closing an item and deleting a branch. A merge
reads its pull request afresh first: one retargeted since the decision is
not merged.

### 4.6 Taking work in

Work starts in three ways: a person opens a session from the web; a
person hands an issue to temper, with a label the deployment names (later
a mention); or the engine creates the item itself, applying a plan or an
outcome. An issue handed in becomes a session (section 6), whose agent
answers it, makes the change, or proposes a plan. From then on the record
says what an item is; labels never decide it again. A person who opens a
session from the web is answered once its first record is written.

## 5. Plans

### 5.1 Primitives

A step is one of a few primitives, which are the engine's code:

- **Agent step:** a run with a charter (agent-domain.md, 4.1). Its outcome
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
gates are people's approvals on the exact head and a person's
acceptance; the rules check them, beside gates of their own that the top
level derives from the step (read-only, green CI, a review at a
permission, spend).

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
- **Growth is repeat-safe.** The goal's record is written first, tried
  again while the forge fails for a while; the growing step's, the
  commit point, waits for it to land; if the goal's cannot be written,
  the step is held for its writes. Applied again, growth finds its steps
  joined to the goal (the same names, after the same steps, added by the
  same run) and asks for the same writes.
- **An accepted growth widens the envelope** by what it added. A
  supervising session's tasks join its goal's plan within its envelope and
  budget, so tasks are no way around it.

### 5.3 What is due

The `plan` child domain is pure: given an item's record and what the
working set holds about it and its relations, it says what is due now,
whether an inbox event wakes the item, what a run's charter is, and what
an outcome writes. Everything it needs is in the record and the forge, so
a restart loses nothing it knew. What building it settled:

- **A change's way to landing.** No branch yet: a run produces it. A
  branch and no pull request: the engine opens one. Open: a conflict or a
  moved base asks for a rebase run; failed CI or changes asked for on the
  head, a repair run; CI pending or a mergeability not yet known waits;
  a clean head whose reviews and gates hold is merged at exactly that
  head. Merged, the step is done; closed unmerged, it is held.
- **A branch another party deleted** has the change made again from its
  base, which counts as a rebase; once it is pushed again, the pull
  request the deletion closed is reopened.
- **A merge the forge refuses for a conflict** goes back to the change:
  its base moved since the head was read, so the merge is dropped as
  stale, the head is taken as conflicting, and the change gets a rebase
  run like any other, never a hold for a failed write.
- **Repairs and rebases are bounded apart,** each held past its limit;
  the rebases' is higher, since a busy base moves often and that is no
  fault of the change. One whose outcome went stale, its branch moved
  under it, counts all the same.
- **Waits on the forge are bounded:** a change that waits for CI, a
  review, mergeability or a merge the rules hold back, past a configured
  stall counted from its head's push or its last release, is held, its
  goal's session told first.
- **A release lifts what it releases:** repairs and rebases count again
  from it, and a decision made before it counts for nothing, so the item
  is not held again at once for the same cause. A session released after
  its runs failed takes the turn it never had, with no new message.
- **A claim says why.** A run is claimed with a progress write recording
  why it runs (an agent step's work, producing a change, a repair and its
  cause, a review at a head, a session's turn) and when, so applying its
  outcome, and a session's next wake, need nothing kept in memory. Every
  outcome applied clears it, last.
- **Sessions end** with an outcome that finishes them, or when a person
  closes their item; a supervising session is done once its goal's steps
  are. An item a person closes is done, whatever its step.
- **Time** in the record and in what the plan reads is wall time
  (`env.wall`, programming-model.md, section 9), so a record means the
  same after a restart; the forge's times are wall times too. A wait
  the plan arms is a deadline on the engine's monotonic clock
  (`env.now`), computed from them.

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
  budget crossing a threshold, a stall, a person's message. Each wake is a
  run whose brief is the goal, the plan's status, what changed since its
  last turn, and the goal's notes (section 10); it acts within the plan's
  envelope and asks a person beyond it.
- **Resume or start fresh** is the engine's choice at each wake; the run
  cannot tell. A snapshot is a cache the engine never looks inside: a
  lost, oversized or stale one means starting fresh from the forge, which
  always works. By default a chatting session resumes and a supervising
  one starts fresh; its step may say always or never.
- **Commitments at once, conversation per turn.** A task, a plan or a
  change is an outcome, applied as in 4.4. The transcript is on the
  session's issue turn by turn; tool calls link to traces.
- **Escalations go to the supervisor first:** a step that needs a decision
  is held, and wakes its goal's session, which holds the conversation's
  context, before any person is asked; the session may release it.
- **A session's outcomes,** as built: a reply (its comment is the reply),
  a plan proposed, steps added to its goal's plan, tasks, the release of
  one of its goal's held steps, and a last turn that finishes it. It may
  change code itself, within its grants, through a change step like any
  other.

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
- **Where the top level asks them.** A run the rules want accepted, or
  refuse, is held before it is claimed (4.2's codes seven and eight),
  and they are asked again as it starts, since what was spent meanwhile
  may change their answer. Every outcome that makes steps (a plan
  proposed, growth, tasks) goes through the plan rule. A note the rules
  want accepted is refused, not held (section 16).

## 8. The fleet

The fleet child domain knows the workers and the runs they host.

- **Workers** dial in with their slots and the workstreams they hold
  checkouts for; a claimed run goes to a worker with a free slot,
  preferring one that holds its item's workstream, so the runs of one
  piece of work find their checkout cached. A worker that refuses as busy
  gets nothing more until it frees a slot, and the attempt is placed
  again: a refusal is no failure.
- **Attempts are fenced.** Everything a worker sends is dropped unless its
  attempt is the item's live claim, but a cancelled attempt's answer. No
  attempt of an item is placed while the fleet knows a worker may still
  host another.
- **Answers are acknowledged once durable** (4.2), or once fenced off. A
  worker sends an answer again after every hello until then; a copy
  meanwhile is dropped unacknowledged, and one for an attempt the engine
  no longer knows is acknowledged again.
- **Room** is kept for the runs workers list: a hello is turned away only
  for want of room for a worker, and a listing beyond it is cancelled.
- **Losing contact.** A lost worker's runs are kept for a grace; one that
  reconnects says what it hosts, and the engine keeps or cancels each.
  Past the grace the runs are presumed lost, and their items retry. An
  assignment in flight when the channel drops is presumed lost only at
  the grace's end.
- **Where a change's branch is** comes from its runs' answers; a late
  one, its attempt presumed lost or fenced off, is not taken but still
  gives its push while the record names no branch. The branch is read on
  the forge before a change is produced again with none recorded (an
  attempt whose answer never came may have pushed it), before a run
  starts from it after one failed for good, and before deciding for a
  change that names one with no open pull request; one gone has the
  change made again from its base (5.3).
- **The graces need no order.** Set past the worker's (worker-domain.md,
  section 2), the engine's grace places no next attempt while a worker
  may still host the last; shorter, attempts may overlap on two workers,
  but pushes are fast-forwards, so the later push is refused as moved
  and the earlier one found as above. The whole worker's world draws it
  on both sides of the worker's, with one worker, so without overlap.
- **Restarting** (section 3). The engine adopts the claims its records
  hold, then says it has read them all; it never cancels a claim before
  adopting it. A claim no worker reports within the grace is lost. A run
  a worker lists that no claim adopts is told to the engine (it names the
  attempts of a mangled record, 4.2), and cancelled once a grace has
  passed since the reading ended and since it was first listed.
- **Every start and adoption ends once:** answered, lost, withdrawn (the
  engine cancelled it), or refused (busy, the workstream held, a
  duplicate).
- **Relaying:** inbox events and cancels down, host calls up and their
  answers back, each call answered once. A call the fleet cannot pass up
  is answered at once, unserved, never dropped: busy while its attempt is
  or may yet be the live claim (no room for the call, or the cold start
  not done), failed once the attempt is fenced off. An inbox event that
  reaches no worker comes back to the engine, which keeps it; one a
  worker bounces comes back without saying which it was (section 15).

## 9. Briefs

The engine renders the brief of every run (agent-domain.md, 4.1) from
typed sections the step selects: the item and its lineage, people's
comments since the run's last turn, its dependencies' outcomes, the CI
failures on its head with their output, review comments, its pull
request against its base, its earlier attempts and why they failed, the
plan's status, the index of the notes in its scope (section 10), the
template it follows.
What the LLM needs reaches it as sections, never folded into its
instructions. What building the brief child domain settled:

- **Sections follow why the run runs,** and those it runs for are
  required: the item always, and what a repair repairs (CI for failed CI,
  reviews for changes asked for, the pull request for a moved base or a
  conflict); and the top level requires the comments while people's
  messages wait (4.3). The rest are optional: the comments, attempts and
  notes for every run; dependencies and a template where it has them; the
  plan for a run that may grow it and a supervisor's turn; reviews for a
  review. A brief without a required section fails, saying which and why
  (its read failed, brought more than a read may, or came too late), and
  its run fails as transient, to be tried again; a missing optional
  section is marked so, and why.
- **Budgets and cuts.** Each section has a byte budget, and so has the
  brief, which the long sections share evenly once the short ones fit
  whole. What does not fit is cut in its kind's order (the oldest
  comments first, a CI failure's output keeping its end), never inside a
  UTF-8 sequence, and each cut says how much: `[N bytes cut]`.
- **A section is read as it is cut,** asking for no more than it keeps,
  and a brief gathers within one deadline of its own: what has not
  arrived by then is missing. People's comments are read oldest first,
  whole, as many as fit, the first cut if it alone does not (4.3).

## 10. Notes

Notes are what temper learns and passes on: a convention a reviewer keeps
asking for, why an approach was dropped, which test is flaky, what a
person prefers. They are to runs what memory is to a coding agent at a
terminal.

- **An entry** has a one-line description, a body, a scope (the
  deployment, a repository or a goal), references to items, closed ones
  included, and who wrote it: a person, or a run and its item.
- **In the forge's wiki,** like everything that must last and belongs to
  no one item (section 12), where people read, correct and delete them: a
  page per entry, whose first line is its description; a repository's in
  its wiki, a goal's under its own path there, the deployment's in its
  home repository's. Notes are hints: where a note and the forge
  disagree, the forge is right.
- **Written through an outlet.** A run writes a note with its `note`
  outlet, which the engine applies like an outcome (4.4): keyed,
  repeat-safe and checked by the rules. People write notes directly, and
  a session on a slow timer can curate them. A supervisor's memory is its
  goal's notes.
- **Read in two steps.** A brief carries the index of the notes in its
  scope, a line per entry, saying how many more there are when they do
  not all fit; a run asks for the entries it wants with a `recall` read,
  by name or by a search of their descriptions.
- **Knowledge of the code itself** belongs in the repository, as
  `AGENTS.md` does, versioned and reviewed with the code.

The notes child domain knows scopes, entries and indexes, and nothing of
plans. What building it settled:

- **Indexes are kept per scope in use,** learned from the scope's wiki one
  operation at a time, and evicted least recently used unless a call
  waits on them. A search matches their descriptions; a recall reads each
  page afresh, and answers with it as the wiki served it.
- **A note revised** names the revision its run recalled, and is refused
  as moved if the page changed since. The wiki's API has no conditional
  edit, so between the read before the edit and the edit a person's
  change can still be lost.
- **The wiki's pages** are read and written by the forge child domain, in
  the same request budget as everything else (section 12).

## 11. Views

What runs report (agent-domain.md, section 7) reaches the engine through
the workers, best effort. The views child domain streams it live to the
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
  earlier engine kept is forgotten too. Traces carry wall time, so they
  mean the same across restarts. A batch the store fails to take is
  dropped and counted: traces are expendable.

## 12. The forge

The forge child domain is the engine's knowledge of the forge and its only
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
  engine creates it or takes it in, and leaves once it is closed or gone,
  the engine told which; one the forge keeps refusing as forbidden is
  retried after a backoff, the engine told once the retries are spent.
  Its capacity is a limit: when it is full, new work waits on the forge,
  where its labels find it again, and nothing taken in is dropped.
- **Everything else on demand:** long threads, a finished dependency's
  outcome, a closed item new work refers to, what a run asks to read;
  fetched within a budget, and not kept.
- **Starting** reads what is live: the open items with the engine's label
  or the hand-in label (4.6), and their records; cold for now (section
  16).
- **Keeping up** reads what changed since the last read, sooner when a
  webhook hints; polling is the backstop, and its cost follows the rate
  of change, not the number of items. What moves no updated time on
  Forgejo (CI reporting, a base moving) is polled per pull request. A
  slow pass over every open item finds one whose label a person removed.
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

## 13. Below the domain

What the protocol and io layers owe the domain, to be designed after it:

- **The forge:** each provider's HTTP API, Forgejo first; pagination;
  webhooks and their signatures; the record and outcome blocks inside
  comments, decoded into domain entities; credentials. As the forge
  child domain's vocabulary fixes it: listings least recently updated
  first (Forgejo's default is newest first), each page saying the forge's
  time as it was made; a revision for each comment, changing whenever its
  body does; markers carrying a creation's key, the person a message is
  written for, and a record's or a wiki page's nonce; and a failure is
  unavailable only when the request provably never reached the forge.
- **Times:** the forge's, decoded as wall times; the domain turns them
  into deadlines itself (5.3).
- **Workers:** the authenticated, framed channel of worker-domain.md.
- **People:** the web: HTTP, live streams, and signing in through the
  forge.
- **The store:** snapshots, traces, and a cache of the working set, on
  disk, taking its operations in order.
- **Configuration:** a deployment's repositories, rules, templates and
  limits.

## 14. The world

Each child domain has a world of its own,
`tests/integration/engine/<name>`, its parent and neighbours scripted in
it and its scenario's expectations in a referee (testing.md, 5.2).
The rules, decisions over data, are tested by their step tests, with a
sweep checked against an independent statement of the protected-landing
rule.

The engine's world, `tests/integration/engine/domain`, runs the top level
with every child domain beneath it against fakes that share none of its
types (testing-strategy.md, section 4): the fake forge, its faults
drawn from the seed, reached through its protocol layer as the world
plays it, with codecs for what the engine writes inside comments and
wiki pages and for the charters and outcomes a worker carries as bytes;
one to three scripted workers, which play each run's script, keep
answers until acknowledged, and lose their channels and come back, or
vanish; scripted people on the forge and the web; a store, slow or
failing; and the engine restarting, cold, as its referee injects it: at
drawn moments, or at one a scenario chooses as the forge shows it (a
claim written, say), before it hears its call answered. The people's
stories: a session's hello; a chat that parks and resumes; an issue
handed in, its change failing CI, repaired, reviewed and landed; a note
a person corrects and a later run recalls; a plan proposed, decided,
grown within its envelope or beyond it, and landed; a proposal
rejected; a change whose CI never reports, stalled and held; a session
whose tracking label a person takes off, whose record they garble, or
whose run they watch and stop; a supervisor woken once by a burst of
its steps ending; an approval of an earlier head, which lands nothing.

Its referee holds the engine to what it promises, seen from outside:
nothing lands on a protected branch without green CI on its exact head
and a person's approval; writes go only to the deployment's
repositories; keyed creations and outcomes are made once (across a
restart at a drawn moment, counted instead: section 16); attempts only
grow, one live run per item, its dependencies done; nothing of a plan
is made, nor a goal's envelope widened, before a person accepts it; a
call its grants allow is never refused as ungranted; a person's message
reaches a run, or its item ends; every story ends within a bound. The
engine, workers and agents meet in the system worlds (testing.md,
2.1), which reuse this world's codecs, people, store and referee, and
the names runs and attempts take on a worker's channel.

## 15. Open questions

- **People on the forge:** whether a person's messages are written with
  their own forge credentials or by the engine on their behalf. The domain
  does the second: a message the engine posts for a person names them,
  and is news from them.
- **Bounced inbox events:** a worker's bounce says why, not which event
  it was, so the engine cannot tell which of the events it relayed a run
  did not take, while the inbox position must move only past what the
  run took (4.3; worker-domain.md, section 10). A bounce naming the event,
  or an answer saying how many the run read, would settle it.
- **An item closed under a live run:** the run is told its item is done,
  and the hub learns it once the run has answered; whether a person's
  close should cancel the run at once.
- **A call that waits for a person,** such as a note the rules want
  accepted: it may outlive its run, and nothing yet has a place for it to
  wait.
- **Notes in the wiki:** how a search reaches many entries; how the
  engine reaches a wiki on a forge whose API has none (GitHub's are git
  repositories only); whether a call should hold a scope's index
  explicitly; and what a note's keyed write, tried again, does on finding
  its page made already.
- **Where templates live:** deployment configuration first; later the
  wiki, as state that belongs to no one item.
- **Spend:** its unit, whether the deployment's bound has a window, and
  how what a run spent reaches the engine, which the worker's channel has
  no place for (worker-domain.md, section 10).
- **Keeping up:** whether a base moving should be news rather than level
  state; whether the engine should hear when an item refused as
  forbidden is readable again.
- **Briefs:** whether a brief's forge reads count against a budget of
  their own; cancelling a brief while it gathers; how a missing section
  shows in the charter the agent reads.
- **Views:** a limit on watchers per subject.
- **Snapshots:** their size limit and how long the engine keeps them
  (worker-domain.md, section 10).
- **Checks without an LLM:** a checks-only run on an exact head, used as
  a gate (worker-domain.md, section 10).
- **Saved-work branches:** their names, whether the engine starts runs
  from them (worker-domain.md, 4.1), and when it deletes them.
- **Provisioning a repository** (labels, webhooks, CI): an engine command,
  later.

## 16. Not built yet

The domain runs everything above in its worlds. What it does not do yet,
each to be designed before it is built:

- **A warm start** (section 12): a cache of the working set in the store,
  so that a restart reads only what changed; every start is cold.
- **Nothing made twice at any moment** (section 3): restarted at a
  random one, the engine may make a keyed creation again; the worlds
  count these, and only the restart scenarios assert none.
- **An item made while the top level's table is full** is not held, nor
  its first record, the only place its step is, written: a restart could
  lose the step (suspected).
- **Subscriptions:** a step names no items it subscribes to, and the
  working set holds no items the engine does not track, so a session's
  wake rule can name the source but nothing feeds it.
- **Labels** (4.1): the engine puts its tracking label on the items it
  makes, and projects nothing more; the plan reads no labels.
- **Asking for reviews:** no step asks the forge child domain to set a
  pull request's reviewers, so a person whose review a step waits for is
  not asked.
- **Checks-only runs, and gates on them** (5.1).
- **Landing a branch other steps built,** as the example's `land` does
  (5.2): a change's branch is always the one its own runs push, in its
  item's repository, the only one a run's workspace holds.
- **Starting from saved work or a commit:** runs start from a base or
  their item's branch (worker-domain.md, 4.1).
- **Dependencies' outcomes** in a brief (section 9): the section is asked
  for and never read, so it is always missing.
- **A CI failure's output** in a brief: the forge child domain reads
  statuses, their descriptions and links, not the output behind them.
- **A message carried whole:** a brief over its total budget may still
  cut a message its comments section carried, which its run takes all
  the same.
- **A message's text** for a live run: the inbound event names the
  comment, which the run reads if its grants let it read the forge.
- **Spend:** what a run spent does not cross the worker's channel, so the
  records' spend stays zero, and the rules over spend weigh only the
  budgets runs ask for.
- **Wall time** (5.3, section 11): records, traces and the forge's
  times still hold the engine's monotonic clock (`Time`), not `env.wall`.
- **Notes held for acceptance:** a note the rules want a person to accept
  is refused (section 15).
- **Reading traces back** for the web: the views keep them, and nothing
  reads them.
- **Templates in the wiki,** and provisioning a repository (section 15).
- **Plans in the whole worker's world:** it runs every story but the
  plans' (testing.md, section 9).
