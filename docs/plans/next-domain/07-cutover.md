# Step 07: the cutover

Provisional, 2026-10-04. The engine's protocol layer moved to the new
root, the system worlds moved with it, and the legacy engine deleted:
the one step that ends the overlap. Its increments each pass the gate;
only the third changes what a component speaks. Overview and
conventions: README.md.

## 1. What it does

- `temper-engine-protocol` translates the new root's vocabulary: runs by
  task and attempt with no packing, the second payload version, turns and
  their acknowledgements; its connections, credentials and OAuth carry
  over.
- It writes smith's charters: the brief and messages rendered, the
  engine's tools declared as smith's host tools, decoded and answered
  (`agent.md`, sections 4 and 5).
- The system worlds, `tests/agent/domain` and `tests/worker/domain`, run
  the new root, with the new engine world as their library, on the
  channel's second version, and smith's agent.
- The legacy crates and worlds of README.md, 4.1, and temper's legacy
  agent (05s-smith.md, section 3), are deleted, with the legacy
  translation in the engine's protocol layer.
- `docs/design/` describes what is built again.

## 2. Before it starts

Step 06 is done, and every legacy story is accounted for:

| Legacy engine world's story (current `engine-domain.md`, 14) | Now |
|---|---|
| a session's hello | a chat that answers (`tests/engine/domain`) |
| a chat that parks and resumes | the same, from its transcript |
| an issue handed in, its change failing CI, repaired, reviewed and landed | a small fix made in a chat and landed; a change failing CI, repaired, reviewed at its head and landed |
| a note a person corrects and a later run recalls | step 08 (notes into the store) |
| a plan proposed, decided, grown within its envelope or beyond, landed | a goal proposed, accepted, planned and done; delegation within authority, and a proposal beyond it (`tests/engine/tasks`) |
| a proposal rejected | a proposal rejected, the proposer hearing why (`tests/engine/tasks`, `tests/engine/people`) |
| a change whose CI never reports, stalled and held | the same (`tests/engine/forge`) |
| a tracking label taken off | goes: no label marks tracked work |
| a record garbled | goes: no record in comments |
| a run watched and stopped | a person stops a run and releases it (`tests/engine/domain`) |
| a supervisor woken once by a burst | a coordinator woken once by a burst (`tests/engine/tasks`) |
| an approval of an earlier head, which lands nothing | a gate at its exact head; a person's approval carried over a clean update and asked again after a repair (`tests/engine/forge`) |

| Legacy referee rule | Now |
|---|---|
| nothing lands on a protected branch without green CI on its exact head and a person's approval | the landing rules of `authority.md`, section 10, a person's approval one a project may add |
| writes only to the deployment's repositories | authority holds: no effect beyond the project's ceiling |
| keyed creations and outcomes made once, across a restart | keyed effects made once, across a restart at any cut |
| attempts only grow; one live run per item; dependencies done first | the same, per task |
| nothing of a plan made before a person accepts it | nothing beyond authority but by an accepted proposal |
| a call its grants allow is never refused as ungranted | every call decided once, against its task's authority |
| a person's message reaches a run, or its item ends | the same, per task |
| every story ends within a bound | the same |

The system worlds' own stories are mapped the same way in 07b's branch.

## 3. Increments

### 07a the engine's protocol translation, beside

In `temper-engine-protocol`, which depends on both roots for the length
of this step:

- move `translate.rs`, `payload.rs` and `names.rs` under a `legacy`
  module (`src/legacy.rs`, `src/legacy/*.rs`), a move only;
- write their successors at the top level against the new root:
  `names.rs` (a run is its task's number, an attempt its attempt number,
  both checked against the limits; no repository packed in), `link.rs`
  (the new root's worker-facing records to and from the channel's link
  messages, turns and their acknowledgements among them), `payload.rs`
  (the second version's charter, messages, calls, outcomes and turns to
  and from the new root's types);
- write temper's half of the agent there (`agent.md`, section 11), on
  `smith-channel` and `smith-transcript`: the root's charter encoded as
  smith's, the brief's typed sections and messages rendered as smith's
  text, the engine's tools and connectors' reads declared as host tools
  with their schemas, their calls decoded and their answers rendered;
  the brief's rendering and the verdict lists come from
  `temper-legacy-agent-protocol`'s temper half, ported;
- `tests/engine/protocol` gains the new translation's tests (every record
  both ways, every refusal of a peer's malformed name or payload), beside
  the legacy ones.

### 07b the system worlds move

- `tests/agent/domain` and `tests/worker/domain` depend on the new root,
  `temper-engine-domain-world` (the new engine world's store, people,
  scripted workers' names and referee) and `temper-engine-forge-world`
  (the connector world's translation), and on 07a's translation; their
  channel speaks the second version; each run is smith's agent, from
  smith by git, with smith's fake LLM;
- their people act on the web only; issues handed in become chats and
  goals; records an earlier life wrote become the store an earlier life
  committed;
- their stories are mapped as in section 2, and their referees hold
  `core.md`, section 10, as the engine world's does.

The legacy engine world still runs, alone, until 07d.

### 07c the engine's protocol layer switches

The listener, the connections, credentials and OAuth deliver the new
root's `Event`s and take its `Request`s, its LLM accounts' tokens
refreshed with `smith-oauth` (or skein's client, 05s-smith.md,
section 6) in place of `temper-oauth`; the `legacy` module and the
dependency on `temper-legacy-engine-domain` go; `tests/engine/protocol`'s
legacy tests go with them, and `tests/worker/protocol`, whose socket
worlds pair a worker with the engine's end of the link, moves to the new
root's records. From here the engine speaks only the second
payload version; workers and agents still speak both until step 08.

### 07d the legacy engine deleted

The five legacy crates and four legacy worlds, temper's legacy agent
with its providers, its fake LLM and its worlds, and `temper-oauth`;
their workspace members and dependency entries, and their lines in
`Cargo.lock`. Nothing else may
need to change: if something does, it was still using the legacy engine,
and that is fixed first in its own increment.

### 07e the documents

- delete `docs/design/engine-domain.md`, `worker-domain.md` and
  `agent-domain.md`; `docs/design/domain/` describes what is built;
- revise, per the design's README, section 5: `protocol.md` (the store
  and the web as boundaries, their order of building, runs by task),
  `channel.md` (the second version, as step 05 built it, the agent's hop
  now smith's; its section 14), `llm.md` (to smith's protocol design, but
  for the engine tools' half), `docs/design/forge.md` (what goes, the new calls, adoption at
  runtime), `testing.md` (sections 2.1, 4.5, 7 and 8: the new worlds, the
  fake store, the system worlds), `performance.md` (turns, transcripts, the
  working set of tasks), `docs/development/protocol-implementation.md`;
- repoint the citations of the deleted documents in kept code and tests
  to `domain/...` (`rg 'engine-domain.md|worker-domain.md|agent-domain.md'`
  finds none afterwards).

Markdown, but for the citations in code, which run the gate.

## 4. Done when

- `rg -i legacy crates tests testing` finds nothing, nor
  `rg 'temper-(agent|llm|oauth|fake-llm)'`;
- the system worlds pass on the new root with every story mapped;
- the two suites are within their budgets with the legacy worlds gone,
  and README.md, 5.5 records their new shares;
- `docs/design/` and the code agree on what is built.

## 5. Early, if it must be

If keeping the legacy engine ever means bending it rather than leaving
it alone (README.md, 2.2), this step comes forward: 07a to 07d as above,
with the system worlds' stories that the new root cannot tell yet left
out and listed in this document as owed, each to come back as the step
that makes it possible lands.
