# Status

Updated 2026-10-09. How far jig's code is from its design, and which
plans take it there. The designer keeps this file; a run's coordinator
changes only a plan's state (skein's development.md, section 3.3).

## 1. Plans

| Plan | State | Takes the design to | Where |
|---|---|---|---|
| jig extraction, sessions 00 to 21 | done 2026-10-08 | the crates of `domain/README.md`, section 2: the core and its eight children, the hub, the inline agent, the charter's translation; the test connector, the conformance kit and its referee; `ops`'s connectors, root and first stories. The children were first built in the host application's repository by its plans next-domain and domain-completion, then carved into jig with their history | the host application's repository; no plan document in jig |

No jig plan is active, ready or drafting: what is left is in sections 2
and 3. smith's reliability plan, active, does not edit jig.

## 2. The design

| Document | Built | Left |
|---|---|---|
| `README.md` | partly | the core, the rails (skein-lib's journal, the restart script, the conformance world, the reference root), connectors and hosts are built. Section 8, the store's encoding of jig's records; section 10, the client, whose `client/domain/` is not designed; section 12's documents below the domain: not planned |
| `domain/README.md` | yes | the crates of section 2 exist, and the gate checks the crate graph's rule; section 6 is open questions |
| `domain/core.md` | yes | the five primitives in the core and its children; section 11's promises are checked by the conformance referee; section 12 is open |
| `domain/tasks.md` | yes | `jig-core-tasks`: batches, lifecycle, holds, amending, cancelling and moving, messages, proposals and escalations, recurring and standing work; section 13 is open |
| `domain/authority.md` | yes | `jig-core-authority`: values and their order, budgets and periods, checks, proposals, requirements as verdicts; section 12 is open, the deployment's unit of spend among it |
| `domain/connectors.md` | yes | the vocabulary in `jig-core`, met by the test connector and `ops`'s two; section 16 is open |
| `domain/engine.md` | partly | `jig-core`: commits through the journal, the restart script, runs, transcripts and tools, the fleet, briefs, notes in the store, views, accounts. The store beneath it (5.5) is a fake in every world: not planned |
| `domain/root.md` | yes | the one shape, in the testing application's root and `ops`'s; `ops`'s is the reference root (section 12); section 14 is open |
| `domain/people.md` | partly | `jig-core-people`: persons and services, roles, requests, inboxes, chats, person tasks. Section 10, the client's protocol and signing in: not planned |
| `domain/hosts.md` | partly | `jig-host`, `jig-inline-agent` and `jig-charter`, with the hub's world; agent processes are composed by an application's worker root. Section 10 below the domain: not planned. 5.2: the inline agent moves to smith (decided 2026-10-09): not planned |
| `domain/testing.md` | partly | the children's worlds, the core's world, the conformance world and its referee, the fakes and kits. Section 7's store is jig's own ordered fake, not skein-kv's in-memory mode: not planned |
| `examples.md` | partly | `ops`'s two connectors and their worlds, its root, and through it the alert (7.1) and the night with no agent (7.3). The request (7.2) and the rest of 7.3, most told only in the connectors' worlds; its client (section 8) and a real backend (section 9): not planned |

## 3. Not planned

- **The store's encoding** (`README.md`, section 8; `domain/engine.md`,
  section 5): jig's records and keys encoded in key order and versioned,
  transcripts as files, the secret records. It waits for the design
  below the domain (`README.md`, section 12) and skein-kv.
- **Fakes on skein-kv** (`domain/testing.md`, section 7; `examples.md`,
  section 10): the worlds' store is jig's own ordered fake, not
  skein-kv's in-memory mode behind a fake. It waits for the store's
  encoding.
- **The client** (`README.md`, section 10; `domain/people.md`, section
  10): the client domain over the primitives, its wire, views and
  shells, the fake person, signing in. `client/domain/` is still to be
  designed; an application's client domain, built outside jig, is its
  likely start.
- **Hosts below the domain** (`domain/hosts.md`, section 10): the
  engine's channel with workers, agent processes in contained process
  trees, the inline agent's completions through skein's LLM client. It
  waits for the protocol layers.
- **smith's inline agent** (`domain/hosts.md`, 5.2 and section 12;
  smith's `host.md`, sections 9 and 11): decided on 2026-10-09, the
  inline agent is smith's, `smith-inline-agent`, typed, which smith's
  reliability plan (`~/src/rust/plans/reliability-plan/`, not
  versioned) builds with jig's as a reference. jig records the decision
  in `hosts.md`, then retires `jig-inline-agent` at a repin, with the
  rest of smith's and skein's changed APIs. It waits for that plan.
- **`ops` complete** (`examples.md`, sections 7 to 9): the request and
  the rest of 7.3 told through its root on the conformance world, its
  client and a real backend. The stories wait for a plan; the client
  and the backend for jig's client and protocol layers.
- **jig's own repository** (the top-level `README.md`, "Where jig is";
  `AGENTS.md`): the split, its workspace files, its dependants pinned.
  It waits for the boundary to settle.

## 4. Drafts

| Draft | About | Next |
|---|---|---|
| none | jig has no `draft/` directory yet | — |
