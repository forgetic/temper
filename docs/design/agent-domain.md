# The agent's domain layer

Provisional, 2026-10-02. What the temper agent does, as a domain layer:
its parts, what each is responsible for, and how they fit together. The
mechanics are those of skein's `programming-model.md`; this document says
what the agent's domain is made of. Each part's details are settled as it
is built; what is still open is listed in section 9, and what is not built
yet in section 10. How the agent is tested, and the fakes around it, is in
`testing.md`.

## 1. In one page

- **An agent is a flexible LLM driver integrated with the forge.** The
  engine decides what work exists and holds authority over the forge, the
  worker prepares a checkout and hosts the run, and the agent does the LLM
  work: a coding job, a review, a design session with a human, a
  coordinator reporting on a feature. A code change is one kind of outcome
  among several.
- **Runs are short-lived by default, and continuity lives in the forge.**
  Each activation is a fresh run whose brief the engine builds from forge
  state, including what the agent itself wrote last time. Nothing in the
  domain stops a run from living longer (4.5).
- **Three child domains under one root domain.** `run` (one agent
  instance: its charter, its sessions, its outcome), `session` (one
  conversation with an LLM) and `tools` (what a session does to the
  checkout), composed by `temper-agent-domain`, the agent loop's entry
  point.
- **The domain is complete** (programming-model.md, section 4). A world of
  domains and fakes runs everything the agent does, with no protocol and
  no io. The protocol layer translates bytes to domain entities and back,
  including the JSON an LLM writes as a tool's input, and decides nothing.
- **Policy is data.** The agent interprets no workflow vocabulary: no
  role, queue or action names. What differs between agents arrives in the
  charter: the brief, the tools granted, the repositories it may write, the
  outcomes it may produce, its budget.
- **Any model, any provider.** The charter names the model a run starts
  with, and the others its sub-agents may use, on any endpoint the agent
  is configured with, so a sub-agent can run on another model or another
  provider. The conversation vocabulary is provider-neutral; each
  provider's API is the protocol layer's business.
- **Failures are visible.** The LLM sees what went wrong, bounded in size:
  a failing test's output, an edit that matched nothing, an outcome that
  broke its contract. A failure is never replaced by a fixed message.

## 2. The agent in the system

```
engine   workflow and forge authority: what work exists, for whom; applies outcomes
worker   checkouts: starts runs, relays their host calls, pushes their changes
agent    LLM work: runs sessions with tools, reports facts and outcomes to the worker
```

- **The engine is the forge's only API client.** It reads workflow state,
  makes every change (pull requests, bodies, comments, child issues,
  labels), and answers the forge reads and outlets a run asks for. The
  worker's one write is pushing a run's branch, over git.
- **The engine never reads a repository.** It works from forge state:
  issues, pull requests, labels, reviews, CI. What a checkout holds, such
  as a repository's `AGENTS.md` or its checks, is for the run to find.
- **The worker starts a run** with a charter (4.1). Credentials never reach
  the domain: it names an LLM endpoint, and the protocol layer holds the
  rest.
- **The agent acts on the world only through the worker:** pushing a
  change, which the worker does, and the forge reads and outlets its
  charter grants (4.3), which the worker relays to the engine. It never
  pushes, merges or writes to the forge on its own authority.
- **Retrying a failed run is decided above the run,** across attempts. A
  run answers once and is done.

## 3. Structure

```
temper-agent-domain                 the agent loop's entry point: one step, one fire
├── temper-agent-domain-run         one agent instance: charter, sessions, outcome
└── temper-agent-domain-session     one conversation with an LLM
    └── temper-agent-domain-tools   read, list, search, write, edit, shell
```

The tree follows programming-model.md, 4.5: each child domain is a step
machine of its own with its own world; a parent owns its children's state
and routes between them; siblings share no domain types, so the run and
the session meet only through translations in `temper-agent-domain`. Its
vocabulary is what crosses the domain boundary: the worker's requests, the
run's answers and facts, calls to LLM providers, and the file and process
operations the tools ask of io.

## 4. The run

A run is one agent instance. It takes its charter from the worker, opens
the sessions that do the work, serves the tools that act beyond the
checkout, judges what comes out, and answers once.

### 4.1 Charter

What the worker gives a run when it starts it, most of it from the engine's
assignment:

- **Brief.** Text for the LLM: the work item and its lineage, the role's
  charter, the action's guidance, the index of the notes in its scope
  (engine-domain.md, section 10), rendered by the engine, which holds the
  forge and the workflow. The agent adds what it finds in the checkout
  (the repository's `AGENTS.md`) and the sections about its own mechanics
  (its tools, its checkout, how to finish), because those are what it
  enforces.
- **Checkout.** The repositories, where they sit, and which may be
  written: the effective write authority, decided by the engine and
  enforced by the tools, not stated in prose.
- **Grants.** What the LLM may do: tool families (inspect, modify, shell,
  forge reads, sub-agents) and outlets (4.3). Data, never derived from a
  role name.
- **Outcome spec.** What counts as done: a change (the diff, with a title
  and body for its pull request), a verdict from a closed list with a
  contract per verdict (how many children, of which kinds, which fields are
  required), or either. For a change, it also says whether the
  repository's checks must pass (4.4).
- **Budget.** Turns, tokens and wall time for the whole run, across all
  its sessions. It is checked at the entrance against the agent's
  `Limits`; a run that asks for more is refused.
- **LLMs.** The endpoint and model the main session runs on, and the
  others a run may use for sub-agents (section 5). The agent is configured
  with endpoints (a provider and its credentials, held by the protocol
  layer); a charter only names them.

The engine's charter is typed in its own terms (engine-domain.md, 5.3
and section 9), and the protocol layer is to map it onto this one. Until
that layer exists, the agent's top-level world maps it as follows
(testing.md, 4.5):

- **The brief** is the engine's sections as text: the plan's guidance
  first, then why the run is due, then each section under a heading.
- **Grants:** reading is always granted; writing and the shell are the
  engine's modify and shell grants, and its `note` grant the outlet of
  that name. Which repositories are writable is the workspace's.
- **Outcome spec:** a change; a review's verdicts, `approve`, or
  `request-changes` with its children; or a report, which the run
  declares as a verdict named `report`. A session's turn has no mapping,
  as sessions do not reach agents yet (section 10).
- **Budget:** the engine gives one token budget, which is split across
  the kinds the agent bounds (half for input, a quarter for output, an
  eighth for cache reads, the rest for cache writes), so that together
  they spend no more than the engine gave.
- **LLMs:** the deployment's models, listed main first, then those a
  sub-agent may pick.

### 4.2 What a run does

1. **Admits** a request, or refuses it at the entrance: busy, or beyond
   the limits.
2. **Equips** its main session: prompt, tools, checkout authority, LLM,
   and a share of the budget.
3. **Drives** the session. A session *yields* when the LLM stops calling
   tools, and the run decides what happens next: nudge the LLM ("you have
   not finished"), pass on a new inbound message, or close it.
4. **Serves delegated tools:** finishing, forge reads, sub-agents, and the
   outlets its charter grants. It runs each call's deadline: a call that
   times out returns only once what it started has settled (a child
   ended, a landing aborted), so nothing it started is still writing.
5. **Judges** a declared outcome against the outcome spec. A violation goes
   back to the LLM as a tool error it can fix; it does not end the run.
6. **Accounts** every session it opens against one budget, and winds down
   when the budget runs out.
7. **Ends once.** It closes its sessions, waits for what is in flight to
   settle, and answers the worker: an accepted outcome, or a typed failure
   (domain, budget, policy, cancelled, stale) the worker can act on. A
   cancel from the worker takes the same path.
8. **Reports** facts as it goes (section 7).

Only a run opens sessions, so ownership is a tree: a run owns its
sessions, and which session is whose sub-agent is the run's bookkeeping.

### 4.3 Inbound events and outlets

A run is shaped around events in and actions out, even while the first
runs use one of each.

- **Inbound:** the request that starts it; later, a human's message (web
  UI, engine, worker), a forge event it waits for, a timer. The first
  request is just the first event.
- **Outlets:** delegated tools that act through the worker, and through
  the engine where they touch the forge: finish, and later reply to a
  human, comment, open an issue, propose a change, write a note, wait for
  an event. Which outlets a run has is part of its grants. Recalling a
  note is a forge read.

| Agent | Inbound | Outlets | Ends when |
|---|---|---|---|
| coding job | the request | finish with a change | the change passes its checks |
| review, triage | the request | finish with a verdict | the verdict meets its contract |
| design session | human messages | reply, propose issues | the human closes it, or it idles |
| feature coordinator | progress, human questions | reply, comment, open issues | its report is written |

### 4.4 Finishing

`finish` is a tool, not a convention about the last message. Its input is
the outcome, typed by the protocol layer; the run checks it against the
outcome spec and answers with acceptance or with what is wrong. An LLM
that stops without finishing is nudged by the run, within its budget.

For a change, finishing also runs the repository's checks, when the
outcome spec asks for them, and then asks the worker to push:

- **The repository says what its checks are,** by convention: an
  executable at a known path, such as `.temper/pre-pr`, looked up when the
  run starts. A repository without one has no checks, and nothing above
  the agent needs to know either way.
- **Checked is pushed.** The checks run as contained processes with
  deadlines, while nothing else writes to the checkout, and the worker then
  commits and pushes exactly that tree. Nothing else writes by
  construction: a write runs alone in its session, and a sub-agent lives
  only as long as the call that asked for it. A push that lands is the
  outcome, even if the run was winding down meanwhile.
- **Failures are feedback,** like any other: a failing check comes back
  with its output, and so does a push that fails. So does one that finds
  nothing to push, which is how a declared change is checked to exist:
  the worker says so, and the run tells the LLM its push failed. A push
  that finds the branch moved is the exception: it ends the run as stale,
  since every later push of the run would find the same (worker-domain.md,
  section 5) and the engine re-plans against fresh forge state.
- **Not a security boundary.** An agent can change what a check runs; CI
  on the forge and the engine's rules guard landing. The checks catch
  failures early, inside the run that can fix them.

Proposing a change mid-run, as a session does, goes the same way.

### 4.5 Lifetime

Runs are short-lived by default: one activation, then an answer.
Continuity lives in the forge, where it is authoritative and visible: a
coordinator woken by progress reads the feature's state and its own last
report, acts, writes a new report and ends.

Three choices keep longer-lived runs possible without a redesign: the first
request is just the first inbound event; ending is the charter's decision,
not "one outcome, then close"; and run state is plain values, so it can be
snapshotted and resumed. Dormant runs, their persistence, and addressing a
live run from the web UI wait until they are wanted.

## 5. Sessions

A session is one conversation with an LLM, opened by a run. It knows the
provider-neutral vocabulary of a conversation and nothing of runs,
charters or outcomes.

- **Turn by turn.** It calls the LLM, runs the tools the LLM asks for,
  sends their results back, and repeats until the LLM yields, a limit ends
  it, or its opener closes it. A session that yields has not ended: its
  opener continues it with a new message, or closes it. Calls in the
  answer it yielded with are answered as not run when it continues.
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
  from its grants.
- **No chains within a step.** An answer that comes back in the step that
  asked for it (a call the tools refuse at their entrance, one the run
  answers at once) waits on the ready list (programming-model.md, 2), so
  what one step emits and holds stays bounded.
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
  forge reads, outlets, sub-agents) or out of the domain (MCP calls, as
  opaque payloads). To the session they differ only in who answers.
- **Context management,** later: when the transcript nears its limit, old
  tool output is elided first; a summarising session, requested from the
  opener like a sub-agent, comes after.

A sub-agent is a delegated tool call that the run serves by opening
another session: the same checkout, its own grants (often read-only), a
budget carved from the run's, and one of the LLMs the charter lists
(4.1), which the asking LLM may name; otherwise the child runs on main's.
When the child ends, its final message is the parent's tool result.

## 6. Tools

The tools child domain is what a session does to the checkout: read, list,
search, write, edit and shell.

- **Typed calls, typed outcomes.** A tool call is a domain entity (`Read`,
  `Edit`, `Shell`, ...), decoded by the protocol layer from the JSON the LLM
  wrote, and so is its outcome (`Edited`, `Exited`, ...). The protocol layer
  owns each tool's schema, the decoding, and the rendering of an outcome as
  the text the LLM reads; the domain decides the content, such as how much
  of a command's output to keep. A malformed call arrives as an `Invalid`
  call with a typed problem (a missing field, a wrong type), and the
  domain decides what the LLM is told.
- **The checkout is shared, knowledge is not.** Files on disk are world
  state, reached through io and shared by every session. What a session's
  LLM has read, and at which version, is that session's own state. Write
  authority is copied into the session when it opens.
- **Read before write.** A session may change a file only if its LLM has
  read the current version; creating a file needs no read. The version is
  checked against the real file when writing, so a change made meanwhile
  by another session, or by anything else, is caught.
- **Confined.** Paths resolve inside the checkout's repositories, and
  writes land only in writable ones. Writes and edits follow no symbolic
  link on any part of their path, so a change lands in the repository its
  path names, and a path with a `.git` part, in any case, is refused.
- **Bounded and visible.** Each tool's output has a size limit set here,
  and a failure comes back with its output (a failing test's tail, an edit
  that matched nothing), not a fixed message.
- **Search** is `rg`, run as a contained process: no configuration, the
  pattern and glob passed so that neither reads as an option, an empty
  environment, and read-only repositories. A code graph served over MCP
  (codebase-memory-mcp) complements it, as a tool source of its own.
- **Shell** runs in a contained process tree (io), with a deadline, the
  head and tail of its output captured, and an environment without
  credentials. The tree's view of the checkout is the confinement: a
  command writes only the writable repositories, and only with the modify
  grant, and every git directory is read-only to it, since the worker
  commits the checked tree and the LLM needs no git writes.

## 7. Facts

Not yet discussed in depth. Each child domain pushes what happened as
typed facts into a bounded queue (programming-model.md, section 3): a run
admitted or ended, an LLM call started, retried or finished, a tool or a
check started (with its deadline) or finished, the usage of each turn. The
protocol layer projects them for the worker: a content-free stream for
liveness, and an optional trace whose content follows a capture policy.
Nothing the agent decides depends on whether a fact is delivered.

## 8. Below the domain

Not yet discussed in depth. What the protocol and io layers owe the
domain, to be designed after it:

- **LLM providers:** HTTP, server-sent events and JSON for each provider
  API; tool schemas and decoding; classifying failures; refreshing
  credentials.
- **The worker:** one agent process per run (worker-domain.md, section 6),
  and one framed channel over its pipes carrying requests, answers, facts,
  cancels and the run's host calls.
- **MCP servers,** over a child process's pipes.
- **Files and processes,** through io: contained process trees,
  environments without credentials, file reads and atomic writes. The run
  reads `AGENTS.md` as text, UTF-8 cut at a character boundary, and a
  file that is not text counts as no guide.

Each layer below is tested against fakes that grow with it
(testing.md, section 3): the fake LLM provider's own protocol and
io layers, and the machine's faces for files, processes and the programs
they run.

## 9. Open questions

- **Facts and the layers below** (sections 7 and 8).
- **Long-lived runs:** what a run puts in its snapshot. The engine keeps
  snapshots as a cache and addresses a run through its item
  (engine-domain.md, section 6).

## 10. Not built yet

The domain layer runs everything above in its worlds. What it does not do
yet, each to be designed before it is built:

- **Context management** (section 5): eliding old tool output as a
  transcript nears its limit, then a summarising session.
- **MCP servers as a tool source** (sections 5 and 6): calls that leave
  the domain as opaque payloads, starting with the code graph that
  complements `rg`.
- **The agent's half of the channel** (section 8; worker-domain.md,
  section 6), which the worker speaks already: inbound events beyond the
  first request, and the run saying it waits, with how many it has read;
  parking with a snapshot, and resuming from one (4.5); forge reads and
  outlets beyond finish (4.3), relayed to the engine, and their answers.
  The channel has no refusal and no place for what a run spent: a refused
  run reaches the worker as failed for policy, and its spend is dropped.
- **Facts for the worker** (section 7): the content-free stream for
  liveness and the trace, projected by the protocol layer.
- **The environment commands run with** (section 6): empty today, so no
  `PATH`; what a command needs to build and test, without credentials.
- **The layers below the domain** (section 8), including one io vocabulary
  for the tools' and the run's operations, which the top level carries as
  two families for now.

What the tests lack, from the fakes' layers below the domain to the tiers
above the domain worlds, is tracked in testing.md, section 9.
