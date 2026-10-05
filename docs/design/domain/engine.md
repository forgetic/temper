# The engine

Provisional, 2026-10-04. What the temper engine is, as a domain layer:
its parts and how they fit, how a decision reaches the store before
anything leaves the engine, how it restarts, how it runs agents, and the
capabilities it keeps from today (the fleet, briefs, notes, views,
accounts). The model is core.md; tasks are tasks.md, authority
authority.md, connectors connectors.md and the forge forge.md, people
people.md. The worker it drives is worker.md, the agent smith, as
agent.md says temper uses it. The
mechanics are those of skein's `docs/foundation/programming-model.md`.
What is still open is listed in section 17.

## 1. In one page

- **The engine is where work is decided.** It keeps the tasks, checks
  every action against authority, starts agents' runs on workers, steps
  procedures, asks people, and drives external systems through its
  connectors. Workers host runs; agents do the LLM work; the store keeps
  what must outlive the process.
- **A root domain and its children.** The tasks child domain is the hub;
  authority is policy; the fleet, people, accounts, briefs, notes and
  views are capabilities; each connector is a subtree. The root routes
  between them and alone faces the protocol layer (section 3).
- **A decision commits once,** and nothing leaves before it does
  (section 5). The engine's state may run ahead of the store by the
  commits in flight; every output (an effect, an assignment, an
  acknowledgement, an answer to a person, a message relayed to a run)
  waits until the commit it follows from is durable.
- **A restart reads the store,** adopts the runs its workers still host,
  reads afresh from the connectors what they own of the live work, and
  looks for what uncertain effects made, before deciding anything new
  (section 6).
- **Agents run from their task.** A run is given its task's charter, a
  brief built for why it runs, its transcript if it resumes, and the
  engine's tools its authority allows. Each turn it takes is committed
  as it ends, with what it spent and what it read, so a run lost
  mid-conversation loses only the turns its worker had not had
  acknowledged, which are bounded; every call that changed something is
  committed and answered once, however often it is asked (section 7).
- **Bounded.** The engine holds live work only: live tasks, their
  inboxes and proposals, the runs in flight, the connectors' working
  sets. Ended tasks, transcripts, history and notes are in the store,
  loaded when wanted. Every limit is refused at its entrance.
- **The domain is complete** (programming-model.md, section 4): a world
  of domains and fakes runs everything the engine does, with no protocol
  and no io (section 15).

## 2. The engine in the system

```
people    the web: requests in, answers and live views out
engine    tasks, authority, runs, connectors; the store's only client
store     what must outlive the engine's process
workers   dial in: assignments, messages relayed and cancels down; turns, tool calls and answers up
agents    reached only through their workers
systems   reached only through their connectors: the forge's API and webhooks
```

- **Workers dial in** (worker.md, section 2). The engine assigns runs,
  relays messages and cancels, answers the tool calls a run makes, and
  acknowledges its turns and its answer once each is committed. It
  never connects to a worker.
- **The engine never reads a repository.** It works from what its
  connectors read of their systems; what a checkout holds is for the run
  to find (agent.md, section 2).
- **People reach temper through the engine's web** (people.md), and the
  forge, where they may also read, is a system temper drives, not where
  they act.
- **The store is the engine's alone.** Nothing else reads or writes it,
  so the engine needs no compare-and-swap against anyone: its own
  ordering of commits is the truth.

## 3. Structure

```
temper-engine-domain                  the root: routing, commits and loads, restart, runs, translations
├── temper-engine-domain-tasks        the hub: tasks, batches, lifecycle, inboxes, wakes, proposals
├── temper-engine-domain-authority    authority's order and checks; the rules and the projects' policies
├── temper-engine-domain-people       people: signing in, roles, requests, inboxes
├── temper-engine-domain-fleet        workers: slots, placement, attempts, relaying
├── temper-engine-domain-accounts     LLM accounts: refreshing, granting
├── temper-engine-domain-brief        a run's brief, from typed sections within a byte budget
├── temper-engine-domain-notes        notes: scopes, entries, indexes
├── temper-engine-domain-views        live streams out; traces kept
└── temper-engine-domain-forge        the forge connector, a subtree of its own (forge.md, section 3)
```

The tree follows programming-model.md, 4.5: each child domain has its
own vocabulary, limits and world; a parent owns its children's state and
routes between them; siblings share no domain types, and the root
translates between them with small total functions.

- **`tasks` is the hub,** as `work` is today: it knows a task's
  lifecycle and nothing of runs, connectors or the web.
- **`authority` is policy:** decisions over data, keeping only the rules
  and policies in force.
- **The others are capabilities,** each knowing its own concern: the
  fleet the workers, people the web's parties, accounts the LLM
  credentials, the brief a run's context, notes what temper learns,
  views who watches.
- **A connector is a subtree,** whose top is a child of the root. The set
  is closed: the root knows every connector and matches on them
  exhaustively; nothing below the root knows any but itself
  (connectors.md, section 2).

The root's vocabulary is what crosses the domain boundary: the store's
commits and loads, the workers' channel, the web's requests, each
connector's protocol (for the forge, its calls and webhooks' hints), and
the OAuth server's (credentials' own document).

## 4. The top level

The root keeps little of its own:

- **the commits in flight,** each with the outputs waiting for it
  (section 5);
- **the loads in flight,** each with what waits for its rows;
- **the next task number,** and every other number the engine gives,
  committed as they are used;
- **the runs in flight:** for each, its task's number, its attempt, and
  what its assignment was made from, so an answer can be routed and a
  brief rebuilt (section 7);
- **the calls committed** for each live task since its last committed
  turn, by attempt and name, with their answers (7.3).

Everything else is a child's. A decision is a route through several
children, completed within one step (programming-model.md, 4.5), then
one commit. What must first wait for a load or a read is a state of its
own (a task preparing, a brief gathering), and its decision is made in
the step its last answer arrives. The usual routes:

| What arrives | Route | Then |
|---|---|---|
| a run's turn | fleet → root: tasks (the inbox taken), authority (spend), views | commit; the turn acknowledged |
| a run's tool call | fleet → root: authority (the check), then tasks, notes or a connector | commit; the call answered |
| a run's answer | fleet → root: tasks (result checked, task closing or idle), authority (spend), a connector (effects) | commit; the answer acknowledged; effects made |
| a procedure's step | its connector → root: authority (each effect, each batch), tasks | commit; effects made; tasks started |
| a connector's event | connector (classify for each subscriber) → root → tasks (inboxes, wakes) | commit; due tasks run, procedures step |
| a person's request | people → root: authority (their role), tasks | commit; the person answered |
| a task due | tasks → root: authority (the run may start); the task preparing while the brief gathers and its transcript loads; then fleet (place) | commit the claim; assign |
| a timer | its child → root | as its child says |

The hand-offs are short and acyclic, and each entry point completes
them before it returns.

## 5. Decisions and the store

### 5.1 Commits

- **One decision, one commit.** What a decision changes in every child
  it touched (a task made, an inbox taken, a proposal accepted, an
  outbox entry added, a procedure's state, a note) is one commit:
  records written or removed, as one.
- **Numbered and in order.** The root numbers its commits and the store
  applies them in that order. Its answer is that a commit is durable,
  which makes every earlier one durable too.
- **The state runs ahead.** A child's state changes as the decision is
  made, not when it commits, so the next decision sees it.
- **Commits in flight are bounded,** and the store's answers are always
  taken, so the bound never stops the answer that frees it. Past the
  bound, a decision that would need a commit is not taken, and what
  asked for it is told so: a person's request is answered busy; a run's
  tool call is answered busy, and the run asks it again after a backoff
  (smith's `run.md`, 5.2); a turn or an answer is answered busy, and its worker
  sends it again after a backoff (worker.md, section 8); a connector
  keeps its events and its outbox's outcomes until there is room. That
  is backpressure on every entrance at once.
- **A failed commit stops the engine** (core.md, section 4). The step
  that hears it asks the shell to stop, and nothing waiting for that
  commit, or any later one, goes out. A restart starts from the last
  durable commit.

### 5.2 What waits for a commit

Every output that someone outside the engine could act on is tagged
with the commit it follows from, and released once that commit is
durable:

- **effects:** an outbox entry is handed to its connector to make
  (connectors.md, section 4);
- **assignments:** a run is assigned only once its claim is durable, so
  an attempt never runs that a restart would not know of;
- **acknowledgements:** a worker's answer, and a run's turn, are
  acknowledged once committed; until then the worker keeps them
  (worker.md, 2);
- **answers to tool calls** that changed state, and to people's
  requests;
- **messages relayed to a live run,** so a run never reads what a
  restart would not have.

What decides nothing waits for nothing: facts streamed to watchers
(section 11), and reads served to a run's read tools.

### 5.3 Loads

- **What is loaded, not held:** ended tasks and their results;
  transcripts; history; notes' entries; anything the web pages through.
- **A load is a request** with a token and one terminal event, rows in
  bounded pages (programming-model.md, 4.4). What waits on it is in a
  state that says so: a brief gathering its sections, a run being
  prepared to resume, a person's page.
- **Bounded:** the loads in flight, and the bytes a load brings, are
  limits; a load past its bytes is cut, and says how much.
- **Pages:** rows are strictly increasing in the requested key range,
  after its exclusive cursor. When more rows remain, the continuation is
  the last returned key; an empty page never claims a continuation. A byte
  cut keeps a whole prefix and reports omitted rows and bytes. Its cursor
  is the last kept key, or the original cursor if no row fits, so a caller
  detects nonprogress and falls back rather than treating it as an empty
  end of range. Each page sees the commits answered before its own request;
  paging does not promise a snapshot across several requests.
  The store request also names the hard decoded reply-byte bound, enforced
  before its protocol hands ownership to the root. This receive bound may
  exceed a waiting brief's or resumed transcript's kept-prefix budget; the
  root counts both the incoming page and temporary prefix slots in its
  worst case. A reply past the hard bound is malformed, not retained.
- **Abandonment:** a waiter may cease waiting, but an issued load keeps
  its slot until its actual store terminal arrives. That terminal releases
  capacity without waking the abandoned waiter; a stale token cannot
  complete a later load in that slot.

### 5.4 What the store keeps

| Record | Written by | Loaded |
|---|---|---|
| the deployment: its id, made at its first start, and its numbers (tasks, runs, commits) | the root | at start |
| projects: repositories and roles, policy, people's roles | people | at start |
| people: identities, sign-ins | people | those with a live sign-in or a role, at start; others as they sign in |
| people's requests' keys, with their answers, for a retention | people | at start, those within it |
| tasks: every field of tasks.md, section 3, write holds and writer slots, the last turn committed for a live attempt | tasks | live ones at start; ended ones on demand |
| calls committed since each live task's last committed turn, by attempt and name, with their answers | the root | with their task |
| funders' numbers: each person's pool and each project's spend, per period | tasks | at start |
| a task's history: made, amended, held, released, cancelled, by whom, why | tasks | on demand |
| inboxes: messages not yet taken | tasks | with their task |
| proposals: pending, and decided ones' outcome | tasks | pending ones at start |
| transcripts: turns, with their spend | the root | to resume; on demand |
| procedures' states | each connector | with their task |
| the outbox: entries not yet made, and their keys | each connector | at start |
| what temper made on each system, by key | each connector | live ones at start; on demand |
| each connector's own state: for the forge, each repository's last listing time, the tips temper pushed or updated, read positions on participating objects, what each goal's issue last projected | each connector | at start |
| notes | notes | indexes per scope in use; entries on demand |
| traces | views | on demand, within their retention |
| secrets: LLM accounts' refresh tokens, sign-in sessions' tokens or their digests | the engine's protocol layer, never the domain, which names them by account and generation, or by a session's number | at start, by the protocol layer |

**Times** the store keeps are wall times (`env.wall`), so a record means
the same after a restart; a wait the engine arms is a deadline on its
monotonic clock (`env.now`), computed from them (programming-model.md,
section 9).

### 5.5 What the engine asks of the store

The store is assumed (core.md, section 2); this is what the domain
relies on, and all it relies on:

- a commit of several records is applied whole or not at all;
- commits are applied in the order sent, and durable once answered;
- a load sees every commit answered before it was sent;
- records are keyed by the engine's own keys, and listed by key ranges,
  in bounded pages; what is live (live tasks, pending proposals, the
  outbox, live resources) is kept in key ranges of its own, so a restart
  reads it without reading history;
- what one engine wrote, the next reads: records carry the version of
  their shape;
- nothing else writes it.

What it is, where it lives and how it is built are not the domain's
concern.

### 5.6 How a child's records reach a commit

Each child that keeps durable state defines its own `Stored` records and
ordered `Key`s in its boundary vocabulary. Neither a sibling nor the
store chooses their shapes. The root translates their keys and carries
their records in its store vocabulary; the store's protocol layer encodes
each shape with a version (5.5). The convention is shared by the children
and their worlds (`docs/plans/next-domain/README.md`, 5.2 and 5.3):

- **`Save { record: Stored }`** keeps the record under its own key, and
  **`Erase { key: Key }`** removes that key. These requests are part of
  the decision the parent is making. They have no terminal event and do
  not ask the child to wait: its state changes as it decides (5.1), so
  the next decision sees it.
- **`Restore { record: Stored }`** supplies one record at restart, and
  **`Restored`** says every record in the child's live ranges has been
  supplied. The child then decides from that restored state. Its records
  must carry everything needed to recover its decisions and pending work.
- **`Load { owner: Token, range: Range }`** asks for one bounded page of
  the child's own key range. Its one terminal event is
  **`Loaded { owner: Token, rows: Box<[Stored]>, more: bool }`**. The token
  is echoed, and `more` says another page remains: the parent supplies
  each page in response to a separate load, rather than sending further
  terminals for the first request. The child requests the next range when
  needed, within its limits. This is distinct from restart's `Restore`
  stream.

A child never waits for a commit and never holds an output back for one.
Assignments, answers and effects leave the child as it decides. Before
its entry point returns, the root gathers every child's `Save` and
`Erase` into the decision's one numbered `Commit`. It tags each outward
request following from that decision with that commit, or with the last
commit made if the decision made none. Requests wait until that number
is durable and are released in their original order through the root's
ready list, within `MAX_OUT`; the step hearing the store does not release
an unbounded group at once. The outputs that decide nothing remain as
5.2 specifies. A failed commit releases none of its waiting requests, nor
any later ones (5.1).

A child's world plays its parent and the store: it retains saved records,
applies erases, commits at the end of each step, and restarts by feeding
the durable records as `Restore` followed by `Restored`. It serves paged
loads in the child's own vocabulary. Restart is therefore tested before
the root is built, including decisions whose effects have yet to settle.

### 5.7 The first root's concrete handoffs (06a)

The walking root settles the following choices before extending the routes:

- **Cold header first.** A worker hello arriving before the deployment page
  ends is retained in bounded cold room, with its host/workstream arrays
  checked before retention. At most one hello per channel and at most fleet
  workers are retained; excess or duplicate hellos are refused immediately.
  Cold known losses coalesce by channel, and stale losses consume no room.
  Further hellos retry after startup so they cannot pressure an issued page.
  No writing decision can precede that page: a
  restored header must not replace numbers already allocated in this process.
  An empty store commits its new header; an existing header is durable already.
- **Bootstrap roles are records.** Configured owner projects missing from the
  people pages get an empty roles record in the startup decision. Signing in
  applies the configured owner identity through the people child. Existing
  durable roles are restored and retained. The root's closed translation is
  owner → authority role 0, maintainer → 1, member → 2, observer → 3; project
  policy supplies the authority and request permissions for those numbers.
- **Callbacks are held too.** Fleet `Start`, `TurnKept` and `Acknowledge` are
  internal effects following the same journal barrier as worker/web outputs.
  Releasing one moves it to bounded root work; journal pressure cannot discard
  it. All child saves and erases caused by its synchronous callbacks join the
  decision before that decision closes. Startup hands every restored claim to
  the fleet before `Loaded` and before making any new assignment. New decisions
  and timers reserve three delivery batches of journal room: the admitted
  batch, its internal callbacks' follow-up effects, and one decision's room
  to route those callbacks. A blocked callback is routed before consuming
  another held callback or admitting new work. A channel loss changes only
  fleet topology/deadlines immediately; it emits no saves or effects.
- **Run readiness retains its cause.** A credential-only wait keeps one bounded
  activation context in due room, reconsidered when the real account becomes
  usable. Deadline, budget and other static authority failures become task
  holds, so they cannot spin the ready pass or block unrelated placements.
  Fleet refusal or withdrawal clears the prepared assignment and routes an
  uncharged refused activation; a refused charged terminal becomes an
  uncharged invalid activation and its eventual durable fleet acknowledgement.
  Rejected cumulative spend or oversized result bytes are never precharged.
- **A chat's workstream is its task number,** encoded as eight big-endian
  bytes. It is nonempty and stable across attempts, and root-issued global
  task numbers prevent different chats from sharing a stream accidentally.
  Connector resource placement and writer slots join in 06e.
- **Funding is one child's.** The configured current period and a person's
  finite pool are admitted through tasks only after the real project/role
  policy allows their ceilings. `Make` reserves that pool atomically. A turn
  and terminal each use the hub's charged admission, not a charge followed by
  a separate event. The original finite ledger stays in tasks; the root restores its own
  bounded current-claim proof before fleet adoption and keeps no copy of balances.
- **The task section is concrete.** For this chat it starts with the person's
  spec, then its report contract's byte bound, then its requester. A task
  requester is the actual person for this route. Task/deployment requester
  brief depth joins only with its actual root caller; this slice refuses it.
  A cut accounts for omitted person bytes in the last kept part. The brief child owns rendering, required-section admission and cuts.
- **A person's result comes from its ended task.** The committed ending may
  produce a live result notice; losing that notice loses no inbox record.
  Reconnecting web clients request that named task's result using a valid
  restored sign-in. The root loads its ended row after prior commits, checks
  the requester, and consumes the read's one reply destination. Startup does
  not replay an unsolicited result notice, and no second persistent people
  inbox is introduced. The broader inbox/read-position routes join in 06d.
- **Idleness is a root fence.** After reclaim, `quiescent` requires finished
  startup, no unfinished page or commit, no held delivery or synchronous
  callback, no body/result waiter, no brief read and no fleet ready/call/turn
  obligation. Account refresh/keep/cancellation terminals also remain owed.
  Live idle workers and sessions, assigned workers awaiting external answers,
  and future task/account timers do not prevent shell idleness. A walking
  story also requires its independent referee's final result condition. The
  shell must additionally own and drain its pending external
  input/output queues; the root predicate cannot inspect those queues.
- **Deep bytes are checked before retention.** Wrapped child rows count
  their boxed records, slice slots and nested bytes. The whole decoded page
  is checked before restoring any prefix. The journal separately admits
  bounded assignments and the closed internal callback set; an arbitrary
  fleet hello cannot be smuggled through a held callback. Its memory bound
  includes simultaneous decision/scratch copies and held section slots. Root
  configuration also prices all cold fleet outputs plus the next load in one
  decision, and requires each decoded page to fit the synchronous route room.

The walking vocabulary remains limited to sign-in, keyed chats, worker
hello/loss, turns, terminals, named historical results and account refresh
terminals. Later tools and connector routes are separate increments; the
root's store wrappers do not expose a blanket child-event pass-through.

## 6. Restart

A restart is a cold start that rebuilds everything a decision reads from
the store and the connectors, and makes nothing twice:

1. **Load what is live:** the deployment's numbers; projects, their
   policies and people's roles; live tasks with their inboxes,
   subscriptions, holds and procedures' states; pending proposals; the
   outbox's entries; the calls committed since each live task's last
   turn; the funders' numbers; LLM accounts; each connector's own
   state. The engine's limits bound what
   is live (tasks.md, section 10), so what is loaded fits; a live set
   past limits lowered since refuses the start, saying which.
2. **Adopt the runs.** Every task claimed or running names its attempt.
   Workers dial in and say what they host; the fleet adopts each
   attempt the store names, keeps waiting for one within its grace, and
   cancels any it does not name (section 8). Nothing new is assigned
   before every claim loaded has reached the fleet. A claim no worker
   reports within the grace counts as not started (tasks.md, 5.2).
3. **Read afresh.** Each connector reads, for the live tasks that name
   them, the resources its system owns (connectors.md, section 13).
4. **Settle the outbox.** Every entry not committed as made may have
   been made. Each is looked for by its key before it is made again; one
   that cannot yet be found waits until a write's lifetime has passed
   since the engine started (connectors.md, 4.3). Entries never sent
   are among them: the engine cannot tell them apart, and looking costs
   one read.
5. **Decide afresh.** Every procedure steps once on the facts read;
   every agent task due gets a run; every inbox's messages are relayed
   or wake as their policy says.

Workers keep the answers and turns they were not acknowledged for, and
send them again after their hello; the engine commits any it does not
have, and acknowledges each.

## 7. Agent executors

### 7.1 From a task to a run

When an agent task is due (tasks.md, 5.2), the root:

1. **Checks** that the run may start (authority.md, 8.3).
2. **Prepares it,** the task preparing (tasks.md, 5.2) over as many
   steps as it takes: gathers its brief (section 9), for why it runs,
   its first activation or the messages that woke it; and loads its
   transcript, if its charter resumes and it has one within the resume
   limit (7.2). Either way it is given the calls committed after its
   task's last committed turn: a resumed run as a note after the
   transcript, a fresh one as a section of its brief.
3. **Commits the claim:** the task claimed, with its new attempt number
   and the writer slots of the write holds its workspace writes
   (connectors.md, 3.3).
4. **Places it** through the fleet, once the claim is durable, with an
   assignment:
   - its run and attempt, the engine's names for them;
   - its workspace: the repositories its task reads and writes, from the
     resources its spec names and its project's (connectors.md,
     section 9): each with where to start, whether it may be written and
     the branch a change is pushed to; and a workstream key, the number
     of the task that holds the resources written, so that a change's
     producing, repairs and conflicts find its checkout cached;
   - its charter, for the agent (agent.md, section 4), which the
     engine's protocol layer encodes as smith's: the task's instructions,
     the brief, the tools its authority gives, the outcome its result
     contract asks for, its budget for the run and its models with their
     prices (authority.md, 7);
   - its transcript, when it resumes, and the calls committed since its
     last turn;
   - the credential grants its models' accounts and its repositories'
     identities need (section 12).

### 7.2 Transcripts

- **A transcript belongs to its task,** whichever runs added to it.
- **Committed turn by turn.** A run sends each turn up as it ends: what
  the LLM said and the calls it made, their results, what it spent, and
  the last inbox message it read. The engine commits the turn with its
  spend and with the messages up to that one taken, then acknowledges
  it. A worker keeps each turn until it is acknowledged, and a run with
  too many unacknowledged turns waits (worker.md, section 8).
  Admission is whole: the task hub validates the attempt, next turn and admitted
  read-through before changing its cumulative spend or funding ledger.
  A refused turn changes neither; fleet fences accepted duplicate turns
  before child admission, so they change neither again. Authority supplies arithmetic over snapshots; the hub owns and
  saves the actual finite funding links and numbers. The root gathers those
  saves with the opaque transcript into the same commit. It never precharges
  an event in a separate hub operation or maintains a duplicate pool ledger.
- **The engine never looks inside.** A turn is bytes of the agent's own
  conversation vocabulary, bounded, which the web's protocol layer
  renders. The domain reads only its size, its spend and what it took.
- **Resuming.** A run that resumes is given the transcript whole, within
  the resume limit, and, as a note after it, the calls committed after
  its last turn, with their answers, since the turn that made them was
  lost: so it does not ask for them again. One past the limit
  starts fresh from a brief that carries the transcript's tail. A
  transcript the agent cannot use (an older shape, another provider)
  fails its run as transient, saying so, and the next run is prepared
  fresh, its brief carrying the tail.
- **Turns are numbered within their attempt,** from one. The task keeps
  the last turn committed for its live attempt, restored atomically with
  the fleet's adoption of the claim, so a copy the engine has
  already committed is acknowledged again and dropped, after a restart
  too.

### 7.3 The engine's tools

A run reaches the engine through smith's host tools, declared in its
charter, which its worker relays (worker.md, 4.2); their schemas,
decoding and rendering are the engine's protocol layer's (agent.md,
4.3). Each is checked against its task's authority
(authority.md, section 8), decided, committed, and answered after the
commit:

| Tool | What it does | Answers |
|---|---|---|
| `delegate` | makes a batch of tasks as its delegates (tasks.md, section 4) | their numbers; refused, and why; beyond authority, and what |
| `message` | words, a question or an answer to a task it references | sent; no reference; the inbox full |
| `amend`, `cancel`, `release` | its delegates (tasks.md, section 6) | done; refused, and why |
| `decide` | accepts, rejects or passes up a proposal or an escalation waiting for it (tasks.md, section 8) | done; beyond its authority, and passed up |
| `propose` | any of these, or an effect, or more authority for itself, with a reason | proposed, and to whom |
| `subscribe`, `unsubscribe` | a connector's topic, a referenced task's state, a timer | done; refused |
| `effect` | a connector's effect (connectors.md, section 4) | made, with what it made; committed and pending, past the call's deadline; failed |
| `note`, `recall` | writes a note in a scope; reads entries by name or by a search (section 10) | written; the entries |
| a connector's reads | reads its system (connectors.md, section 8) | the answer, within the read's budget |

**Calls are named, and decided once.** Every call names its task's
attempt and the run's own name for it. The engine commits a call that
changes state together with its answer, and answers a call it has seen
before from that record, never deciding it twice. It keeps a task's
records, by attempt and name, until a turn of the task that carries
their answers commits, or the task ends; so calls an attempt made in a
turn it lost are still there for the next attempt to be told of (7.1).
A run whose call was withdrawn, lost with its channel, or answered busy
asks it again with the same name while it lives, after a backoff
(smith's `run.md`, 5.2), so the LLM is told the outcome rather than
trying anew;
an effect's key is derived from the call that asked for it
(connectors.md, 4.4).

Three tools a run has are not the engine's: `wait`, which the run and
its worker serve, holding the slot until a message arrives (7.4);
`finish`, which the run serves, checking its result against the
contract (smith's `run.md`, section 7), and whose result reaches the
engine as the run's answer (tasks.md, 5.6); and `deliver`, which its
worker serves as a push (agent.md, section 6).

Beyond authority, a call is refused naming what it lacked; the run may
`propose` the same action with a reason. Calls never become proposals on
their own, so a person is never asked by accident.

### 7.4 Answers, waiting and parking

- **A run answers once:** finished, with its result; parked; or failed,
  with a typed failure (worker.md, 4.3). The engine commits it, once
  it has every turn the answer counts, and acknowledges it; a finished
  task closes, a parked one is idle, a failed one retries or is held
  (tasks.md, 5.5).
- **Waiting holds the slot; parking frees it.** A run that calls `wait`
  keeps its worker's slot while it waits, so a chat answers at once.
  Idle past its charter's threshold, it parks: every turn it took is
  committed first, so nothing of it is lost, and it ends. A coordinator's
  threshold is zero: it parks as soon as it waits.
- **Woken again,** a parked task is due. Its charter says whether the
  next run resumes the transcript or starts fresh from a brief; by
  default a chat resumes and a coordinator starts fresh, its brief
  carrying its goal, its plan's state, its inbox and its notes.
- **Messages to a live run** are relayed as they arrive, named, and a
  turn that reads one takes it (7.2). What the run did not read before
  it ended stays in the inbox.
- **A cancel** ends the run: the worker cancels it and answers; what it
  committed before stays committed.
- **Saved work.** A run that parks, fails or is cancelled with
  unfinished changes has them saved by its worker to its saved-work
  branch, and the next run of its task starts from it (worker.md, 4.2).

### 7.5 Current tasks boundary and root-owned replay evidence

The concrete per-variant audit is tasks.md, section 14. Root owns transport
proofs, transcripts, worker terminal evidence and child correlation; tasks
owns semantic lifecycle/dependency state and authentic task/pool/period
financial numbers. Removing raw task peeks does not introduce a second
ledger. Actual Activate supplies bounded temporary RunContext for authority
and the person-chat brief; Adopt carries its actual committed kept turn;
owned worker payloads carry task and attempt for refusal correlation.

Before Claim mutates the child, root reserves one RunProof map slot, bounded
by live task capacity. The accepted claim and initial proof share one commit.
Each accepted next Turn commits its latest metadata proof, immutable TurnRecord
and all child financial/task writes together. Fleet's kept-turn fence handles
older or pending duplicate bodies before they can produce Turned; therefore
one latest turn's metadata is enough. Transcript bytes remain only in their
immutable archive, with no duplicate replay window or reduced worker bound.

A priced Answer reserves its current proof and bounded typed terminal room
before child mutation. On acceptance, root Terminal archive plus current
proof and all child lifecycle/financial writes share one decision. Rejected
answers normalize to an unpriced Invalid terminal without applying their
expense. Topology refusal/loss is unpriced; its proof records the actual root-translated
terminal at the latest accepted expense, with no invented worker answer. Ending erases RunProof in that same decision; coalescing means the
final transaction contains the terminal archive and proof Erase. The held
Fleet Acknowledge owns the completion handoff after this barrier, including
fenced duplicate worker inputs. Root never charges an event separately.

Startup pages Live/Ledger and then the RunProofs family. It validates every
proof against transient metadata from its owned live-task page: current
attempt, kept turn, run expense and last answered attempt. Latest turn expense
cannot exceed run expense; a stored terminal must match current answered
attempt and final run expense. Proof terminal presence must match the child's current answered attempt;
unpriced topology records its actual root-translated terminal at unchanged
accepted expense.
The root also refuses restored task shapes with no actual root route: non-person
requesters, a different charter, non-Report contracts or delegate/dependency/input
work. Tasks Restored is deferred until all proof rows pass, so invalid proof
cannot commit a restored closing consequence or release a result/cancel.
Missing, unrelated, oversized, malformed or excess proof rows stop startup;
none is silently dropped. Only after all proofs pass does root hand every
restored claim to fleet, followed by Loaded and new assignments.

A new claim replaces its proof, an ended task erases it, and historical turn
or terminal archives never load into the bounded live map. Worst-case memory
prices proof slots/owned terminal bytes, transient startup correlation,
activation contexts, incoming payloads and simultaneous child/decision
copies. This preserves the independently tested first-turn durable/lost-store-
completion restart. Terminal-commit-before-ACK/result restart remains later
coverage; its durable typed archive is preserved without claiming that cut.

## 8. The fleet

The fleet child domain knows the workers and the runs they host. It
keeps what it does today, with a run named by its task and attempt
instead of an item's:

- **Workers** dial in with their slots and the workstreams their
  checkouts hold; a run goes to a worker with a free slot, preferring
  one that holds its workstream. A refusal as busy is no failure.
- **Attempts are fenced.** Everything a worker sends is dropped unless
  its attempt is its task's live one, but a cancelled attempt's answer.
  No attempt of a task is placed while the fleet knows a worker may
  still host another.
- **Answers and turns are acknowledged once committed** (5.2), or once
  fenced off; a copy before then is dropped unacknowledged, one for an
  attempt the engine has finished with is acknowledged again.
- **Losing contact.** A lost worker's runs are kept for a grace; one
  that reconnects says what it hosts, and the engine keeps or cancels
  each. Past the grace they are lost, and their tasks retry.
- **Relaying:** messages and cancels down, tool calls up and their
  answers back, each call answered once; a call the fleet cannot pass
  up is answered at once, unserved, by the root, never left without an
  answer.
- **The graces are ordered.** The engine's grace is longer than the
  longest a worker may take to stop a run it lost contact for: its
  grace, then the longer of its cancel's grace and a push's deadline,
  then a save's commit and push (worker.md, 4.2). The worker says that
  sum at its hello; a worker whose sum is equal to or longer than the
  engine's grace is refused. So the engine places no next attempt while a
  worker may still host the last, and one run at most writes a write hold at a time
  (connectors.md, 3.3). A lost attempt's push is found by its change
  afterwards (forge.md, 8.2).
- **Every start and adoption ends once:** answered, lost, withdrawn (the
  engine cancelled it), or refused (busy, a duplicate, or invalid).
- **Restarting** (section 6): the fleet adopts the claims the store
  names, then hears that the loading is done; a run a worker lists that
  no claim adopts is cancelled once a grace has passed.

## 9. Briefs

The brief child domain builds every run's brief from typed sections,
each within a budget, the brief within its own, and the agent's protocol
layer renders them as text, as today: sections
follow why the run runs, those it runs for are required, the rest
optional; what does not fit is cut in its kind's order and says how
much; sections are gathered within one deadline, while the task is
preparing, and a required one missing sends the task back to due after
a backoff, no attempt spent (tasks.md, 5.2).

Its sections come from two places:

- **The core:** the task (its spec and contract, and its lineage: its
  requesters' specs, cut); its inbox since its last run, people's words
  whole; its dependencies' results; its delegates and their states, and
  the results that came since (the plan, for a coordinator); its earlier
  attempts and why they failed; the calls its task committed since its
  last turn, when it does not resume; the proposals and escalations
  waiting for its decision; the index of its notes; its charter's
  template, if it follows one; a transcript's tail, when it does not
  resume.
- **Its connectors,** for the resources it names (connectors.md,
  section 8): for the forge, the pull request against its base; CI's
  failures on its head, with their output; reviews' remarks; a conflict,
  with what landed in the base since (forge.md, section 8).

A section is loaded or read as it is cut, asking for no more than it
keeps.

## 10. Notes

Notes are what temper learns and passes on: a convention a reviewer
keeps asking for, why an approach was dropped, which test is flaky, what
a person prefers. The notes child domain knows scopes, entries and
indexes:

- **An entry** has a one-line description, a body, a scope (the
  deployment, a project, a repository, a goal), references to tasks, and
  who wrote it: a person, or a task.
- **In the store,** where people read, correct and delete them in the
  web. Notes are hints: where a note and the facts disagree, the facts
  are right.
- **Written by a tool,** checked against the task's note scopes
  (authority.md, section 3), and committed like any decision. A revision
  names the revision its run recalled, and is refused as moved if the
  entry changed since: the notes child domain checks it against the
  revision its index holds, which is exact, since the engine is the
  store's only writer and this child domain its only writer of notes.
- **Read in two steps.** A brief carries the index of the notes in its
  scope, a line per entry, saying how many more there are when they do
  not all fit; a run asks for the entries it wants with `recall`, by name
  or by a search of their descriptions, answered a page at a time.
  Indexes are kept per scope in use, and evicted least recently used;
  entries per scope are bounded.
- **Knowledge of the code itself** belongs in the repository, as
  `AGENTS.md` does. A note that matures may be proposed as a change to
  it, which is a change task like any other.

## 11. Views and the web's reads

The views child domain streams what runs report, and what tasks do, to
the people watching, as today:

- **Watches:** a run (its streaming text, its tool calls, its turns as
  they commit), a task's tree (phases as they change), a project's goals.
  A watch starts from a snapshot the engine gives, delivered first.
- **A slow watcher never holds the engine up:** one delivery in flight,
  a bounded backlog, what overflows dropped and the watcher told.
- **Traces** are batched into the store, kept for a retention period,
  with content as the run's capture policy says; expendable.
- **History is the store's.** What the web shows of the past (a task's
  results, its transcript, a plan's revisions and why) is loaded
  (5.3), paged, never held.

Nothing the engine decides depends on a fact or a view arriving.

## 12. Accounts

The accounts child domain is as `credentials.md`, section 5 designs it:
LLM accounts refreshed by the engine alone, their rotated refresh tokens
made durable before the access tokens they bought are granted, grants
pushed to every live attempt that uses an account, and a run whose
account is unusable not started. Only where the protocol layer keeps a
refresh token changes: in the store's secret records, which the domain
never reads, instead of a file of its own.

## 13. Limits and working sets

What the engine holds, each bounded by its limits and summed into its
worst case (programming-model.md, 6.3):

- live tasks, with their inboxes, subscriptions, references and holds;
  pending proposals (tasks.md, section 10);
- the rules and the live projects' policies, within limits on projects,
  repositories per project and people holding roles;
- people signed in, and their pending requests;
- the calls committed since each live task's last turn, and the
  funders' numbers;
- workers, their runs, and the calls and messages relayed;
- LLM accounts;
- each connector's working set, outbox and request budget
  (connectors.md);
- commits in flight and the outputs waiting for them; loads in flight
  and their rows;
- briefs gathering, and the notes' indexes in use;
- watches and their backlogs.

None of it grows with the store's history. A new task, a person, a run
or a watch past its limit is refused at its entrance, and the asker
told why.

## 14. Below the domain

What the protocol and io layers owe the domain. The protocol layer is
designed in `docs/design/protocol.md`, with the channel in `channel.md`,
credentials in `credentials.md` and the forge in `docs/design/forge.md`;
README.md, section 5 lists what this design changes in each.

- **The store:** commits of typed records, numbered, answered once
  durable, and loads by key range in pages, over temper's sized binary
  records with a version per shape. A fake store in the worlds, slow and
  failing.
- **Workers:** the channel of worker.md, with what this design adds:
  turns up and their acknowledgements, spend in each turn, the engine's
  tools as relayed calls, transcripts in assignments, a merge in
  progress as a place to start. Snapshots go.
- **Connectors:** each its own protocol; for the forge, its calls and
  webhooks (forge.md, section 17).
- **People:** the web: HTTP, live streams, signing in through the forge
  (people.md, section 11).
- **Configuration:** the deployment's rules, connectors and their
  systems' addresses and credentials (one API token per forge), LLM
  endpoints, accounts and models' prices, workers' secrets, the charters
  the deployment offers (instructions, models, tools, result contracts,
  wake policies, waiting times) and procedures' settings, limits.
  Projects and their policies are the store's, seeded from
  configuration on a first start.

## 15. The world

Each child domain has a world of its own under `tests/engine/<name>`,
its parent and neighbours scripted in it, its scenario's expectations in
a referee; authority, which keeps no state, is tested by its step tests.

The engine's world, `tests/engine/domain`, runs the root with every
child domain beneath it against fakes that share none of its types
(testing-strategy.md, section 4): the fake forge, its faults drawn from
the seed; one to three scripted workers, which play each run's script,
keep answers and turns until acknowledged, lose their channels and come
back, or vanish; scripted people on the web; a fake store, slow, or
failing a commit, which stops the engine; and the engine restarting
cold, at drawn moments or at one a scenario chooses, from the store's
last durable commit.

Its stories, as people meet them: a chat that answers, parks and
resumes; a small fix made in a chat and landed; a goal proposed,
accepted, planned with spikes, a person's choice and changes, revised
mid-way, and done; a change failing CI, repaired, reviewed at its head
and landed; two goals landing into one branch, one updated cleanly, one
in conflict and resolved (forge.md, section 15); a delegate held past
its tries, escalated to a person and released; a goal cancelled with
runs live and a merge in flight, closing deepest first; a tool call
whose answer is lost with the channel and the engine, asked again after
a restart and answered from its record, made once; a chat's push handed
to a change while its run still holds the writer slot; a note written,
corrected by a person and recalled; a recurring task across a period's
reset; a person's words to a running run; a pull request closed by
someone else, held, and explained.

Its referee holds the engine to core.md, section 10, seen from outside:
authority holds; keyed effects are made once, across a restart at any
drawn moment; nothing a worker, a person or the forge saw depended on a
commit that was lost; dependencies come first; one run per task, one
writer per resource; merges land exactly the heads decided; results and
people's words arrive; nothing is written over; every story ends within
a bound; every call is decided once; budgets are never counted twice.
The engine, workers and agents meet in the system worlds
(`docs/design/testing.md`, 2.1), which reuse this world's people, store
and referee.

## 16. From today

- **The top level** keeps its routing, its translations, the brief's
  gathering, the runs it starts and the calls it serves, renamed from
  items to tasks. It loses most of its state: the item table, records
  composed into comments and split from them, their nonces, side writes
  and their retries, and the restart contract built on them become tasks
  in the store, commits and section 6. People's requests (`people.rs`
  today) move to the people child domain.
- **`work`** becomes `tasks`, and **`plan`** splits between `tasks` and
  the forge's change procedure (tasks.md, section 12). **`rules`**
  becomes `authority` (authority.md, section 12).
- **`forge`** becomes the forge connector's subtree, losing records,
  the wiki and labels as the index of live work (forge.md, section 19).
- **`fleet`, `accounts`, `brief`, `notes` and `views`** stay, with
  items renamed to tasks, snapshots replaced by transcripts, notes moved
  from the wiki to the store, and briefs' sections given by the core and
  by connectors.
- **The engine's tools.** Today it serves five calls: forge reads,
  `recall`, `note`, a comment and an escalation. The forge's reads become
  a connector's reads, the comment an `effect`, the escalation a
  `propose`; `delegate`, `message`, `amend`, `cancel`, `release`,
  `decide`, `subscribe` and `unsubscribe` are new.
- **Configuration splits.** Today's (`config.rs`) holds the
  repositories and the wiki's home, the tracking, hand-in and projected
  labels, the rules, the session's step, templates, models, the trace
  policy, and the branch and saved-work prefixes. Repositories, rules
  and roles seed projects and policies in the store; labels go; the
  session's step and templates become charters; the prefixes give way to
  forge.md, section 11's names.
- **Two boundaries are new below the domain.** No store and no web exist
  today: the store's requests (snapshots, traces) are served only by the
  engine world's fake, and people's requests come only from the worlds.
  The protocol layer's refresh-token record, a file today (temper-oauth's,
  written by the engine's protocol layer), moves into the store with
  them.
- **Runs:** starting from saved work, which the worker supports and the
  engine never asks for, and deleting saved-work branches, are new; so
  are transcripts in place of snapshots, which nothing produces today.
- **Tests:** `tests/engine/work` and `tests/engine/plan` give way to
  worlds for `tasks`, `people` and the forge's subtree; the engine
  world's codecs for records and wiki pages go; its fake store gains
  ordered commits, paged loads and commits that fail, and its people act
  only on the web.
- **New:** `people`, as a child domain of its own (people.md,
  section 13); commits and loads; transcripts turn by turn.

## 17. Open questions

- **Commits in flight:** how many, and whether the store should take
  several decisions as one commit when they wait together.
- **Briefs:** whether a brief's store loads and connector reads share
  one budget; cancelling a brief while it gathers; how a missing section
  shows in what the agent reads.
- **Transcripts' limits:** the resume limit, whether runs compact a
  transcript (smith's `session.md`, section 8) before it is reached, and how long
  the store keeps them.
- **Views:** a limit on watchers per subject; how a watch of a task's
  tree is bounded when the tree is large.
- **Scaling out:** one engine per deployment until it binds; the store
  being the engine's alone is what would change first.
