# Status

Updated 2026-10-09. How far temper's code is from its design, and which
plans take it there. The designer keeps this file (skein's
development.md, section 3.3).

## 1. Plans

| Plan | State | Takes the design to | Where |
|---|---|---|---|
| next-domain | done 2026-10-07; step 08 ready | `domain/` from the legacy engine: authority, tasks, people, the forge connector, the runtime, the root, the agent moved to smith. Step 07's protocol cutover was voided on 2026-10-07 and the legacy code deleted. Left in step 08: the contractions of the checkout and the fake forge, whose wiki remains (section 2), and the plans below the domain (section 3) | `docs/plans/next-domain/` |
| domain-completion-plan | done 2026-10-07 | next-domain's steps 01 to 07 at the domain layer: runs, delegation, control, funding and procedures, people, the forge's children and top, restarts, the typed system world, gates, owner requests, the strict subset; legacy and protocol code deleted | `~/src/rust/plans/domain-completion-plan/` (not versioned) |
| jig extraction | done 2026-10-08; temper 05 ready | `draft/jig.md`, sections 4 and 5: the core and its children, the hub and the inline agent carved into `jig/`; temper's root on `jig-core`, the forge on jig's connector vocabulary, the worker on `jig-host`. Of temper 05, the root in jig's reference shape is merged (`b0ab9ad4`); temper's scenarios on jig's conformance world are not | no plan document found; its record is `docs/plans/next-domain/STATUS.md`, "jig extraction" |
| web-implementation | done 2026-10-06 to W4; W5 to W10 ready | `web/` in `temper-web-domain` and `temper-web-view`. W1 to W4 are built: the two spines, the person and the client's world, the first slice. W5 to W10 are left: the joint world, inbox, conversation, task page, board, forge panels. Written before jig's client (jig's `README.md`, section 10) | `docs/plans/web-implementation/` |
| store plan | ready | `store/README.md` on skein-kv, S1 to S9. Written before jig's store part (jig's `README.md`, section 8) | `docs/design/store/plan.md` |

No plan is active. smith's reliability plan, active, edits neither
temper nor jig.

## 2. The design

| Document | Built | Left |
|---|---|---|
| `agent-domain.md` | no | describes the legacy agent, deleted on 2026-10-07; superseded by smith and `domain/agent.md`; to be removed: not planned |
| `channel.md` | no | `temper-channel` was deleted on 2026-10-07; skein's channel supersedes it, and the engine's channel is jig's (jig's `hosts.md`, section 10): not planned |
| `credentials.md` | no | `temper-oauth` was deleted on 2026-10-07; skein's OAuth client supersedes it; secrets move to the store (`domain/README.md`, section 5): not planned |
| `engine-domain.md` | no | the legacy engine, deleted on 2026-10-07 and superseded by `domain/`; still cited by the fake forge and the checkout; to be removed: not planned |
| `forge.md` | no | `temper-forge-forgejo` was deleted on 2026-10-07; the Forgejo v16.0.5 protocol with the calls of `domain/forge.md`, section 17: not planned |
| `llm.md` | partly | providers moved to smith and skein's LLM client; of temper's half, the engine tools' schemas and decoding are in `temper-engine-smith`, rendering in a protocol layer is not built: not planned |
| `performance.md` | no | section 3's doubling buffer and `ByteRing` (no `ByteRing` in skein-lib); sections 4 to 6 wait for the protocol layer and a shell: not planned |
| `protocol.md` | no | the protocol layer, deleted on 2026-10-07, with the revisions of `domain/README.md`, section 5: not planned |
| `testing.md` | partly | the domain and system worlds stand; sections 7 and 8 still describe deleted protocol and legacy worlds; section 9's tiers: not planned; temper's scenarios on jig's conformance world: jig extraction, temper 05 |
| `worker-domain.md` | no | the worker before `jig-host`, superseded by `domain/worker.md` and jig's `hosts.md`; still cited by the worker's crates and worlds; to be removed: not planned |
| `domain/README.md` | partly | the names and maps stand; section 5's revisions of the protocol documents are not made: not planned |
| `domain/core.md` | yes | built in `jig-core` and its children under temper's root; to become temper as an application (`draft/jig.md`, section 6): not planned |
| `domain/tasks.md` | yes | built as `jig-core-tasks`; the document still names temper's crates; to move to jig's `tasks.md` (`draft/jig.md`, section 6): not planned |
| `domain/authority.md` | yes | built as `jig-core-authority`, landing rules translated to the forge's requirements; the same move: not planned |
| `domain/connectors.md` | partly | the contract is in `jig-core` and the forge meets it; section 14's second connector, test environments: not planned |
| `domain/engine.md` | partly | the root in jig's reference shape on `jig-core` and skein-lib's journal, notes in the store; section 14 below the domain and the configuration: not planned |
| `domain/forge.md` | partly | the connector's four crates on the fake forge; sections 13 and 14 beyond the `Fork` role and participation news, and section 17's protocol: not planned |
| `domain/people.md` | partly | `jig-core-people` and the root's routes; section 11, the web and signing in through the forge: not planned |
| `domain/agent.md` | partly | the typed translation to smith (`temper-engine-smith`, `jig-charter`), the worker hosting smith, the system world; section 9's protocol tests and the charter's encoding: not planned |
| `domain/worker.md` | partly | the worker root on `jig-host`, the checkout and smith's host domain, with whole-worker worlds; section 3 still names `temper-worker-domain-host`; section 11's contraction (a missing base branch created, a push with no expected head): next-domain, step 08; section 9 below the domain: not planned |
| `store/` | no | the store over skein-kv: store plan; turns are still records keyed per turn, so even its S1 is not built |
| `web/` | partly | W1 to W4 built; W5 to W10: web-implementation; the wire, the protocol layer and the shells: not planned |

## 3. Not planned

- **The protocol and io layers** (`protocol.md`, `channel.md`,
  `credentials.md`, `forge.md`; `domain/engine.md`, section 14): temper
  below the domain, deleted on 2026-10-07 to be planned afresh on
  skein's channel, OAuth client, HTTP and JSON. next-domain's step 08,
  section 3 asks for their plans; they wait for jig's layers below the
  domain (jig's `README.md`, section 12), which temper's compose.
- **The tiers below the domain** (`testing.md`, section 9): protocol
  worlds, simulated worlds and the real loop. They wait for the protocol
  layers and a shell.
- **Configuration** (`domain/engine.md`, section 14): rules, connectors'
  addresses and tokens, LLM endpoints and prices, charters, seeded
  projects. It waits for the protocol layer and `main`.
- **Documents of deleted code** (`engine-domain.md`, `agent-domain.md`,
  `worker-domain.md`): to be removed. They wait for the code citing them
  to be repointed to `domain/` and jig's documents.
- **temper's `domain/` as an application of jig** (`draft/jig.md`,
  section 6): `core.md`, `tasks.md`, `authority.md`, `engine.md`,
  `people.md`, `worker.md` and `agent.md` still describe crates that are
  now jig's. Each keeps temper's side and cites jig's for the rest. It
  waits for jig's boundary to settle.
- **A second connector** (`domain/connectors.md`, section 14): test
  environments. It waits for continuous QA or the factory to be adopted
  (`draft/qa.md`, `draft/factory.md`).
- **Participating objects and upstream contributions**
  (`domain/forge.md`, sections 13 and 14): outside contributions
  triaged and reviewed, changes landed by others' merges, submissions
  from a fork. Not needed first; they wait for a project that wants them.
- **Performance's fixes** (`performance.md`, sections 3 to 6): two
  skein-lib containers before the first machine that joins pieces, copy
  accounting with the protocol layer, allocator tuning once a shell runs.
- **smith's and skein's changed APIs** (smith's `host.md`, section 11):
  the typed agent face, smith's inline agent, removed modes. temper
  repins after smith's reliability plan
  (`~/src/rust/plans/reliability-plan/`, not versioned), which never
  edits temper.

## 4. Drafts

| Draft | About | Next |
|---|---|---|
| `core.md` | the generic engine on five primitives, of 2026-10-04 | superseded by `domain/`, which maps each of its sections (`domain/README.md`, 6.4) and says it is removed: remove |
| `factory.md` | the software factory: write, prove and run planes, the lab, on jig and os | not adopted: discuss; its section 13 changes `qa.md` |
| `inline-agent.md` | who owns the engine's inline agent | decided 2026-10-09: smith (option 1); smith's reliability plan builds `smith-inline-agent`. Record it in jig's `hosts.md`, 5.2 and section 12; jig retires `jig-inline-agent` at a repin once smith's lands; then remove |
| `jig.md` | temper on jig: building jig here, what goes, what stays, the move | mostly carried out by the jig extraction: bring sections 4, 5 and 8 up to date; the move (section 7) waits for the boundary to settle; then remove, as its section 7 says |
| `one-host.md` | one hub for jig's runs, wherever their agents run | adopted 2026-10-07 and built (`jig-host`, `jig-inline-agent`, the local host retired); its open questions are in jig's `hosts.md`, section 12: remove |
| `qa.md` | continuous QA on a running build of main | not adopted: discuss; its order of building (section 8) waits for a running build |
