# temper's domain

Provisional, 2026-10-04. The next design of temper's domain layer: temper
as a generic engine on five primitives, which owns its state in its own
store and drives the forge as a connector. It replaces the domain
documents in `docs/design/` (`engine-domain.md`, `worker-domain.md`,
`agent-domain.md`) and the draft it grew from (`docs/design/draft/core.md`,
removed with this set), and changes what the protocol layer's documents
say in the places section 5 lists. The legacy engine is described by
`docs/design/engine-domain.md` until the cutover
(`docs/plans/next-domain/07-cutover.md`). New code is written against this
directory and cites it as `domain/<file>.md`. The migration plan from today's
code to this design is built from sections 5 and 6 and each document's
"From today" section.

## 1. Reading order

1. **core.md:** the model: the five primitives, decisions and the store,
   projects and goals, plans and coordinators, temper and external
   systems, people, and what temper promises. Read it first.
2. **tasks.md:** the hub: tasks, batches, lifecycle and closing, holds,
   inboxes and wake policies, proposals and escalations.
3. **authority.md:** what a task may do: authority as a value, budgets
   and spend, checks, what proposals need and what accepting them funds,
   the rules no task loosens.
4. **engine.md:** the engine's domain as a whole: its parts, commits and
   the store, restart, agents' runs and the engine's tools, and the
   capabilities it keeps (the fleet, briefs, notes, views, accounts).
5. **connectors.md:** the contract every external system meets:
   resources and write holds, effects and the outbox, topics,
   procedures, projections, drift, adoption.
6. **forge.md:** the forge connector: keeping up, the change procedure,
   landing queues, updates and conflicts, branches, issues.
7. **people.md:** people as parties: identity, roles, requests, inboxes,
   chats, person tasks.
8. **worker.md** and **agent.md:** the worker, complete, changed
   little, and smith's host; and the agent, which is smith: what temper
   takes from it and fills in. smith's own design is in its repository
   (`docs/design/domain/`).

## 2. Conventions

- **A bare file name** names a document of this directory: `forge.md` is
  the forge connector here. A document of today's design is named by its
  path, `docs/design/forge.md` (the forge's protocol layer), or as "the
  current `engine-domain.md`".
- **skein's foundation documents** are named as their own file names:
  `programming-model.md`, `testing-strategy.md`, `notes.md`, cited by
  section, as today.
- **smith's documents,** in smith's repository's `docs/design/domain/`,
  are named with smith's name: smith's `run.md`, cited by section.
- **Each document** says in one page what it is, then its structure, its
  parts, what it owes and is owed below the domain, its world, what
  changes from today, and what is open.

## 3. Names

| Name | What it is | Today |
|---|---|---|
| deployment | one engine, its store, its workers, its configuration; an id made at its first start | the same, with no id |
| project | repositories with roles, a home repository, a policy, people's roles, notes (core.md, 5.1) | the deployment's repositories |
| task | a unit of work someone asked for (core.md, 3.1) | an item: an issue or pull request with a record |
| goal | a tracked task: a priority, an issue, landings as news, a notes scope (core.md, 5.2) | a plan's goal item |
| plan | the subtree of tasks one task delegated (core.md, 6.1) | a typed graph of steps in a goal's record |
| batch | the tasks one decision makes, checked whole (tasks.md, section 4) | a plan accepted, or its growth |
| input | an ended task's result a new task names, read in its brief (tasks.md, section 3) | a dependency's outcome |
| closing | a task settling before it ends: its run, its delegates, its effects, its releases (tasks.md, 5.1) | applying an outcome |
| executor | who carries a task out: an agent, a procedure, a person (core.md, 3.2) | a step's primitive |
| charter | how an agent executor is set up: instructions, models and their prices, tools, result contract, wake policy (engine.md, 7.1) | a step's charter; the agent's charter |
| result contract | what counts as done (tasks.md, section 3) | an outcome spec |
| chat | an agent task whose requester is a person (people.md, section 7) | a chatting session |
| coordinator | an agent whose charter is to delegate and supervise (core.md, 6.2) | a supervising session |
| run | one activation of an agent task on a worker | a run of an item |
| attempt, attempt number | one try at a run, fenced by a number that only grows | the same |
| tries | the failed attempts counted per class, which a release resets (tasks.md, 5.5) | failures per class |
| transcript | an agent task's conversation, kept turn by turn (engine.md, 7.2) | a snapshot, a cache |
| turn | one LLM turn of a run, committed as it ends, numbered within its attempt | none |
| call | a run's tool call, named by the run, decided once (engine.md, 7.3) | a relayed call |
| brief | the context a run is given for why it runs (engine.md, section 9) | the same |
| message | what a task hears once started (core.md, 3.3) | an inbox event |
| inbox | a task's durable, bounded messages (tasks.md, 7.2) | derived from comments after a record's position |
| wake policy | which messages wake a task, and how they batch (tasks.md, 7.3) | a session's wake rule |
| subscription, topic | a task's standing interest; a connector's subject (tasks.md, 7.4) | subscriptions, unfed |
| reference | a task another may message (tasks.md, 7.5) | none |
| authority | what a task may do (authority.md, section 3) | rules, envelopes, grants |
| fit | whether a delegate's authority goes under its creator's, given what it has left (authority.md, section 5) | within an envelope |
| grant | an effect or a read on a prefix of resources (authority.md, section 3) | a run's grants of tool families |
| funder | who a task's budget was reserved from: a task, a person's pool, a project's period (authority.md, section 7) | none |
| proposal | an action beyond its proposer's authority, waiting for a holder (tasks.md, section 8) | acceptance |
| escalation | a held task, waiting for a holder's decision (tasks.md, section 8) | an escalation outcome; a hold |
| rules, policy | the deployment's, a project's: the root of authority (authority.md, section 6) | the deployment's rules |
| requirement | facts an effect needs, whatever the authority (authority.md, section 10) | gates, the protected-landing rule |
| connector | an external system as temper sees it (connectors.md) | the forge child domain |
| resource | what has an identity on a system, named by a path (connectors.md, 3.1) | an item, a branch |
| write hold, writer slot | a resource a task writes alone; the one run that may write it at a time (connectors.md, 3.3) | one live run per item |
| held | a task's phase while it waits for a decision (tasks.md, 5.1) | held |
| effect | what temper does on a system (connectors.md, section 4) | a write |
| outbox | effects committed, waiting to be made (core.md, section 4) | an outcome recorded, then applied |
| procedure | engine code as an executor (connectors.md, section 6) | the plan's change mechanics |
| projection | what a connector writes of a goal for people to read (connectors.md, section 7) | records and labels on items |
| change | a task the forge's change procedure runs to landing (forge.md, section 8) | a change step |
| gate | a verdict a change needs at its exact head (forge.md, 8.3) | a step's review; approvals |
| landing queue | ready changes into one branch, in order (forge.md, section 9) | none |
| workspace | a run's checkout | a checkout |
| workstream | the key a workspace is cached under: the holding task's number | an item's workstream |
| owned, participating, context | what temper writes alone, writes among others, only reads (core.md, 7.1, 7.2; 5.1) | none |
| drift | a change temper did not make to what it owns and relies on (connectors.md, section 10) | mangled records, a deleted branch |
| session | an agent's conversation with an LLM, only (smith's `session.md`) | also an engine item that chats |
| smith | the agent kit, and the agent, temper runs (agent.md) | temper's own agent crates |
| host tool | an engine tool or a connector's read, as smith offers it to the LLM (agent.md, 4.3) | a relayed call |
| delivery | smith's request that its host make a change durable: for temper, a push (agent.md, section 6) | a push |
| sign-in | a person signed in to the web (people.md, section 3) | none |

## 4. What goes

- **Records in comments,** their nonces, mangled records, and the restart
  contract built on reading them back.
- **The forge as temper's database:** an issue per item, labels as the
  index of live work, the hand-in label, the wiki for notes.
- **Plans as typed graphs** of fixed primitives, with envelopes and
  growth.
- **Sessions as items,** and snapshots as caches.
- **People acting on the forge** as a way to reach temper.
- **Configuration as the set of repositories:** repositories are adopted
  into projects at runtime, and may be on several forges.

## 5. What the other documents now owe

What this design changes in each, for the migration plan:

- **smith** (smith's `docs/design/domain/`): the agent's domain, its
  protocol layer's agent half, its providers and its channel with the
  worker leave temper for smith's repository (agent.md, section 11);
  what this list says below of the agent's hop of `channel.md` and of
  `llm.md` is smith's protocol design's to carry, and temper keeps the
  engine's protocol layer's half: charters encoded, host tools declared,
  decoded and answered, messages and the brief rendered.

- **`docs/design/protocol.md`:**
  - the store becomes a boundary in its own right: commits numbered and
    answered once durable, loads in pages by key range, temper's sized
    records with a version per shape, and secret records the protocol
    layer alone writes (refresh tokens, sign-ins);
  - the web, listed under "later" in its section 8, comes first: it and
    the store go before records in comments can go, so section 10's
    order of building changes;
  - runs and attempts are named by task, no longer by repository and
    item;
  - formats inside forge comments shrink to keys; snapshots give way to
    transcripts.
- **`channel.md`:**
  - up: turns, numbered within their attempt, kept until acknowledged,
    each with its spend and the last message it read; the answer with
    its turn count and its whole spend; the worker's graces and push
    deadline at its hello;
  - down: acknowledgements of turns; a transcript, with the calls
    committed after its last turn, in the assignment and in the agent's
    start, in place of a snapshot; a merge in progress as a starting
    point, with its conflicted files in the agent's start;
  - messages for a run, carried whole and named: the kinds of tasks.md,
    7.1, in place of today's news, finished, held and decided, which
    carried ids;
  - relayed calls: the engine's tools (engine.md, 7.3) and the
    connectors' reads, in place of today's forge reads, notes, comments
    and escalations, each named so it can be asked again;
  - the outcome: a change, a verdict, a report or a failure; plans,
    steps, tasks, replies, finished, releases and escalations go, as
    tools;
  - the charter: instructions, tool families from authority, a result
    contract, a budget in the deployment's unit with its models' prices,
    waiting time; the brief stays typed sections, rendered by the
    agent's protocol layer; repair reasons go to the brief;
  - names: runs by task, repositories no longer a fixed index of the
    deployment's (the 256-repository packing goes);
  - the snapshot payload goes; a versioned turn payload takes its place,
    whose schema is the session vocabulary's (`llm.md`);
  - its caps and sizes gain unacknowledged turns and transcripts; its
    section 14 is rewritten. The channel's first version may still
    change freely, as it has not shipped (`channel.md`, 4.5);
  - the agent's hop leaves for `smith-channel`: the worker speaks it as
    smith's host, carrying the charter as bytes, and `channel.md` keeps
    the engine's hop with workers.
- **`credentials.md`:** an LLM account's refresh token moves from
  temper-oauth's file record into the store's secret records, still
  written by the engine's protocol layer, never by the domain; the web's
  OAuth client with the forge, for signing people in, is a credential of
  its own; git identities, webhook secrets and API tokens (one per
  forge) come with repositories as they are adopted, not from a fixed
  map at startup.
- **`llm.md`:** moves to smith's protocol design, but for what the
  engine's protocol layer keeps of it: schemas, decoding and rendering
  for every engine tool, each connector's reads and `wait`; `finish` taking the result
  contract; a session's turns encoded with a version and their names
  resolved, its providers' opaque blocks kept verbatim with the dialect
  and endpoint they came from, so a later session can open from them;
  completions priced as they are made.
- **`docs/design/forge.md`:** as forge.md, section 17: records, outcome
  blocks, notes' pages and the wiki, nonces, the person in a key's marker,
  the wiki's webhook and listings by label go; keys' markers stay,
  carrying the deployment's id; new calls for a pull request's files and
  diff, comparisons, updating from a base, commit statuses, editing
  titles and bodies, branch protection, collaborators, repository
  settings, creating a branch at a commit, and CI's output; repositories
  adopted at runtime, on one forge or several.
- **`docs/design/testing.md`:** the engine's world keeps its fake store and
  its scripted people, the store gaining ordered commits, paged loads
  and failing commits, the people acting only on the web; forge-side
  people's stories go; worlds for the new child domains (tasks, people,
  the forge's subtree) replace `tests/engine/work` and `tests/engine/plan`;
  the fake forge grows two-parent merges, updates that conflict,
  statuses, branches made at a commit, files, comparisons, CI's output,
  protection, settings and collaborators; the fake checkout grows merges
  that conflict; runs are named by task; the referee holds core.md,
  section 10.
- **`docs/design/performance.md`:** a turn's bytes now also go up the
  channel and into the store, one more copy per turn of what section 4
  counts, bounded by a turn's limit; a resumed run's transcript comes
  down once, at its start, and a worker holds one per slot; turns kept
  unacknowledged are bounded per run; the engine's working set is of
  tasks, no longer of forge items (section 2).
- **`docs/development/protocol-implementation.md`:**
  - of the paused forge increment, the committed but unexported
    `forge_blocks.rs` and `forge_cursor.rs` are deleted, and wiki pages,
    listing by label, label writes and the wiki's webhook in
    `temper-forge-forgejo` and the fake forge go; Forgejo's documents and
    webhooks, and the calls for pull requests, reviews, statuses,
    branches, issues and keyed creations, stay;
  - decisions it holds that this design settles: acknowledging messages
    by the last one read, with names per attempt; spend on the channel;
    who keeps the refresh token (the store's secret records); labels on
    keyed creations, now for display only; opaque snapshots, which go.
- **Citations.** About two hundred citations of the current domain
  documents, in code, tests and the protocol documents, are repointed as
  the migration moves each part.

## 6. Where today's documents went

### 6.1 `engine-domain.md`

| Section | Now |
|---|---|
| 1. In one page | engine.md, 1; "the forge is the truth" and "everything is an item" replaced by core.md, 1 and 4 |
| 2. The engine in the system | engine.md, 2; people acting on the forge gone (people.md, 10) |
| 3. Structure; records composed and split; the restart contract | engine.md, 3 and 6; records gone, for the store's (engine.md, 5) |
| 4.1 What an item is | tasks.md, 3; records and labels gone; one live run in tasks.md, 5.2 |
| 4.2 Lifecycle | tasks.md, 5; failure classes and holds in 5.5; hold codes gone |
| 4.3 The inbox | tasks.md, 7.2; the comment inbox in forge.md, 13, for participating objects |
| 4.4 Outcomes | engine.md, 5 (commit, then effects); connectors.md, 4 (keys, uncertainty); tasks.md, 5.1 (closing); acceptance in authority.md, 9 |
| 4.5 Engine actions | forge.md, 6 and 8: a procedure's effects |
| 4.6 Taking work in | people.md, 5: chats and goals; the hand-in label gone |
| 5.1 Primitives | core.md, 3.2 (executors); changes in forge.md, 8; waits as dependencies and person tasks; sessions as chats; gates in forge.md, 8.3 and authority.md, 10 |
| 5.2 A plan; envelopes; templates | core.md, 6; envelopes as authority (authority.md, 5); templates as charters in configuration (engine.md, 14); a step done only after the steps it added, in tasks.md, 5.6 |
| 5.3 What is due | forge.md, 8 for changes; tasks.md, 5 for the rest; wall times in engine.md, 5.4 |
| 5.4 Wakes | tasks.md, 7.3 |
| 6. Sessions | people.md, 7 (chats); core.md, 6.2 (coordinators); engine.md, 7.4 (resume or fresh); snapshots gone |
| 7. Rules | authority.md, 12 says where each rule went |
| 8. The fleet | engine.md, 8; where a change's branch is, in forge.md, 8.2; the graces, now ordered, in engine.md, 8 |
| 9. Briefs | engine.md, 9 |
| 10. Notes | engine.md, 10, in the store |
| 11. Views | engine.md, 11 |
| 12. The forge | forge.md, 5; the wiki and labels as an index gone |
| 13. Below the domain | engine.md, 14; forge.md, 17 |
| 14. The world | engine.md, 15 |
| 15. Open questions | people on the forge: people.md, 14; bounced events: settled, named (worker.md, 11); an item closed under a live run: settled, a cancel closes it (tasks.md, 6); a call that waits for a person: a proposal (tasks.md, 8); notes in the wiki: gone; templates: charters in configuration; spend: authority.md, 7; keeping up: forge.md, 5, and a resource readable again in forge.md, 20; briefs (their budget, cancelling one, a missing section): engine.md, 17; views: engine.md, 17; snapshots: gone; checks without an LLM: forge.md, 20 and worker.md, 12; saved-work branches: forge.md, 11; provisioning: connectors.md, 11 |
| 16. Not built yet | a warm start: gone, the store is read at start; an item made while the table is full: gone, a batch commits whole; subscriptions: tasks.md, 7.4; labels: gone; asking for reviews: participating objects only (forge.md, 13); checks-only runs: forge.md, 20; landing a branch others built: forge.md, 11.1; starting from saved work or a commit: engine.md, 16; dependencies' results in briefs: engine.md, 9; CI's output: forge.md, 2 and 17; a message carried whole, and its text for a live run: tasks.md, 7.2; spend: authority.md, 7; wall time: engine.md, 5.4; notes held for acceptance: gone; reading traces back: engine.md, 11; templates in the wiki: gone; plans in the whole worker's world: `docs/design/testing.md`; the protocol layer's asks (comments from a time, costs, hints' causes, listings without text): forge.md, 17 and the current `docs/design/forge.md`, 12 |

### 6.2 `worker-domain.md`

| Section | Now |
|---|---|
| 1 to 7 | worker.md, 1 to 7, with tasks for items, transcripts for snapshots, a merge in progress, and no branch made by the worker |
| — | worker.md, 8: turns, new |
| 8. Below the domain | worker.md, 9 |
| 9. The world | worker.md, 10 |
| 10. Open questions | worker.md, 12; inbound acknowledgement settled (names); snapshots gone; saved-work branches in forge.md, 11; refusals on the channel in agent.md, 10; spend in authority.md, 7 |
| 11. Not built yet | worker.md, 11 (the layers below the domain, static git identities) and 12 (saving periodically, checks-only runs, a code graph, several workers in one world, repositories in parallel) |
| 12. Deferred conformance | worker.md, 13 |

### 6.3 `agent-domain.md`

| Section | Now |
|---|---|
| 1 to 3 | smith's `README.md`, 1 and 4, and `run.md`, 2; temper's place in agent.md, 1 to 3 |
| 4.1 Charter | smith's `run.md`, 3.1, with instructions, tools, a result contract and a budget in a unit; temper's charter in agent.md, section 4 |
| 4.2 What a run does | smith's `run.md`, section 4, with turns |
| 4.3 Inbound events and outlets | smith's `run.md`, 5.2 and section 6: host tools and messages; temper's in agent.md, 4.3 and section 5 |
| 4.4 Finishing | smith's `run.md`, sections 7 and 8, with a merge in progress; the push in agent.md, section 6 |
| 4.5 Lifetime | smith's `run.md`, section 6: waiting, parking, resuming |
| 5 to 8 | smith's `session.md`, `tools.md`, `run.md`, section 11 (facts), and `host.md`, section 3 (the channel) |
| — | smith's worlds, each document's; temper's in agent.md, section 9 |
| 9. Open questions | smith's `run.md`, 15, `session.md`, 11, `tools.md`, 8; agent.md, 10 |
| 10. Not built yet | smith's `session.md`, 8 (context management), `tools.md`, 8 (the commands' environment), `run.md`, 5.1 (MCP); agent.md, 11 |

### 6.4 `draft/core.md`

| Section | Now |
|---|---|
| 1. In one page | core.md, 1 |
| 2. temper and external systems | core.md, 7; forge.md, 4 |
| 3. The five primitives | core.md, 3; in depth in tasks.md, authority.md, connectors.md |
| 3.6 The store and the outbox | core.md, 4; engine.md, 5; connectors.md, 4 |
| 4. Plans and coordinators | core.md, 6 |
| 5. Communication | tasks.md, 7 |
| 6. The forge connector | forge.md |
| 7.1 A goal, as tasks | core.md, 9 |
| 7.2 Landing, step by step | forge.md, 15 |
| 8. What carries over | this section, and each document's "From today" |
| 9. Open questions | core.md, 11, and each document's |

### 6.5 This directory's `agent.md`, before smith

The agent's document of 2026-10-04, which code and plans cite as
`domain/agent.md`:

| Section | Now |
|---|---|
| 1. In one page | smith's `README.md`, 1; agent.md, 1 |
| 2. The agent in the system | agent.md, 2 |
| 3. Structure | smith's `README.md`, 4; agent.md, 3 |
| 4.1 Charter | smith's `run.md`, 3.1 and 3.2; agent.md, section 4 |
| 4.2 What a run does | smith's `run.md`, sections 4 and 10 |
| 4.3 Messages and the engine's tools | smith's `run.md`, 5.2 and section 6; agent.md, 4.3, section 5 and section 8 |
| 4.4 Finishing | smith's `run.md`, sections 7 and 8; agent.md, section 6 |
| 4.5 Waiting, parking, resuming | smith's `run.md`, section 6; agent.md, 4.6 |
| 5. Sessions | smith's `session.md` |
| 6. Tools | smith's `tools.md` |
| 7. Facts | smith's `run.md`, section 11 |
| 8. Below the domain | smith's documents, each; agent.md, section 3 |
| 9. The world | smith's documents, each; agent.md, section 9 |
| 10. Open questions | smith's `run.md`, section 15; agent.md, section 10 |
| 11. From today | smith's `run.md`, section 14; agent.md, section 11 |
