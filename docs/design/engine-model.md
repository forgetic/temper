# The engine's model layer

Provisional, 2026-10-02. What the temper engine does, as a model layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of `programming-model.md`; the worker it drives is
described in `worker-model.md`, and the agent in `agent-model.md`. Each
part's details are settled as it is built; what is still open is listed
in section 14.

## 1. In one page

- **The engine is where work is decided.** It is the forge's only API
  client and the one place plans and rules live. It decides what happens
  next: which run to start, what a run's outcome changes on the forge,
  when to try again, when to ask a person. Workers host runs; agents do
  the LLM work.
- **The forge is the truth, and the engine keeps only caches.** What must
  outlive the engine's process is written to the forge: each item's
  record, the outcomes being applied, the transcripts. The engine's own
  store holds a mirror of the forge, snapshots of parked runs and traces,
  all of them rebuildable or expendable. A restart is a rescan.
- **Everything is an item.** An item is an issue or pull request the
  engine tracks, with a record of its own, an inbox, and at most one live
  run. Chat sessions, tasks, goals and the steps of a plan are all items;
  what differs is the step each one carries.
- **Plans are free; primitives and rules are fixed.** Agents compose
  plans for the task at hand, as graphs of a few primitives: agent steps,
  changes, waits and sessions. The primitives are the engine's code. The
  rules are the deployment's, and a plan can add to them but never loosen
  them, which is what makes plans written by agents safe to run.
- **Level-triggered.** What is due follows from the forge's state, not
  from the events that announced it. Webhooks are hints that make the
  engine look sooner; polling is the backstop. Before writing, the engine
  reads afresh and decides again.
- **Every write is repeat-safe.** Creations are keyed, sets are written as
  sets, and an outcome is recorded before it is applied, so an application
  that was interrupted resumes where it stopped.
- **The model is complete** (programming-model.md, section 4). A world of
  models and fakes runs everything the engine does, with no protocol and
  no io (section 13).
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
├── temper-engine-model-forge        the mirror and the client: sync, reads, keyed writes, request budget
├── temper-engine-model-fleet        workers: slots, placement, attempts, contact, relaying
├── temper-engine-model-brief        a run's brief, from typed sections within a byte budget
└── temper-engine-model-views        facts in; live streams and retained traces out
```

The tree follows programming-model.md, 4.5: each sub-model has its own
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
- **Labels are a projection.** The engine writes labels from the record,
  so the forge's lists and filters show where things stand, and reads only
  the labels a plan declares as inputs. Editing any other label changes
  nothing the engine relies on.
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

### 4.5 Engine actions

Some steps need no LLM: merging a pull request whose gates hold, closing
an item whose children are done, opening the pull request for a pushed
change. The engine takes these itself, as writes planned and checked like
an outcome's, with the same keys and commit point.

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
acceptance, a checks-only run passing on a head.

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

The `plan` sub-model is pure. Given an item's record and what the mirror
holds about it and its relations, it says what is due now, whether an
inbox event wakes the item, what a run's charter is, and what an outcome
writes. Everything it needs is in the record and the forge, so a restart
loses nothing it knew.

### 5.4 Wakes

Each step says what wakes it: its own changes, its dependencies or
children, the items it subscribes to, a person's message, a timer. A wake
rule may batch, letting events through once there are at least N or the
oldest is older than T, so a supervisor watching twenty changes wakes
once for a burst of them.

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
  status, what changed since its last turn and the notes it left itself.
  It acts within the plan's envelope, asks a person beyond it, writes its
  notes and ends.
- **Resume or start fresh** is the engine's choice at each wake, and the
  run cannot tell the difference. A snapshot is a cache the engine never
  looks inside: a lost, oversized or stale one means starting fresh from
  the forge, which always works. Which to prefer, per session or per
  step, is policy that can change without touching the mechanism.
- **Commitments at once, conversation per turn.** Spawning a task,
  proposing a plan or opening a change is an outcome, applied as in 4.4.
  The transcript (a person's messages and the session's replies) is on the
  session's issue turn by turn; tool calls link to traces rather than
  being copied.
- **Escalations go to the supervisor first.** A step under a goal that
  needs a decision wakes the goal's session, which holds the context of
  the conversation, before any person is asked.
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
- spending is bounded per run, per goal and per deployment;
- who may accept, steer, cancel or release follows their permission on
  the repository, as the forge reports it.

## 8. The fleet

The fleet sub-model knows the workers and the runs they host.

- **Workers** dial in and say how many run slots they have and which
  workstreams they hold checkouts for. An item names its workstream, so
  the runs of one piece of work find their checkout cached.
- **Placement:** a claimed item's run goes to a worker with a free slot,
  preferring one that holds its workstream.
- **Attempts are fenced.** Every assignment carries its attempt; once the
  engine has cancelled or replaced an attempt, whatever it still sends is
  dropped.
- **Losing contact.** A worker that drops its channel keeps its runs for a
  grace period; on reconnecting it says what it hosts, and the engine
  keeps or cancels each run. Past the grace, the runs are presumed lost
  and their items go to retry. After an engine restart, the same
  reconnection tells the engine which claims are still live.
- **Relaying:** inbox events and cancels down to runs; host calls up, and
  their answers back.

## 9. Briefs

The engine renders the brief of every run (agent-model.md, 4.1) from
typed sections the step selects: the item and its lineage, the comments
since the run's last turn, the outcomes of its dependencies, the CI
failures on its pull request's head with their output, review comments, its
earlier attempts and why they failed, the plan's status, the notes it left
itself, the template it follows. Each section has a byte budget, and what
does not fit is cut with a note saying so. What the LLM needs to know
reaches it as sections, never folded into its instructions as prose.

## 10. Views

What runs report (agent-model.md, section 7) reaches the engine through
the workers, best effort. The views sub-model:

- **streams it live** to the people watching a run or an item, such as a
  session's text as it is written;
- **keeps traces** for a retention period, with content as the capture
  policy says; the engine sends the policy with each assignment, so there
  is one;
- **shows items' states** as they change, for the web's boards.

Nothing the engine decides depends on a fact arriving.

## 11. The forge

The forge sub-model is the engine's knowledge of the forge and its only
way to change it.

- **A mirror** of what the engine tracks and what its plans observe:
  items, their comments and records, pull requests and their heads, CI,
  reviews, dependencies. Bounded by count.
- **Sync:** a full read at start, then incremental reads of what changed
  since the last one, sooner when a webhook hints at a change. Polling is
  the backstop, so a lost webhook costs only latency, and the cost of
  keeping up follows the rate of change, not the number of items.
- **Fresh reads** before every write, and for the forge reads runs ask
  for.
- **Writes** are typed operations, serialised per item, retried when the
  failure is transient, with keys that make every creation findable.
- **A request budget** keeps the engine within the forge's rate limits,
  with reads for writes ahead of background sync.

## 12. Below the model

What the protocol and io layers owe the model, to be designed after it:

- **The forge:** each provider's HTTP API, Forgejo first; pagination;
  webhooks and their signatures; the record and outcome blocks inside
  comments, decoded into model entities; credentials.
- **Workers:** the authenticated, framed channel of worker-model.md.
- **People:** the web: HTTP, live streams, and signing in through the
  forge.
- **The store:** snapshots, traces, and a cache of the mirror, on disk.
- **Configuration:** a deployment's repositories, rules, templates and
  limits.

## 13. The world

The engine's world runs the model against fakes that share none of its
types (programming-model.md, section 11):

- **a forge** with issues, pull requests, comments and labels; CI that
  passes, fails or never reports; reviews; merges that conflict; webhooks
  that come late or not at all; requests that fail or hit a rate limit;
  and people who comment and edit;
- **workers** hosting scripted runs: outcomes, failures, yields, parks,
  lost contact and reconnection;
- **people** who chat, accept and reject plans, and release held work.

The engine, workers and agents meet in a larger world, with the fake LLM
provider in place of a real one.

## 14. Open questions

- **People on the forge:** whether a person's messages are written with
  their own forge credentials or by the engine on their behalf.
- **Where templates live:** deployment configuration first; later, a
  repository the engine reads through the forge's API.
- **The rules' vocabulary,** and what an envelope can bound, settled as
  they are built.
- **Snapshots:** their size limit and how long the engine keeps them
  (worker-model.md, section 10).
- **Checks without an LLM:** a checks-only run on an exact head, used as
  a gate (worker-model.md, section 10).
- **Saved-work branches:** their names and when the engine deletes them,
  with the worker.
- **Provisioning a repository** (labels, webhooks, CI): an engine command,
  later.
