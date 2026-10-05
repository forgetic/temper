# The agent

Provisional, 2026-10-04. What the temper agent does, as a domain layer:
its parts, what each is responsible for, and how they fit together. An
agent's run is one activation of a task whose executor is an agent
(core.md, 3.2); the engine prepares it (engine.md, section 7) and a
worker hosts it (worker.md). The mechanics are those of skein's
`docs/foundation/programming-model.md`. What is still open is listed in
section 10.

## 1. In one page

- **An agent is a flexible LLM driver.** The engine decides what work
  exists and holds authority, the worker prepares a workspace and hosts
  the run, and the agent does the LLM work: a coding job, a review, a
  chat with a person, a coordinator revising a plan. A code change is one
  kind of result among several.
- **A run is one activation of a task.** It starts from its task's
  charter, a brief the engine built for why it runs, and the task's
  transcript when it resumes. Continuity is the task's, in the engine's
  store, turn by turn; between runs nothing of the agent is live.
- **The engine's tools make it an actor.** Besides its own tools on the
  workspace, a run may delegate tasks, message, steer its delegates,
  decide what they propose, propose, subscribe, make effects, note and
  recall, wait and finish, as its task's authority allows (engine.md,
  7.3). It reaches all of them through its worker.
- **Three child domains under one root domain.** `run` (one activation:
  its charter, its sessions, its result), `session` (one conversation
  with an LLM) and `tools` (what a session does to the workspace),
  composed by `temper-agent-domain`, the agent loop's entry point.
- **The domain is complete** (programming-model.md, section 4). A world of
  domains and fakes runs everything the agent does, with no protocol and
  no io. The protocol layer translates bytes to domain entities and back,
  including the JSON an LLM writes as a tool's input, and decides nothing.
- **Policy is data.** The agent interprets no workflow vocabulary: no
  role, task kind or verdict names. What differs between agents arrives
  in the charter: the brief, the tools, the repositories it may write, the
  result it must produce, its budget.
- **Any model, any provider.** The charter names the model a run starts
  with, and the others its sub-agents may use, on any endpoint the agent
  is configured with. The conversation vocabulary is provider-neutral;
  each provider's API is the protocol layer's business.
- **Failures are visible.** The LLM sees what went wrong, bounded in size:
  a failing test's output, an edit that matched nothing, a result that
  broke its contract, a tool call beyond its authority. A failure is never
  replaced by a fixed message.

## 2. The agent in the system

```
engine   tasks, authority, connectors, the store; prepares each run and serves its tools
worker   workspaces: starts runs, relays their tool calls and turns, pushes their changes
agent    LLM work: runs sessions with tools, reports turns, facts and its result to the worker
```

- **The engine is the only client** of the store and of every
  connector's system. It answers the reads and engine tools a run calls,
  and commits the run's turns and result. The worker's one write is
  pushing a run's branch, over git.
- **The engine never reads a repository.** It works from what its
  connectors read; what a workspace holds, such as a repository's
  `AGENTS.md` or its checks, is for the run to find.
- **The worker starts a run** with a charter (4.1). Credentials never
  reach the domain: it names an LLM endpoint, and the protocol layer holds
  the rest.
- **The agent acts on the world only through the worker:** pushing a
  change, which the worker does, and the tools its charter offers, which
  the worker relays to the engine. It never pushes, merges or writes to a
  system on its own authority.
- **Retrying a failed run is decided above the run,** across attempts. A
  run answers once and is done.

## 3. Structure

```
temper-agent-domain                 the agent loop's entry point: one step, one fire
├── temper-agent-domain-run         one activation: charter, sessions, result, the engine's tools
└── temper-agent-domain-session     one conversation with an LLM
    └── temper-agent-domain-tools   read, list, search, write, edit, shell
```

The tree follows programming-model.md, 4.5: each child domain is a step
machine of its own with its own world; a parent owns its children's state
and routes between them; siblings share no domain types, so the run and
the session meet only through translations in `temper-agent-domain`. Its
vocabulary is what crosses the domain boundary: the worker's messages,
the run's turns, answers and facts, calls to LLM providers, and the file
and process operations the tools ask of io.

## 4. The run

A run is one activation of an agent's task. It takes its charter from
the worker, opens the sessions that do the work, serves the tools that
act beyond the workspace, sends each turn up, judges what comes out, and
answers once.

### 4.1 Charter

What the worker gives a run when it starts it, from the engine's
assignment:

- **Instructions.** The role the task's charter gives it (a chat, a
  coordinator, a reviewer with a lens, a producer of a change), as text.
- **Brief.** Typed sections the engine builds for why the run runs
  (engine.md, section 9), which the agent's protocol layer renders as text
  for the LLM: the task, its inbox, its dependencies' results, its
  delegates' states, its notes' index, and what its connectors say of its
  resources. The agent adds what it finds in the
  workspace (the repository's `AGENTS.md`) and the sections about its own
  mechanics (its tools, its workspace, how to finish), because those are
  what it enforces.
- **Workspace.** The repositories, where they sit, and which may be
  written: the effective write authority, decided by the engine and
  enforced by the tools, not stated in prose. Whether a repository starts
  from a merge in progress, and which files the merge left in conflict.
- **Tools.** The families the LLM may call: inspect, modify, shell,
  sub-agents, each connector's reads, and each of the engine's tools
  (4.3). Data, derived by the engine from its task's authority, never from
  a role's name.
- **Result contract.** What counts as done (tasks.md, section 3): a change
  (the diff, with a title and body for its pull request), a verdict from a
  closed list with a contract per verdict (which fields are required, how
  many follow-up tasks of which kinds it may propose), a report, or one of
  these or a failure. For a change, it also says whether the repository's
  checks must pass (4.4).
- **Budget.** What the whole run may spend, across all its sessions, in
  the deployment's unit, from what its task has left (authority.md,
  section 7), with each model's prices; and turns and wall time. The run
  prices each completion it makes as it makes it. It is checked at the
  entrance against the agent's `Limits`; a run that asks for more is
  refused.
- **LLMs.** The endpoint and model the main session runs on, and the
  others a run may use for sub-agents (section 5). The agent is configured
  with endpoints (a provider and its credentials, held by the protocol
  layer); a charter only names them.
- **Waiting.** How long the run may wait for a message holding its slot
  before it parks (4.5).
- **Transcript,** when it resumes: the task's earlier turns.

### 4.2 What a run does

1. **Admits** a request, or refuses it at the entrance: busy, or beyond
   the limits.
2. **Equips** its main session: prompt, tools, workspace authority, LLM, a
   share of the budget, and the transcript it resumes, if any.
3. **Drives** the session. A session *yields* when the LLM stops calling
   tools, and the run decides what happens next: nudge the LLM ("you have
   not finished"), pass on a message that arrived, wait for one, or close
   it.
4. **Serves delegated tools:** finishing, sub-agents, and the engine's and
   connectors' tools its charter offers, which it relays through the
   worker (4.3). It runs each call's deadline: a call that times out
   returns only once what it started has settled (a child ended, a landing
   aborted), so nothing it started is still writing.
5. **Sends each turn up** as it ends (section 5): what the LLM said and
   called, the results, what it used, and the messages it read.
6. **Judges** a declared result against the contract. A violation goes
   back to the LLM as a tool error it can fix; it does not end the run.
7. **Accounts** every session it opens against one budget, pricing each
   completion, its sub-agents' included, and winds down when the budget
   runs out.
8. **Ends once.** It closes its sessions, waits for what is in flight to
   settle, and answers the worker: a result accepted, parked, or a typed
   failure (domain, budget, policy, cancelled, stale, or a transcript it
   could not resume) the engine can act on, with what it spent. A cancel from the worker takes the same path.
9. **Reports** facts as it goes (section 7).

Only a run opens sessions, so ownership is a tree: a run owns its
sessions, and which session is whose sub-agent is the run's bookkeeping.

### 4.3 Messages and the engine's tools

A run is shaped around messages in and actions out:

- **Messages in:** what its task's inbox lets through while it is live
  (tasks.md, 7.2): a person's words, a delegate's result, a question, an
  answer, a decision, an amendment, a cancel, news, each carried whole
  and named; and, relayed the same way, the proposals and escalations
  waiting for its decision (tasks.md, section 8). With each turn, and
  when it waits, the run names the last message it has read.
- **Actions out:** the engine's tools (engine.md, 7.3), each a delegated
  tool the run relays to the engine through the worker and whose answer
  it gives the LLM as the tool's result: `delegate`, `message`, `amend`,
  `cancel`, `release`, `decide`, `propose`, `subscribe`, `unsubscribe`,
  `effect`, `note`, `recall`, and the connectors' reads. Two more it
  serves itself: `wait` (4.5), which its worker holds the slot for, and
  `finish` (4.4). An action beyond the task's authority is answered as
  such, saying what it lacked, and the LLM may `propose` it instead.
- **Asked again, never anew.** Each call carries the run's name for it.
  A call to the engine left unanswered (withdrawn past its deadline, or
  lost with a channel) or answered busy is asked again with the same
  name while the run lives, after a backoff, once the earlier relay of it
  has ended, and the engine answers it from its record (engine.md,
  7.3).
  Only when the run cannot learn the outcome is the LLM told that it is
  unknown, and then it may look (its delegates, its effects) before
  acting again.

| Agent | Messages in | Tools it is given | Ends when |
|---|---|---|---|
| producing a change | none | the workspace's; `finish` with a change | its change passes its checks and is pushed |
| a review, a triage | none | reads; `finish` with a verdict | its verdict meets its contract |
| resolving a conflict | none | the workspace's, from a merge in progress; `finish` | the merge is resolved, checked and pushed |
| a chat | its person's words; its delegates' results | most; `wait` | its person closes it |
| a coordinator | questions, results, overlapping landings; proposals and escalations to decide | `delegate`, `amend`, `cancel`, `decide`, `propose`, `note`; `wait` | its goal's report is written |

### 4.4 Finishing

`finish` is a tool, not a convention about the last message. Its input is
the result, typed by the protocol layer; the run checks it against the
contract and answers with acceptance or with what is wrong. An LLM that
stops without finishing is nudged by the run, within its budget.

For a change, finishing also runs the repository's checks, when the
contract asks for them, and then asks the worker to push:

- **The repository says what its checks are,** by convention: an
  executable at a known path, such as `.temper/pre-pr`, looked up when the
  run starts. A repository without one has no checks, and nothing above
  the agent needs to know either way.
- **Checked is pushed.** The checks run as contained processes with
  deadlines, while nothing else writes to the workspace, and the worker
  then commits and pushes exactly that tree. Nothing else writes by
  construction: a write runs alone in its session, and a sub-agent lives
  only as long as the call that asked for it. A push that lands is the
  result, even if the run was winding down meanwhile.
- **A merge in progress** is finished the same way: the worker commits
  the tree as the merge, with both parents, and refuses it while a file
  the merge left in conflict still holds a marker (worker.md, section 5),
  which the run tells the LLM, naming the file.
- **Failures are feedback,** like any other: a failing check comes back
  with its output, and so does a push that fails. So does one that finds
  nothing to push, which is how a declared change is checked to exist:
  the worker says so, and the run tells the LLM its push failed. A push
  that finds the branch moved is the exception: it ends the run as stale,
  since every later push of the run would find the same, and the engine
  decides afresh.
- **Not a security boundary.** An agent can change what a check runs; CI
  on the forge and authority's requirements guard landing. The checks
  catch failures early, inside the run that can fix them.

Push failure feedback keeps a typed reason and the failed repository's
index, with the last **512 bytes** of the git invocation's diagnostic
output and the number of preceding bytes dropped. This is a fixed
protocol cap, sealed by its value constructor and included in the
domains' fixed-size memory bounds; it is separate from an agent
process's operator-only stderr. If several repositories fail, feedback
names the first in workspace order. The worker channel further trims the
tail to its configured answer-byte limit, counting those dropped bytes
too. Branch movement takes precedence. An ambiguous push is still
verified before feedback is returned; unsuccessful verification keeps
the original push diagnostic. No changed tree is a distinct `Nothing`
result, reported to the LLM as failed finish feedback with that reason.

Proposing a change mid-run, as a chat does before handing it to a change
task, goes the same way: checked, then pushed.

### 4.5 Waiting, parking, resuming

- **Waiting.** A run whose work depends on what comes next (a chat on its
  person, a coordinator on its delegates) calls `wait`. It yields until a
  message arrives, holding its slot, the watchdog paused.
- **Parking.** Past its charter's waiting time it parks: every turn it
  took is sent, and it answers parked. Its state is its transcript, which
  the engine already has, so it hands over nothing else. A coordinator's
  waiting time is zero.
- **Resuming.** A task woken again gets a new run, which resumes its
  transcript when its charter says so: the session opens with the turns as
  they were, provider blocks included, adds the calls the engine committed
  after the last of them with their answers, and goes on with the
  messages that woke it. A run that does not resume starts fresh from its brief, which
  carries what it needs: its goal, its plan's state, its inbox, its notes,
  and, past the resume limit, the transcript's tail.
- **Within a run, sub-agents; beyond it, tasks.** A run delegates work it
  needs now, in its workspace, to sub-agents (section 5); work that should
  outlive the run, run elsewhere, or have its own authority, to tasks.

## 5. Sessions

A session is one conversation with an LLM, opened by a run. It knows the
provider-neutral vocabulary of a conversation and nothing of runs,
charters or results.

- **Turn by turn.** It calls the LLM, runs the tools the LLM asks for,
  sends their results back, and repeats until the LLM yields, a limit ends
  it, or its opener closes it. A session that yields has not ended: its
  opener continues it with a new message, or closes it. Calls in the
  answer it yielded with are answered as not run when it continues.
- **Each turn is told to its opener** as it ends: the LLM's message, its
  calls with their results, and what the completion used, in the
  conversation's own vocabulary with every name resolved to its value, so
  that a turn means the same outside the session that made it. The main
  session's turns are the run's transcript; a sub-agent's are summarised
  in the call that asked for it.
- **Opened from a transcript.** A session may open with earlier turns, as
  they were told, including the provider's opaque blocks, which go back
  to the same provider and endpoint verbatim; a transcript from another
  provider, or one it cannot read, fails the run as transient, saying
  so, and the engine prepares the next run fresh (engine.md, 7.2).
- **Provider-neutral.** A session talks to the endpoint and model it was
  opened with, and nothing in it depends on which provider that is.
- **Retries are policy.** It classifies a failed call (overloaded, rate
  limited, unavailable, timed out, context too long, invalid,
  unauthorised), retries the transient ones after a jittered exponential
  backoff, and gives up when its retries run out. The protocol layer runs
  the attempt and its connect and idle deadlines.
- **Tools run in parallel where that is safe.** Each tool has an effect,
  read or write. Adjacent reads in one turn run together, a write runs
  alone, and results go back in call order. A sub-agent's effect follows
  from its tools; an engine tool's is a write.
- **No chains within a step.** An answer that comes back in the step that
  asked for it (a call the tools refuse at their entrance, one the run
  answers at once) waits on the ready list (programming-model.md,
  section 2), so what one step emits and holds stays bounded.
- **Everything in flight can be stopped.** Every tool call has a deadline,
  and an opener can abort its session, which cancels what is in flight and
  waits for it to settle (programming-model.md, 5.3).
- **Budgets:** turns, tokens (input, output, cache reads and writes, as the
  provider counts them) and time, given by the run at open; bytes held,
  against the agent's limits. Crossing a budget stops the next completion,
  not the turn in flight: the calls of the completion that crossed it
  still run and settle. Time is the exception: when it runs out, the
  session closes at once.
- **Owned and delegated tools.** A session runs the tools it owns through
  its tools child domain, and delegates the rest: to its opener (finish,
  sub-agents, the engine's and connectors' tools) or, later, out of the
  domain (MCP calls, as opaque payloads). To the session they differ only
  in who answers.
- **Context management,** later: when the transcript nears its limit, old
  tool output is elided first; a summarising session, requested from the
  opener like a sub-agent, comes after. A long-lived chat needs it before
  its transcript reaches the resume limit.

The session extension (migration 05e) selects version two with `OpenV2`,
whose concrete `Opening` carries a dialect, charter prices, a deployment-unit
budget and optional typed history. Absence of history never selects version
one. `Open` retains the first-version session unchanged. Versioned `Turn`
and `Transcript` records are domain values; their byte encoding belongs to
the protocol translation in 05g. Each settled turn contains its completion
and surrounding user messages, provider blocks in position, provider call
ids, names and original input bytes. A historical call contains no executable
session ticket. Delegated results contain concrete rendered bytes, and a
withdrawal has its own typed result. Closing waits for actual terminal
responses before telling the turn, including answers that win cancellation.

History is checked before a kit opens or a completion starts: version,
endpoint and dialect; contiguous turn sequence; message and block bounds;
roles and call/result ids; and absence of unresolved session tickets. It
must fit the message and byte limits with the waking prompt and room for a
completion. Version, endpoint, dialect, malformed, unresolved and oversized
history are distinct `TranscriptRefused` reasons. The run treats every one
as transient and starts its next attempt fresh (05f); the legacy run never
opens these sessions. Committed concrete results after the last turn are
restored before the waking prompt. An unanswered call in a yielded tail gets
the same `NotRun` result as an ordinary continuation.

Prices are integer input, cached and output amounts per positive `unit`
tokens (channel.md, 16). New input and cache writes use the input rate;
cache reads use the cached rate. The combined rational charge of **each
completion** is rounded upwards once, using checked arithmetic; the result
and cumulative deployment spend must fit `u64`. Overflow is a typed
`PriceOverflow` ending with an overflow spend report, never a saturated
charge. `Limits::spend` bounds admission, beside unchanged token caps. The
completion crossing the unit budget still runs and settles its tools; the
next completion stops. A concrete delegated terminal, answered or withdrawn, carries the
sub-agent's cumulative spend once under its call identity; duplicate or
stale delivery cannot charge it again. `Priced` and `Turn::spent` are
cumulative within this activation, including these children; restoring
history does not charge old activations again. A run must count each child
through its terminal parent response once, rather than adding both child
spend reports and the parent's inclusive total. Enforcing one aggregate
budget across concurrently open sessions belongs to the run in 05f.

A sub-agent is a delegated tool call that the run serves by opening
another session: the same workspace, its own tools (often read-only, and
never the engine's), a budget carved from the run's, and one of the LLMs
the charter lists (4.1), which the asking LLM may name; otherwise the
child runs on main's. When the child ends, its final message is the
parent's tool result.

## 6. Tools

The tools child domain is what a session does to the workspace: read,
list, search, write, edit and shell.

- **Typed calls, typed outcomes.** A tool call is a domain entity (`Read`,
  `Edit`, `Shell`, ...), decoded by the protocol layer from the JSON the
  LLM wrote, and so is its outcome (`Edited`, `Exited`, ...). The protocol
  layer owns each tool's schema, the decoding, and the rendering of an
  outcome as the text the LLM reads; the domain decides the content, such
  as how much of a command's output to keep. A malformed call arrives as
  an `Invalid` call with a typed problem (a missing field, a wrong type),
  and the domain decides what the LLM is told.
- **The workspace is shared, knowledge is not.** Files on disk are world
  state, reached through io and shared by every session. What a session's
  LLM has read, and at which version, is that session's own state. Write
  authority is copied into the session when it opens.
- **Read before write.** A session may change a file only if its LLM has
  read the current version; creating a file needs no read. The version is
  checked against the real file when writing, so a change made meanwhile
  by another session, or by anything else, is caught.
- **Confined.** Paths resolve inside the workspace's repositories, and
  writes land only in writable ones. Writes and edits follow no symbolic
  link on any part of their path, so a change lands in the repository its
  path names, and a path with a `.git` part, in any case, is refused.
- **Bounded and visible.** Each tool's output has a size limit set here,
  and a failure comes back with its output (a failing test's tail, an edit
  that matched nothing), not a fixed message.
- **Search** is `rg`, run as a contained process: no configuration, the
  pattern and glob passed so that neither reads as an option, an empty
  environment, and read-only repositories. A code graph served over MCP
  (codebase-memory-mcp) is to complement it, as a tool source of its own,
  once MCP servers are a tool source.
- **Shell** runs in a contained process tree (io), with a deadline, the
  head and tail of its output captured, and an environment without
  credentials. The tree's view of the workspace is the confinement: a
  command writes only the writable repositories, and only with the modify
  tools, and every git directory is read-only to it, since the worker
  commits the checked tree, merges in progress included, and the LLM
  needs no git writes.

## 7. Facts

Each child domain pushes what happened as typed facts into a bounded
queue (programming-model.md, section 3): a run admitted or ended, an LLM
call started, retried or finished, a tool or a check started (with its
deadline) or finished, a stream of the text the LLM writes. The protocol
layer projects them for the worker: a content-free stream for liveness,
and an optional trace whose content follows a capture policy. Nothing
the agent decides depends on whether a fact is delivered. What must be
delivered (turns, the answer, what was spent) is not a fact.

## 8. Below the domain

What the protocol and io layers owe the domain. The protocol layer is
designed in `docs/design/protocol.md`, with the LLM providers in
`llm.md`, the channel in `channel.md` and credentials in
`credentials.md`; README.md, section 5 lists what this design changes in
them. io is skein's:

- **LLM providers:** HTTP, server-sent events and JSON for each provider
  API; tool schemas and decoding; classifying failures; refreshing
  credentials.
- **The worker:** one agent process per run (worker.md, section 6), and
  one framed channel over its pipes carrying the charter and transcript,
  messages, answers to tool calls, grants and cancels down; tool calls,
  turns, facts, waiting and the answer up.
- **Turns' encoding:** a turn's bytes, which the engine keeps and the
  web's protocol layer renders, are the conversation vocabulary's, with a
  version, so a later agent can resume an earlier one's transcript or
  know it cannot.
- **MCP servers,** over a child process's pipes.
- **Files and processes,** through io: contained process trees,
  environments without credentials, file reads and atomic writes. The run
  reads `AGENTS.md` as text, UTF-8 cut at a character boundary, and a
  file that is not text counts as no guide.

## 9. The world

The agent's worlds run the domain against neighbours that share none of
its types (testing-strategy.md, section 4): a fake LLM provider whose
completions a script draws (tool calls, malformed input, every class of
failure, streaming); the machine's faces for files, processes and the
programs they run (git directories read-only, `rg`, `sh`, checks that
pass and fail); and the run's host, scripted: messages, answers to the
engine's tools, grants, cancels. Each child domain has a world of its
own; the agent's top-level world is a system world, with the real worker
and engine (worker.md, section 10).

Its stories: a change produced, checked and pushed; a verdict judged
against its contract, corrected and accepted; a chat that waits, hears
its person's words, parks, and resumes from its transcript; a
coordinator that delegates a batch, hears results, and proposes what is
beyond its authority; a resolution from a merge in progress; a sub-agent
on another model; a cancel in the middle of a turn. Its referee: one
answer per run, after every turn it took; turns in order; nothing
written outside the writable repositories; budgets exceeded by at most
one turn; every tool call answered once. What the tests lack is tracked
in `docs/design/testing.md`, section 9.

## 10. Open questions

- **Context management** (section 5): eliding, then summarising, and how
  a summary is kept in the transcript.
- **The environment commands run with** (section 6): empty today, so no
  `PATH`; what a command needs to build and test, without credentials.
- **Which engine tools each charter offers** by default, beyond what its
  task's authority allows; and how a brief teaches an LLM when to delegate
  a task rather than a sub-agent.
- **A run's refusal:** the channel has no refusal, so an agent that
  refuses a run reports it as failed for policy.

## 11. From today

- **The run, the session and the tools stay** as built: charters judged
  at the entrance, sessions with budgets and retries, typed tools,
  finishing with checks and a push, the push's feedback.
- **The run's answer** gains parking, and what it spent on the channel,
  which today it keeps to itself: only best-effort usage facts leave it.
- **The engine's tools are new to the agent.** Today it offers the LLM
  only `finish` and sub-agents; forge reads, notes and comments exist on
  the wire, but the agent fails on a relayed answer. Every tool of 4.3 is
  offered by its charter, relayed, and answered.
- **Messages and waiting are new to the agent.** The worker already
  delivers messages, holds a waiting run's slot and pauses its watchdog;
  the agent drops messages and never waits.
- **Sessions tell their turns,** with their names resolved: today a
  transcript holds names (tickets) meaningful only inside its session,
  and opaque provider blocks. A session opens from a transcript.
- **The charter** carries instructions, tools from authority and a result
  contract in place of the engine's typed step, grants and outlets;
  sessions' turns as the engine's step kind goes, since a chat is just an
  agent task. Today the agent's protocol layer maps the engine's charter
  onto the run's: it renders the brief's typed sections as text, which
  stays; its verdict lists are fixed in code (a report, an approval, a
  request for changes with typed children), which become the result
  contract's; and it refuses a session's turn as unsupported.
- **Results:** a report and a declared failure are not result kinds of
  the run today, which judges changes and verdicts; both are new.
- **Pushing mid-run** is not built: a run pushes only from `finish`,
  which winds it down. A chat's small fix (core.md, 6.3) needs it.
- **A merge in progress** is new to the agent: the files in conflict in
  its start, and a push refused for a marker left, naming the file.
- **Budgets:** today a run is given a token count, split across kinds
  (half for input, a quarter for output, an eighth each for cache reads
  and writes); now it is given what it may spend in the deployment's
  unit, with its models' prices, and prices each completion. Per-kind
  caps stay as the sessions' limits.
- **Not built, as before:** MCP servers as a tool source; facts projected
  for the worker by the protocol layer; one io vocabulary for the tools'
  and the run's operations, which the top level carries as two families.
- **Already done in the domain,** for the protocol layer: providers'
  opaque blocks, grants held by name and named on completions, rejection
  and exhaustion notices, unauthorised failures as transient, and the
  token budget's split.
- **Calls asked again** with the same name, rather than withdrawn and
  left to the LLM, are new.
