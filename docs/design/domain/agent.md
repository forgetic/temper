# The agent

Provisional, 2026-10-05. temper's agent is smith: the kit for flexible
LLM agents, and the standard agent built from it, in smith's repository,
on skein like temper. What smith is and does is its own design (smith's
`docs/design/domain/`, cited as smith's `run.md`, `session.md`,
`tools.md`, `host.md`). This document says what temper takes from smith
and what temper fills in: the charters its engine writes, the tools its
engine serves, its result contracts and conventions, and its worker as
smith's host. What is still open is listed in section 10; where the
previous version of this document went, in README.md, 6.5.

## 1. In one page

- **temper runs smith's stock agent.** A worker spawns smith's agent for
  each run, or composes its domain in one process; temper builds no code
  into it. Policy is data: a run's charter and its host's answers are all
  that make it temper's.
- **The engine writes the charter** (section 4): the task's instructions,
  its brief rendered, the engine's tools and its connectors' reads as host
  tools, a result contract with temper's verdicts and a change's title and
  body, `.temper/pre-pr` and `AGENTS.md` as conventions, a budget in the
  deployment's unit with its models' prices.
- **The worker is smith's host** (section 6 and worker.md): it prepares
  the workspace from the forge, supervises the agent's process with
  smith's host domain, delivers a change by pushing it, and relays the
  rest to the engine.
- **The engine keeps what smith leaves to hosts:** transcripts, turn by
  turn, in its store; host tools' calls, decided once; what follows an
  answer.
- **One translation.** The engine's protocol layer is the only place where
  temper's charter, tools and messages become smith's, and smith's calls
  become the engine's. smith interprets no temper vocabulary, and the
  worker none of either.
- **temper imports smith at every layer** (section 3): its domains in
  temper's worlds and in one process, its protocol crates in the engine's
  and the worker's protocol layers, its binary on workers.

## 2. The agent in the system

```
engine   tasks, authority, connectors, the store; writes each run's charter and serves its tools
worker   smith's host: workspaces, agent processes, pushing their changes; relays the rest
agent    smith: LLM work, one run per process, reporting to its worker
```

- **The engine is the only client** of the store and of every
  connector's system. It answers the host tools a run calls, and commits
  the run's turns and result. The worker's one write is pushing a run's
  branch, over git.
- **The engine never reads a repository.** It works from what its
  connectors read; what a workspace holds, such as a repository's
  `AGENTS.md` or its checks, is for the run to find.
- **The worker starts a run** with smith's start: the charter the engine
  wrote, carried as bytes, and the workspace the worker prepared.
  Credentials never reach a domain: a charter names an LLM endpoint, and
  the protocol layers hold the rest.
- **The agent acts on the world only through the worker:** a delivery,
  which the worker makes as a push, and host tools, which the worker
  relays to the engine. It never pushes, merges or writes to a system on
  its own authority.
- **Retrying a failed run is decided above the run,** by the engine,
  across attempts. A run answers once and is done.

## 3. What temper takes from smith

| Where in temper | smith's crates | For |
|---|---|---|
| the engine's domain | none | transcripts are bytes; the charter is temper's own |
| the engine's protocol layer | `smith-channel` (charter, calls, answers, messages), `smith-transcript` | encoding charters, decoding host tools' calls, rendering their answers; the web rendering turns |
| the engine's protocol layer | `smith-oauth` | refreshing LLM accounts' tokens (`credentials.md`) |
| the worker's domain | `smith-host-domain` | its agent child: processes, the channel's rules, the watchdog, cancel then kill |
| the worker's protocol layer | `smith-channel`, the host's half | the channel over an agent's pipes |
| workers' machines | `smith`, the binary | the agent process |
| worlds | `smith-domain`, smith's fakes and scripted agent | the worker's and the system worlds (section 9) |
| one process | `smith-domain` | engine, worker and agents in one process (smith's `host.md`, section 9) |

What both would otherwise keep goes to skein, as smith's `README.md`,
section 4, lists: framed channels, an OAuth client (temper's web signs
people in with the forge's), supervised processes.

## 4. temper's charter

The engine decides an agent task's charter in its own terms (engine.md,
7.1); its protocol layer encodes it as smith's (smith's `run.md`, 3.1),
in one place, `temper-engine-protocol` (section 11; the migration's
07a). The agent process's translation belongs to smith's `smith-protocol`.

### 4.1 Instructions and brief

- **Instructions:** the task's charter's instructions, from its template
  in configuration or its spec: a chat, a coordinator, a reviewer with a
  lens, a producer of a change.
- **The brief:** the typed sections the brief child domain gathered and
  cut to its budget (engine.md, section 9), rendered as smith's titled
  sections: the task and its lineage, its inbox, its dependencies'
  results, its delegates and their states, its earlier attempts, the
  calls committed since its last turn, the proposals and escalations
  waiting for it, its notes' index, a transcript's tail, and what its
  connectors say of its resources (for the forge, the pull request, CI's
  failures with their output, reviews' remarks, a conflict). smith adds
  the sections about its own mechanics, and each repository's guide.

### 4.2 Workspace

The engine names the repositories, where each starts, which may be
written and the branch a change goes to (engine.md, 7.1); the worker
prepares them and gives smith the workspace: each repository's directory
and name, whether it may be written, and for a merge in progress the
files in conflict (worker.md, section 5). Every temper workspace is git
repositories.

### 4.3 Tools

A task's tool families (authority.md, section 3) become smith's:

| temper's family | In smith |
|---|---|
| inspecting, modifying, the shell | smith's inspect, modify and shell |
| sub-agents | smith's sub-agents |
| `delegate`, `message`, `amend`, `cancel`, `release`, `decide`, `propose`, `subscribe`, `unsubscribe`, `effect`, `note`, `recall` | host tools, each declared with its schema, its description and its effect (engine.md, 7.3) |
| each connector's reads | host tools, one per read the connector defines, each a read |
| waiting, by its wake policy | smith's `wait` |
| a change, by its result contract or mid-run | smith's `deliver` |

- **Declared and decoded by the engine's protocol layer:** each host
  tool's JSON Schema and description are written once there; a call's
  input arrives as the LLM wrote it, is decoded into the engine's typed
  call, or answered as an error naming what is wrong.
- **Effects:** connectors' reads and `recall` are reads; every other
  engine tool is a write, run alone.
- **Answers are rendered there too,** as smith's text: done, with what it
  made; refused, and why; beyond authority, naming what it lacked, so the
  LLM may `propose` it instead.
- **Calls are named by the run** and decided once by the engine, from its
  record when asked again (engine.md, 7.3); smith asks again under the
  same name after busy or a lost answer (smith's `run.md`, 5.2).

### 4.4 Result contracts

A task's result contract (tasks.md, section 3) becomes smith's (smith's
`run.md`, 7.1):

| temper | In smith |
|---|---|
| a report: words, bounded | a report |
| a verdict from a closed list, each with the fields it requires and the follow-up tasks it may propose, how many and of which kinds | a verdict: the same labels and fields; follow-ups as items, whose kinds are the kinds of task a verdict may propose, with their fields |
| a change: a branch pushed | a change, requiring the fields `title` and `body`, checks required unless the contract waives them |
| a failure | a failure |

The engine judges a result again as it commits it, against the contract
and the state as it is now (tasks.md, 5.6).

### 4.5 Conventions

Checks at `.temper/pre-pr`, guides at `AGENTS.md`.

### 4.6 Budget, models, waiting

- **Budget:** what the task has left, capped per run by the deployment,
  in the deployment's unit (authority.md, section 7), with each model's
  prices from its account (engine.md, section 12); turns and wall time.
- **LLMs:** the endpoints are the deployment's accounts; the charter names
  the main session's and those its sub-agents may use; their credentials
  come to the agent as grants through the worker (`credentials.md`).
- **Waiting and resuming:** the waiting time is the charter's; a
  coordinator's is zero. A chat resumes its transcript by default, and a
  coordinator starts fresh from its brief (engine.md, 7.4).

## 5. Messages

What a task hears while its run is live (tasks.md, 7.1) goes to smith as
messages, rendered by the engine's protocol layer: a sender's label (its
requester, a delegate by number, a person by name, a subscription's
topic) and the message's words, whole. Proposals and escalations waiting
for the task's decision are relayed the same way, each naming what
`decide` takes. Every message is named; the run names the last it read
with each turn and when it waits, and the engine takes the inbox up to
that one as it commits the turn (engine.md, 7.2).

## 6. Delivery is a push

smith's `deliver` is the worker's push (worker.md, section 5): the
worker commits exactly the tree the run checked, with the result's
`title` and `body`, and pushes it as a fast-forward, never forced, to the
branch the assignment names; a merge in progress is committed as the
merge, with both parents. Its outcomes, in smith's terms (smith's
`run.md`, 8.2):

| The push | smith's delivery |
|---|---|
| landed in every repository with a change | delivered, naming each head |
| nothing to push | nothing |
| a branch moved, as a fast-forward cannot take it or its head is not the one expected | stale: the run ends, and the engine decides afresh |
| a conflicted file still holds a marker | refused, naming the file |
| refused by the forge, unreachable, timed out, broken, too large, missing a branch or a commit | failed, with the reason and the last 512 bytes of git's diagnostic output |

A push that lands is the result, even if the run was winding down. The
answer carries what the run left on the forge (worker.md, 4.2), and the
forge's change procedure goes on from it (forge.md, section 8).

## 7. Turns and transcripts

- **Turns are smith's** (smith's `session.md`, section 3): bytes of its
  versioned conversation vocabulary. The worker reads only their number,
  size, spend and last message read (worker.md, section 8); the engine
  keeps them opaque in its store (engine.md, 7.2); the web renders them
  with `smith-transcript`.
- **Resuming:** within the resume limit, a run is given its task's
  transcript and the calls committed after its last turn; past it, or
  when smith refuses the transcript, the next run starts fresh, its brief
  carrying the tail.

## 8. The agents temper runs

| Agent | Messages in | Tools it is given | Ends when |
|---|---|---|---|
| producing a change | none | the workspace's; `finish` with a change | its change passes its checks and is pushed |
| a review, a triage | none | reads; `finish` with a verdict | its verdict meets its contract |
| resolving a conflict | none | the workspace's, from a merge in progress; `finish` with a change | the merge is resolved, checked and pushed |
| a chat | its person's words; its delegates' results | most; `deliver` mid-run; `wait` | its person closes it |
| a coordinator | questions, results, overlapping landings; proposals and escalations to decide | `delegate`, `amend`, `cancel`, `decide`, `propose`, `note`; `wait` | its goal's report is written |

## 9. The world

smith's worlds test the agent itself (smith's documents, each section
"The world"). temper's test what it fills in, and the agent under its
host:

- **The engine's protocol tests:** every charter the engine writes
  encodes to one smith admits; every host tool's schema accepts what the
  engine's decoding takes and nothing it refuses; every answer renders
  within smith's limits.
- **The worker's worlds:** smith's scripted agent, for the agent child
  (worker.md, section 10); and the system world, where the worker meets
  the real engine and each run is smith's domain with a fake LLM
  provider, its tools working in the trees the worker prepared.
- **Stories that are temper's:** a change produced, checked and pushed,
  and its push found moved; a coordinator that delegates a batch, hears
  results, and proposes what is beyond its authority; a chat that waits,
  hears its person's words, parks, and resumes from its transcript; a
  host tool answered busy by the engine, asked again and decided once.

## 10. Open questions

- **Which engine tools each charter offers** by default, beyond what its
  task's authority allows; and how a brief teaches an LLM when to delegate
  a task rather than a sub-agent.
- **Runs with no repository:** smith allows a run without a workspace,
  which a coordinator or a chat may want; the worker refuses an empty
  workspace today.
- **A code graph** for a workspace (worker.md, section 12), as an MCP
  tool source once smith has one.
- **Moving with smith:** which revision of smith temper pins, and how a
  change to smith's channel reaches temper's workers.

## 11. From today

- **The agent's crates move to smith:** `temper-agent-domain` and its
  children, `temper-llm-anthropic`, `temper-llm-openai`, `temper-oauth`,
  and `temper-fake-llm-domain` and `temper-fake-llm-protocol`, with their
  worlds, as smith's crates of the same parts (smith's `README.md`,
  section 4).
- **`temper-agent-protocol` splits:** the agent process's half (the
  channel's agent side, providers, tools' schemas, rendering the prompt)
  is smith's `smith-protocol`; temper's half (rendering the brief,
  mapping the charter, the verdict lists fixed in code, the engine tools'
  schemas and answers) stays, as the engine's protocol layer's.
- **`temper-channel` loses the agent's hop** to `smith-channel`; it keeps
  the engine's channel with workers.
- **`temper-worker-domain-agent`** becomes `smith-host-domain`.
- **What the previous version of this document planned as new** (the
  engine's tools offered and relayed, messages and waiting, turns and
  resuming, results that are reports and failures, pushing mid-run, a
  merge in progress, budgets in a unit with prices, calls asked again)
  is built in smith, generic (smith's `run.md`, section 14), and filled in
  here.
- **The legacy run** and the session's first version stay in temper until
  the cutover (`docs/plans/next-domain/07-cutover.md`).
