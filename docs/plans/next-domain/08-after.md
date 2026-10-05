# Step 08: after the cutover

Provisional, 2026-10-04. What the overlap kept and the design does not:
notes moved from the wiki to the store, now that one root uses them; the
shared crates contracted to what the new root speaks; and the work below
the domain that the design asks for, each part its own plan. Overview
and conventions: README.md.

## 1. Notes into the store

`temper-engine-domain-notes` is converted in place: only the new root
uses it now (README.md, 2.2). It keeps its indexes per scope in use,
evicted least recently used, its search of descriptions, its cuts to a
brief's budget and its bounds (`engine.md`, section 10). It changes:

- **its backing:** the wiki's list, fetch, create, edit and delete go;
  it follows the durable-state convention (README.md, 5.2), its entries
  `Stored` records, a scope's index loaded at first use and entries loaded
  on demand by `recall`;
- **its writes** are part of the decision that makes them, and a
  revision is refused as moved when the entry changed since the revision
  its run recalled, checked against the index it holds, which is exact
  since it is the store's only writer of notes;
- **its scopes and authors:** the deployment, a project, a repository, a
  goal; a person, or a task.

```rust
// notes, boundary.rs, after (sketch)
pub enum Event {
    Index { reply_to: ReplyTo, scopes: Scopes, budget: u32 },
    Search { reply_to: ReplyTo, scopes: Scopes, query: Box<[u8]>, most: u32 },
    Recall { reply_to: ReplyTo, recall: Recall },
    /// A note written by a run's tool or a person's request, authority having passed it.
    Note { reply_to: ReplyTo, scope: Scope, name: Box<[u8]>, revision: Option<u64>, change: Change, by: Author },
    Restore { record: Stored },
    Restored,
    Loaded { owner: Token, rows: Box<[Stored]>, more: bool },
}
```

The root adds it as a child, routes `note` and `recall` (`engine.md`,
7.3) and people's note requests (`people.md`, 5.1) to it, and gathers its
index into briefs; the engine world gains its story: a note written,
corrected by a person in the web, and recalled by a later run. Its own
world, `tests/engine/notes`, keeps its stories that still hold and loses
the wiki's.

## 2. Contractions

Each an increment through the gate, deletions with the renames the
overlap deferred (README.md, 3.3):

| Crate | What goes, or changes |
|---|---|
| `temper-channel` | `payload/v1.rs`; the second version renumbered the first, since nothing has shipped (`channel.md`, 4.5) |
| `temper-worker-protocol` | its first-version translations |
| `temper-worker-domain-host`, `-agent` | snapshots: `Assignment::snapshot`, `Finish::Parked`'s snapshot |
| `temper-worker-domain-agent` | replaced by `smith-host-domain` as the worker's agent child, once no run speaks the first version (05s-smith.md, 05s6) |
| `temper-worker-domain-checkout` | creating a missing base branch from the default branch (`worker.md`, section 11); `Op::Push`'s expected head no longer optional |
| `temper-engine-domain-fleet` | the graces at the hello no longer optional |
| `temper-engine-domain-brief` | the `Item`, `Comments`, `Dependencies` and `Plan` sources and kinds |
| `temper-engine-domain-views` | `Subject::Item` renamed `Task`, `Board` renamed `Project`, and their chunks |
| `testing/temper-fake-forge-domain` | its wiki; labels as anything but display; a push that names no expected head |
| `temper-forge-forgejo`, `testing/temper-fake-forge-protocol` | wiki pages, listing by label, label writes but for display, the wiki's webhook, the person in a key's marker (`forge.md`, section 19) |
| everywhere | the citations still naming deleted documents or sections |

## 3. Below the domain

The domain is complete after the cutover (programming-model.md, section
4): every story runs in worlds. What makes temper run outside them is
the protocol and io layers, which the design's README, section 5, revises
and `docs/design/protocol.md`, section 10, orders. Each is a plan of its
own, written from that revision, not here:

- **the store:** its protocol (temper's sized binary records, a version
  per shape, the codecs of the domain's `Record`s; ordered commits answered
  once durable; paged loads by key range; secret records written by the
  protocol layer alone) and its io (files through skein's whole-file
  operations and atomic writes, once skein has them);
- **the web:** HTTP and live streams on skein's server and SSE writer;
  signing in through the forge's OAuth, the sign-ins' tokens in the
  store's secret records; JSON both ways; rendering transcripts' turns;
- **the worker's agent link:** `smith-channel`'s host half over an agent
  process's pipes, spawning smith's binary in a contained process tree
  (`worker.md`, section 9);
- **what goes to skein,** as 05s-smith.md, section 6 lists, where a step
  has not taken it already;
- **credentials:** an LLM account's refresh token moved from
  its file record into the store's secret records, still
  written by the engine's protocol layer; the web's OAuth client; git
  identities, webhook secrets and API tokens per adopted repository and
  per forge (`credentials.md`, as the design's README revises it);
- **the forge's protocol:** the new calls of `forge.md`, section 17, with
  fixtures from step 00a's captures; repositories adopted at runtime, on
  several forges; target Forgejo v16.0.5, with job listing and bounded
  plaintext job-log API reads pinned to the failing job attempt, backed by
  v16 conformance exchanges (`domain/forge.md`, 20.1);
- **the engine's `iterate` and shell,** the first simulated world and the
  real loop (`docs/design/testing.md`, sections 2.3 and 2.4).

## 4. Done when

- notes live in the store, and the note's story passes in the engine
  world;
- no code speaks the first payload version, carries a snapshot, or makes
  a base branch on a worker;
- `rg 'wiki|snapshot' crates testing tests` finds only what the design
  keeps;
- the plans for the layers below the domain are written.
