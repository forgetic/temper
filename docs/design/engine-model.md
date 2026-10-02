# The engine's model layer

Provisional, 2026-10-02. What the temper engine does, as a model layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of `programming-style.md`; the worker it drives is
described in `worker-model.md`, and the agent in `agent-model.md`. Each
part's details are settled as it is built; what is still open is listed
in section 15, and what is not built yet in section 16. How the engine is
tested, and the fakes around it, is in `testing-pyramid.md`.

## 1. In one page

- **The engine is where work is decided.** It is the forge's only API
  client and the one place plans and rules live. It decides what happens
  next: which run to start, what a run's outcome changes on the forge,
  when to try again, when to ask a person. Workers host runs; agents do
  the LLM work.
- **The forge is the truth, and the engine keeps only caches.** What must
  outlive the engine's process is written to the forge: each item's
  record, the outcomes being applied, the transcripts. The engine's own
  store holds a cache of the live work it tracks, snapshots of parked runs
  and traces, all of them rebuildable or expendable. A restart is a
  rescan of live work, never of the forge's history.
- **Everything is an item.** An item is an issue or pull request the
  engine tracks, with a record of its own, an inbox, and at most one live
  run. Chat sessions, tasks, goals and the steps of a plan are all items;
  what differs is the step each one carries.
- **Plans are free; primitives and rules are fixed.** Agents compose
  plans for the task at hand, as graphs of a few primitives: agent steps,
  changes, waits and sessions. The primitives are the engine's code. The
  rules are the deployment's, and a plan can add to them but never loosen
  them, which is what makes plans written by agents safe to run.
- **Temper learns.** Notes keep what runs and people learn, scoped to the
  deployment, a repository or a goal, in the forge's wiki. Every brief
  lists the notes in its scope, and a run recalls the ones it needs
  (section 10).
- **Level-triggered.** What is due follows from the forge's state, not
  from the events that announced it. Webhooks are hints that make the
  engine look sooner; polling is the backstop. Before writing, the engine
  reads afresh and decides again.
- **Every write is repeat-safe.** Creations are keyed, sets are written as
  sets, and an outcome is recorded before it is applied, so an application
  that was interrupted resumes where it stopped.
- **The model is complete** (programming-style.md, section 4). A world of
  models and fakes runs everything the engine does, with no protocol and
  no io (section 14).
- **One engine per deployment.** Scaling out is a later problem with known
  solutions. What depends on it now is that the engine is the only writer
  of its own records.

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
- **People reach temper through the engine.** The web is the engine's
  front door: a chat to hold, a proposal to accept, a run to watch or
  stop, held work to release. People also act on the forge directly: a
  comment, a review, a label a plan reads. Either way the engine learns of
  it as a request from a person or a change on the forge, and a person is
  a forge user.
- **The engine is the only writer of its own records.** People and CI
  change the forge too; the engine observes what they change, and never
  needs a compare-and-swap against itself.

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
  repositories, from the moment the engine creates it or takes it in
  (4.6) until its step is done.
- **Its record** is the engine's state for it, kept in one comment the
  engine owns on the item: a readable status panel with a typed record
  inside. It holds the step the item carries (section 5), where the item
  is in its lifecycle, its inbox position, the current claim and the
  attempts so far, the outcome being applied, what it has spent, and the
  relations the forge cannot hold natively, such as a parent in another
  repository.
- **The record's parts have owners,** each a sub-model's, which the top
  level composes when it writes a record and splits when it reads one:
  the step and its progress (why its claimed run runs, when it last ran,
  its runs, repairs, rebases and rejections, the last verdict on its head,
  its last release), and on a goal's item the plan, its envelope, budget
  and growth so far, are the plan's (section 5); the phase, the attempts
  and the failures per class are the hub's (4.2); the inbox position,
  and a nonce naming the write that made the record, are the forge
  sub-model's (section 12), which knows where each record is and
  nothing else of what it says. A record that does not decode, because a person mangled
  it, holds its item for a person; so does a goal's plan that is not one
  the engine could have written, such as one edited into a cycle. Nothing
  the forge holds makes the engine panic.
- **Labels are a projection.** The engine writes labels from the record,
  so the forge's lists and filters show where things stand, and reads only
  the labels a plan declares as inputs. Editing any other label changes
  nothing the engine relies on. One label marks every item the engine
  tracks, which makes it the index the engine finds live work by
  (section 12). The labels the engine projects are configured, and it
  adds and removes only those, so a label a person sets is never taken
  off.
- **One live run at most.** An item's work is one run at a time: a
  supervisor never races itself, and two runs never write the same
  branch.

### 4.2 Lifecycle

```
waiting ─► due ─► claimed ─► running ─► applying ─► waiting, or done
                               │
                               ├─ parks ──► parked ──── a wake ────► due
                               └─ fails ──► a retry after backoff ─► due
                                            or held for a person
```

1. **Waiting** on dependencies, gates or the inbox: nothing to do yet.
2. **Due:** the plan says the item has work now (5.3): an agent step to
   run, or an engine action to take, such as merging or closing.
3. **Claimed:** the engine writes the claim into the record, naming the
   run and its attempt, before it assigns the run. Attempts count up
   across restarts, so a stale attempt stays fenced.
4. **Running** on a worker. Inbox events are relayed to the live run as
   they arrive, and its forge reads and outlets are served. A run that
   yields waits for its next event; a run that parks hands over its
   snapshot, and the item waits for its next wake.
5. **Applying** the outcome (4.4); then waiting for what its step does
   next (a change waits for CI once its pull request is open), or done.

A failure is retried after a backoff, within a number of attempts per
failure class, or the item is **held for a person**: it stops being due
until a person releases it. Held is a state of the engine's, shown by
whatever label the deployment projects for it, not a label the engine
reads.

The `work` sub-model, the hub, keeps the lifecycle. What building it
settled:

- **What the record says.** Waiting, parked, retrying (and the failure
  class), claimed, applying (and the outcome's comment), held (why, and
  the outcome it keeps) and done are written. Due and running never are:
  what is due is decided afresh after a restart, and a claimed run is
  running once a worker says so. A backoff is not written either: an item
  read retrying backs off again from the restart. So the hub is rebuilt
  from the records alone, fed to it as they are read, those claimed or
  applying first: a claim read is adopted (section 8), an application
  read is resumed.
- **Failure classes:** transient, permanent until something on the forge
  changes, the run's own, the agent's, lost, and an outcome the engine
  judged invalid; each with its own number of retries and its own
  backoff, jittered. A run refused before anything ran (the worker busy,
  its workstream held, the attempt a duplicate) is no failure: the item
  claims again after a pause.
- **An answer is acknowledged once it is durable:** an outcome once the
  record says it is being applied, a park or a failure once the record
  says so. Until then the worker keeps it, and its slot. If that record
  cannot be written, the item is held, still keeping the answer, and the
  slot stays taken until a person releases it.
- **Holds keep their outcome.** An item held with an outcome not wholly
  applied (one waiting for acceptance, one whose writes failed, one whose
  record could not be written) applies it again on release. A hold's
  reason is typed in the record: the plan's (as a code that means the same
  after a restart), failures of a class, a person's stop, acceptance,
  writes, or the record.
- **A person's stop** cancels the run and holds the item once the run has
  answered. An outcome it answered with still counts (what landed is the
  truth): it is applied, and then the item is held rather than waiting.
- **Held items stay in the working set,** and count against its capacity.
- **A mangled record** loses its attempt count: the item counts from the
  highest attempt its outcomes on the forge name, and the highest a
  worker lists, so its next claim is past every attempt that may still
  act.

### 4.3 The inbox

An item's inbox is what has changed on the forge since its record's
inbox position, among the things its step cares about: new comments on
it, such as a person's message; its dependencies and children finishing;
CI and reviews on its pull request; the items it subscribes to; a timer.

- **Derived, not queued.** The inbox is computed from forge state, so it
  survives anything that happens to the engine. A person's message is on
  the forge before it is delivered, and the position moves only once a
  run has taken what came before it.
- **Relayed or woken.** A live run gets inbox events as they arrive. An
  item without one is woken by them, as its step's wake rule says (5.4).
- **What is news,** as built: comments after the position on the item
  and on its pull request, save the engine's own; and its pull request's
  head, CI, state and verdicts, as a level against what the position
  took, not as a stream of changes. A person's message from the web,
  which the engine posts on the item, carries who it is for, and is news
  from them. A dependency the engine does not track leaves no news: its
  state is read afresh when a decision needs it.
- **Bounded, and held where it is.** The working set keeps a bounded
  inbox per item; what does not fit stays on the forge, since all that is
  queued is where to read from. Events the fleet could not deliver to a
  run yet are kept by the hub and relayed again once the run is placed,
  and the position moves only past what a run took, so a failed run's
  events go again to its retry.

### 4.4 Outcomes

- **Recorded, then applied.** A run's outcome is posted on its item
  first: a readable account with a typed block inside. That comment is
  the audit trail and the durable intent at once.
- **Applied against fresh state.** The engine reads the item, and what the
  outcome touches, afresh; asks the plan what the outcome writes; checks
  those writes against the rules; and makes them.
- **Keyed, with one commit point.** Every creation carries a key derived
  from the outcome, so a second attempt finds what the first one made,
  and sets (labels, reviewers, dependencies) are written as sets. The
  record's update comes last and is the commit point: until it lands, the
  outcome is still being applied, and a restart resumes it.
- **Judged twice.** The run judges an outcome against its outcome spec
  (agent-model.md, 4.4). The engine judges it again against the forge as
  it is now; an outcome that no longer fits, because the item moved on
  meanwhile, is stale and is not applied.
- **Held for acceptance** when the rules say so (section 7). A proposal is
  an outcome waiting for a person to accept it; until then nothing of it
  is applied.
- **Words are in the outcome's comment.** What an outcome says in words
  (a report, a session's reply, an escalation, a verdict's reasons) is in
  the comment posted first; the writes the plan asks for carry none of
  their own.
- **What may have happened is found, not repeated.** A request that
  provably never reached the forge did nothing; any other failure may
  have done what it asked, and may still land until a configured lifetime
  after it went out. So a write that failed so is looked for before it is
  tried again (a creation by its key, a merge by its head, a record by
  its nonce), and nothing more of its item's writes goes out until that
  lifetime has passed. After a restart the engine asks again for the
  writes of an outcome it was applying, and their keys are looked for
  from after the outcome's comment, the one bound an earlier life leaves.
  A new engine makes no call at all until a lifetime after it starts.
- **Records and wiki pages are read before they are written.** One a
  person changed since the engine last read it is not written over: the
  write fails, and the engine hears what it now says. Each one written
  carries a nonce of its write, so a read after an uncertain write tells
  its own landing from someone else's change.

### 4.5 Engine actions

Some steps need no LLM: merging a pull request whose gates hold, closing
an item whose children are done, opening the pull request for a pushed
change. The engine takes these itself, as writes planned and checked like
an outcome's, with the same keys and commit point. As built, the plan's
actions are opening a change's pull request (keyed by its branches: the
newest pull request for them, open or not, is the one made), reopening
it, merging it at exactly the head the decision saw, closing an item and
deleting a branch.

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
person's acceptance of the step; the rules check them too, and take
gates of their own (read-only, green CI, a review at a permission, a
run's and a goal's spend), for the top level to derive from the step.
Checks-only gates are not built (section 16).

### 5.2 A plan

- **A graph of steps** under a goal: each step with its primitive, its
  spec, its dependencies, and the gates it adds. A step's brief carries
  the outcomes of the steps it depends on, which is how results flow
  through a plan.
- **Written by agents,** as an outcome: a session proposes one, an agent
  step extends one. Nothing in the engine requires work to follow a fixed
  process.
- **Checked before it exists:** known primitives only, no cycles, a
  bounded size, writes only to the deployment's repositories, an estimate
  within budget. A plan that fails a check is feedback for the run that
  wrote it, like any outcome that breaks its spec.
- **Accepted when the rules say so.** A plan that lands on a protected
  branch, or spends beyond a threshold, waits for a person's acceptance.
  Once accepted, the engine creates its items in one repeat-safe
  application, each with its step in its record.
- **An envelope** says how far an accepted plan may grow on its own: how
  many steps of which primitives, into which branches and repositories.
  Growth within it needs no new acceptance; growth beyond it does.
- **Templates** are plan shapes and step charters that have worked, which
  an agent may reuse or ignore: a library that speeds things up and gates
  nothing.

What building the plan settled:

- **A step is done only once the steps it added are,** so a step that
  comes after it waits for those too, and the plan's checks order the
  whole graph, the added steps included. Accepting a plan makes its items
  in that order, dependencies first, and a step waits while fewer of its
  dependencies are found than it names.
- **Growth is repeat-safe.** It writes two records, the goal's and the
  growing step's, which may land in either order across a restart:
  applied again, growth finds its steps already joined to the goal (the
  same names, after the same steps, added by the same run) and asks for
  the same writes as if they were new.
- **An accepted growth widens the envelope** by what it added, so later
  growth within the new bounds needs no acceptance. A supervising
  session's tasks join its goal's plan as steps of no dependency, within
  its envelope and budget, so tasks are no way around it.
- **A goal's plan is forge data,** checked as it is read: one a person
  edited into something the engine could not have written holds the
  item.

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

### 5.3 What is due

The `plan` sub-model is pure. Given an item's record and what the
working set holds about it and its relations, it says what is due now, whether an
inbox event wakes the item, what a run's charter is, and what an outcome
writes. Everything it needs is in the record and the forge, so a restart
loses nothing it knew.

What building it settled:

- **A change's way to landing.** With no branch yet, a run produces it;
  with a branch and no pull request, the engine opens one; with one open,
  a conflict or a moved base asks for a rebase run, failed CI or changes
  asked for on the head for a repair run, CI pending or a mergeability
  not yet known waits, and a clean head whose reviews and gates hold is
  merged at exactly that head. Merged, the step is done; closed unmerged,
  it is held.
- **Repairs and rebases are bounded apart.** A change repaired for
  failures as often as the limits allow is held when it needs another;
  rebases have a limit of their own, higher, since a busy base moves
  often and that is no failure of the change.
- **Waits on the forge are bounded.** A change that waits for CI, a
  review or mergeability longer than a configured stall, counted from its
  head's push or its last release, is held, and its goal's session is
  told first if it has one.
- **A release lifts what it releases.** Repairs and rebases count again
  from the release, and a person's decision made before it counts for
  nothing, so a released item is not held again at once for the same
  cause.
- **A claim says why.** A run that is due is claimed with a progress
  write recording why it runs (an agent step's work, producing a change,
  repairing it and why, reviewing it at a head, a session's turn) and
  when, so applying its outcome, and a session's next wake,
  need nothing kept in memory. Every outcome applied clears it, last.
- **Sessions end** with an outcome that finishes them, or when a person
  closes their item; a supervising session is done once its goal's steps
  are. An item a person closes is done, whatever its step.
- **Time** in the record and in what the plan reads is the engine's
  clock: the forge's times are mapped onto it as they are read
  (section 13).

### 5.4 Wakes

Each step says what wakes it: its own changes, its dependencies or
children, the items it subscribes to, a person's message, a timer. A wake
rule may batch, letting events through once there are at least N or the
oldest is older than T, so a supervisor watching twenty changes wakes
once for a burst of them.

As built, only sessions have wake rules: every other step is asked what
is due whenever what it reads changes. A person's message is never
batched, since a person waits for the answer, and every event is read,
however many come from sources the rule does not name, so no message is
lost behind them.

## 6. Sessions

A session is an item whose step is a session: a conversation with a
person, and, once a plan it proposed is accepted, the supervisor of that
plan's goal.

- **Chatting.** One live run holds the LLM conversation in its agent
  process: it yields after each reply and waits for the next message. Idle
  past a threshold, it parks: its snapshot goes to the engine and its
  worker slot is freed. The next message starts a run on any worker,
  resumed from the snapshot.
- **Supervising.** The session subscribes to the goal's items and wakes on
  what needs judgement: a step held for a person, an escalation from a
  run, the budget crossing a threshold, a stall, a person's message. By
  default each wake is a fresh run whose brief is the goal, the plan's
  status, what changed since its last turn, and the goal's notes
  (section 10). It acts within the plan's envelope, asks a person beyond
  it, notes what its next turn should know, and ends.
- **Resume or start fresh** is the engine's choice at each wake, and the
  run cannot tell the difference. A snapshot is a cache the engine never
  looks inside: a lost, oversized or stale one means starting fresh from
  the forge, which always works. Which to prefer, per session or per
  step, is policy that can change without touching the mechanism. A
  session's step says it: by default a chatting session resumes and a
  supervising one starts fresh; either may be told always or never.
- **Commitments at once, conversation per turn.** Spawning a task,
  proposing a plan or opening a change is an outcome, applied as in 4.4.
  The transcript (a person's messages and the session's replies) is on the
  session's issue turn by turn; tool calls link to traces rather than
  being copied.
- **Escalations go to the supervisor first.** A step under a goal that
  needs a decision wakes the goal's session, which holds the context of
  the conversation, before any person is asked. The escalating item is
  held, and the session may release it with an outcome of its own.
- **A session's outcomes,** as built: a reply (the outcome's comment is
  the reply), a plan proposed, steps added to its goal's plan while it
  supervises, tasks, the release of one of its goal's held steps, and a
  last turn that finishes it.
- **A session may change code itself**, within its grants: its change is
  a change step like any other, with the same lifecycle and rules.

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

- **Four answers,** each saying what clears it: allow; wait, for facts
  the forge has yet to report (green CI, an approving review, on the exact
  head), which only those facts clear, never a person; a person's
  acceptance, at a permission on the repository; or refuse, which nothing
  the engine waits for clears. A check answers the strictest of what it
  found, and says why. A landing on a protected branch without its CI or
  review waits, or is refused once CI has failed; it never asks a person
  instead.
- **Checks are pure,** over facts the top level gathers: before a run
  starts (where it may push, its budget on top of what was spent), before
  a write, and before a person's request is acted on.
- **Reviews:** a review counts from a person with write permission at
  least (the deployment may ask for more on protected branches), never
  from the engine's own user; a request for changes stands, whatever head
  it was on, until the same reviewer approves.
- **Plans:** a plan needs acceptance past a size, past an estimate, or
  when it lands on a protected branch; its estimate counts on top of its
  goal's spend.
- **Spend** is counted in one unit of the deployment's, bounded per run,
  per goal and per deployment.

## 8. The fleet

The fleet sub-model knows the workers and the runs they host.

- **Workers** dial in and say how many run slots they have and which
  workstreams they hold checkouts for. An item names its workstream, so
  the runs of one piece of work find their checkout cached.
- **Placement:** a claimed item's run goes to a worker with a free slot,
  preferring one that holds its workstream.
- **Attempts are fenced.** Every assignment carries its attempt; once the
  engine has cancelled or replaced an attempt, whatever it still sends is
  dropped. As built, everything a worker sends is dropped unless its
  attempt is the item's live claim; only a cancelled attempt's answer is
  still taken. No attempt of an item is placed while a worker may still
  host another.
- **Answers are acknowledged once durable.** A worker keeps a run's
  answer, and the slot it takes, until the engine acknowledges it, and
  sends it again after every hello. The fleet hands each answer on once,
  and acknowledges it only once the engine has made it durable (4.2), or
  once it is for an attempt fenced off; a copy that comes meanwhile is
  dropped unacknowledged, and one for an attempt the engine no longer
  knows is acknowledged again.
- **Refusals are not failures.** A worker that refuses an assignment as
  busy gets nothing more until it frees a slot, and the attempt is placed
  again, there or elsewhere. A hello is turned away only when the fleet
  has no room for another worker, and room is kept for the runs workers
  list; a listing beyond it is cancelled, not refused. A worker's
  workstreams are bounded, the one used longest ago evicted.
- **Losing contact.** A worker that drops its channel keeps its runs for a
  grace period; on reconnecting it says what it hosts, and the engine
  keeps or cancels each run. Past the grace, the runs are presumed lost
  and their items go to retry. An assignment in flight when the channel
  drops is presumed lost only at the grace's end.
- **Restarting.** The engine adopts the claims its records hold, and then
  says it has read them all. A claim no worker reports within the grace is
  lost. A run a worker reports that no claim adopts is told to the engine
  (it names the attempts of a mangled record, 4.2), and is cancelled
  once a grace has passed both since the reading ended and since it was
  first listed. Until the reading ends, the engine is to start nothing
  new, and it never cancels a claim before adopting it.
- **Every start and adoption ends once:** answered, lost, withdrawn (the
  engine cancelled it), or refused (busy, the workstream held, a
  duplicate).
- **Relaying:** inbox events and cancels down to runs; host calls up, and
  their answers back, each call answered once. An inbox event that
  reaches no worker comes back to the engine, which keeps it; one a
  worker bounces comes back too, but without saying which it was
  (section 15).

## 9. Briefs

The engine renders the brief of every run (agent-model.md, 4.1) from
typed sections the step selects: the item and its lineage, the comments
since the run's last turn, the outcomes of its dependencies, the CI
failures on its pull request's head with their output, review comments,
its pull request against its base, its earlier attempts and why they
failed, the plan's status, the index of the notes in its scope
(section 10), the template it follows. Each section has a byte budget,
and what does not fit is cut, with a line saying how much. What the LLM
needs to know reaches it as sections, never folded into its instructions
as prose.

What building the brief sub-model settled:

- **Sections follow why the run runs,** and those it runs for are
  required: every run has the item (required), the comments, its
  attempts and the notes, its dependencies if it has any, and its
  template if it follows one; a run that may grow the plan, and a
  supervising session's turn, add the plan; a repair adds what it
  repairs, required: CI for failed CI, reviews for changes asked for, the
  pull request for a moved base or a conflict; a review adds the reviews.
  A brief without a required section fails, saying which and why (its
  read failed, brought more than a read may, or came too late), and the
  engine decides whether the run waits for another try; an optional one
  is marked missing, and why.
- **What goes first** when a section does not fit is its kind's: the
  item's farthest ancestors, then the tail of what is left; the oldest
  comments and attempts; for dependencies, CI and reviews, each part's
  excess over an even share, so every part reaches the brief, and a CI
  failure keeps the end of its output; the notes' index by whole lines,
  from the last, keeping the line that says how many entries did not
  fit; and the tail of the rest. Each cut is a line, `[N bytes cut]`;
  a list of dependencies too long to read ends with `[N items cut]`.
  Cuts never split a UTF-8 sequence. When the sections together exceed
  the brief's budget, short ones stay whole and long ones share what is
  left evenly.
- **A section is read as it is cut,** each read asking for no more than
  its section can keep, and a brief gathers within one deadline of its
  own: what has not arrived by then is missing. A brief that has answered
  lets its reads still in flight end without it, and a brief refused for
  want of room is told when there is room again.

## 10. Notes

Notes are what temper learns over time and passes on: a convention a
reviewer keeps asking for, why an approach was dropped, which test is
flaky, what a person prefers. They are to runs what memory is to a coding
agent at a terminal.

- **An entry** has a one-line description, a body, a scope and
  references. The scope is the deployment, a repository or a goal; the
  references name items, closed ones included. Each entry says who wrote
  it: a person, or a run and the item it ran for.
- **In the forge's wiki,** like everything that must last and belongs to
  no one item (section 12), where people can read, correct and delete
  entries: a page per entry, whose first line is its description. A
  repository's notes are in its wiki, a goal's under the goal's own path
  in its repository's wiki, and the deployment's in the wiki of a
  repository the deployment names as its home. Notes are hints: where a
  note and the forge disagree, the forge is right.
- **Written through an outlet.** A run writes a note with its `note`
  outlet, which the engine applies like any outcome (4.4): keyed,
  repeat-safe and checked by the rules, which may want a person to accept
  a note of wide scope. People write notes directly.
- **Read in two steps.** A brief carries the index of the notes in its
  scope, one line per entry, within its section's budget, saying how many
  more there are when they do not all fit. A run asks for the entries it
  wants with a `recall` read, by name or by a search of their
  descriptions, and follows their references on demand. The working set
  holds the indexes; entries are fetched when they are asked for.
- **Curated.** A session on a slow timer can merge duplicates, rewrite what
  has drifted and drop what is stale, through the same outlet.
- **A supervisor's memory** is its goal's notes (section 6): what one turn
  leaves for the next.
- **Knowledge of the code itself** (conventions, pitfalls, architecture)
  belongs in the repository, as `AGENTS.md` does: versioned and reviewed
  with the code, and found by runs in their checkout. A note that proves
  durable can move there through a change.

The notes sub-model knows scopes, entries and indexes, and nothing of
plans: which notes are in a run's scope, what of their index fits a
brief's budget, what a search finds, and what a write changes. What
building it settled:

- **Indexes are kept per scope in use,** learned from the scope's wiki: a
  listing of its pages, then reads of the pages it does not know at their
  revision. A scope runs one wiki operation at a time, so answers are
  taken in the order they were asked; changes that come during a pass
  wait for the next one, so no stream of changes holds the calls waiting
  back. A scope a call needs is pinned while it waits; the others are
  kept until another needs the room, the least recently used evicted
  first.
- **A search** matches descriptions in the kept indexes; a recall, by
  name or by search, reads each page afresh, and answers with it as the
  wiki served it, never from the index.
- **A note revised** names the revision its run recalled. The page is
  read afresh before the edit, and the note is refused as moved if the
  page changed since. The wiki's API has no conditional edit, so between
  that read and the edit there is a window in which a person's change can
  still be lost.
- **The wiki's pages** are read and written by the forge sub-model, in
  the same request budget as everything else (section 12).

## 11. Views

What runs report (agent-model.md, section 7) reaches the engine through
the workers, best effort. The views sub-model:

- **streams it live** to the people watching a run or an item, such as a
  session's text as it is written;
- **keeps traces** for a retention period, with content as the capture
  policy says; the engine sends the policy with each assignment, so there
  is one;
- **shows items' states** as they change, for the web's boards.

Nothing the engine decides depends on a fact arriving. What building the
views settled:

- **Watching.** A person watches a run, an item, or a repository's
  board. A watch starts from a snapshot the engine gives, delivered
  first; then a run's reports go to the watchers of the run and of its
  item, and an item's phase to the watchers of the item and of its
  board. Reports and traces carry the run's attempt, so a retry's never
  mix with an earlier attempt's.
- **A slow watcher never holds the engine up.** A watcher has one
  delivery in flight, whose time the engine bounds; what comes meanwhile
  waits in a bounded backlog, and when it overflows, what waits is
  dropped and the watcher is told how much it missed with its next
  delivery, as it is of a delivery its stream did not take and of a
  report dropped for want of room. A watch past the limits is refused.
- **Capture policy** is per kind of report: nothing, its shape, or its
  content too.
- **Traces** are batched in memory and appended to the store, which
  takes its operations in the order they are asked for. Expiry is by
  time: a periodic sweep asks the store to forget what is past the
  retention, the first a period after the engine starts, so what an
  earlier engine kept is forgotten too. The views' times are the
  engine's clock, which starts again with it; the store maps them to wall
  time. A batch the store fails to take is dropped and counted, never
  retried: traces are expendable.

## 12. The forge

The forge sub-model is the engine's knowledge of the forge and its only
way to change it. It holds live work, not the forge: what it holds, and
what it costs to start, grow with the work in progress, never with the
forge's history. Anything else, closed items included, is a read away.

- **Items and the wiki.** What belongs to an item is on the item: its
  record, its outcomes, its transcript, all of which end with it. State
  that belongs to no one item or repository and has no lifecycle, such as
  notes, is in the forge's wiki.
- **A working set, not a copy.** It holds what the engine's decisions
  need about the items that are not done: their records, their pull
  requests' heads, CI and reviews on those heads, whether their
  dependencies have finished, and the indexes of the notes in their
  scopes. An item enters when the engine creates it
  or takes it in, and leaves once it is done, which on the forge means
  closed. Closed issues and pull requests are not held, whether the
  engine never tracked them or has finished with them.
- **Bounded, and refused at the entrance.** The working set's capacity is
  a limit. When it is full, new work waits to be taken in; nothing already
  taken in is dropped.
- **Everything else on demand.** Long comment threads, an item's history,
  a finished dependency's outcome, a closed item that new work refers to,
  whatever a run asks to read: fetched when a brief or a run needs it,
  within a budget, and not kept.
- **Starting** reads what is live. The engine lists the open items that
  carry its label, and those that carry the label handing work to it
  (4.6), and reads the records of the ones that changed since its cache
  last saw them: with a warm cache, a list per repository and the
  changes; with a cold one, every live record. A slow pass over all open
  items catches one whose label a person removed.
- **Keeping up:** incremental reads of what changed since the last one,
  sooner when a webhook hints at a change. Polling is the backstop, so a
  lost webhook costs only latency, and the cost of keeping up follows the
  rate of change, not the number of items.
- **Fresh reads** before every write, and for the forge reads runs ask
  for.
- **Writes** are typed operations, serialised per item, retried when the
  failure is transient, with keys that make every creation findable.
- **A request budget** keeps the engine within the forge's rate limits,
  with reads for writes ahead of keeping up.

What building it settled:

- **What the working set holds** of an item: where its record is and the
  inbox position it names, its labels, its pull request's head, where
  its base is, CI on its head and the verdicts on it (the latest of each
  reviewer, the engine's own excluded), and its inbox (4.3). Whether its
  dependencies finished is read afresh when a decision needs it, unless
  they are tracked themselves; the notes' indexes are the notes
  sub-model's (section 10).
- **Starting is cold** for now: every live record is read. A warm cache
  is not built (section 16).
- **The forge's clock, not the engine's.** Every time a listing compares
  is the forge's (an item's updated time, when a page was made), never
  the engine's own clock's. Listings follow Forgejo: items updated at or
  after a time, at a second's resolution, least recently updated first.
  The sub-model pages by time where it can and lists again from the
  newest time it saw, inclusive, so an item that moves during a listing
  costs a duplicate, never a miss; an item listed again at the time it
  last showed counts as changed only if that second had passed when the
  listing was made.
- **What moves no updated time is polled.** CI reporting and a base
  moving leave an item's updated time alone on Forgejo, so the pull
  requests held are read again when a webhook names their head's commit
  or their base branch, and on a backoff of their own whatever their
  state, doubling while nothing changes, up to the slow pass's interval.
  A pull request's state is read whatever room its inbox has.
- **The slow pass** lists every open item of a repository, a page at a
  time at the lowest priority, and reads a bounded few of the unlabelled
  ones on each page for a record of the engine's, a different few each
  cycle; at a cycle's end it reads the items held that it did not see
  open, to learn whether they are still there.
- **Leaving.** An item leaves the working set when it is closed or the
  forge no longer has it, and the engine is told which. One the forge
  keeps refusing as forbidden is retried after a backoff, and the engine
  is told once the retries are spent. An item refused for want of room
  waits on the forge, where its labels find it again, and the engine is
  told when there is room.
- **The budget's order:** the engine's fresh reads first, a write's own
  checks among them, then writes, then keeping up, then the slow pass;
  a share of each window is kept for the last two, so neither starves.
  A refusal for the rate stops every call until the reset it names.

## 13. Below the model

What the protocol and io layers owe the model, to be designed after it:

- **The forge:** each provider's HTTP API, Forgejo first; pagination;
  webhooks and their signatures; the record and outcome blocks inside
  comments, decoded into model entities; credentials. As the forge
  sub-model's vocabulary fixes it: listings sorted least recently updated
  first (Forgejo's default is newest first), each page saying the forge's
  time as it was made; comments paged by id and reviews and statuses by
  number, mapped onto Forgejo's own paging; a revision for each comment,
  changing whenever its body does; the markers that carry a creation's
  key, the person a message is written for, and the record's and a wiki
  page's nonce; and a failure is unavailable only when the request
  provably never reached the forge, anything later being a timeout.
- **Times:** the forge's, mapped onto the engine's clock as the plan
  reads them, and the times records keep, mapped so that they mean the
  same after a restart, whose clock starts again.
- **Workers:** the authenticated, framed channel of worker-model.md.
- **People:** the web: HTTP, live streams, and signing in through the
  forge.
- **The store:** snapshots, traces, and a cache of the working set, on
  disk. It takes its operations in order, and maps the views' times,
  which start again with each engine, to wall time.
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
