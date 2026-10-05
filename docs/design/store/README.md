# The store

Provisional, 2026-10-05. What temper's store is beneath the engine's
domain: skein's key-value store, embedded in the engine's process, holds
every record the domain keeps, and session transcripts are files beside
it. This document is the protocol and io side of what `domain/engine.md`,
section 5 assumes. The domain's half (commits, loads, what is kept) stays
there. The plan for building it is `plan.md`, in this directory. Read it
with skein's `docs/design/kv.md` (the store) and
`docs/plans/kv-implementation.md` (its plan), which this document cites as
kv.md and skein's plan. What is still open is in section 14.

## 1. In one page

- **skein-kv, in the engine's process.** The engine owns one skein-kv
  store, a `no_std` step machine on skein's loop. It keeps ordered byte
  keys and byte values in memory, made durable by an append-only log and
  snapshots (kv.md, sections 1 to 5). There is no store process, no
  network and no SQL. The engine is the store's only client, and the store
  is the engine's alone (`domain/engine.md`, section 2).
- **Everything in the map, except transcripts.** Every record the domain
  keeps is a key and a value in the map: projects, people, live and ended
  tasks, funding, proofs, archives, notes, the outbox and the connectors'
  state. A session's transcript, which is large, appended to and read
  rarely, is a file of its own beside the store, and the file is the only
  record of it (section 8).
- **A thin protocol layer between them.** `temper-engine-protocol-store`
  translates the root's store vocabulary, which already exists
  (`Commit`, `Load` and their terminals), into skein-kv's. It encodes
  keys so that bytes sort as the domain's keys do, and encodes records
  with a version per shape. It also writes and reads transcript files
  through skein-io. It holds no state the domain relies on.
- **One decision, one commit.** Each root commit is exactly one skein-kv
  commit with the same number. A turn's bytes are synced to its file
  before the commit that accepts the turn, so a turn is durable with its
  commit or not at all. Group commit folds bursts into one sync. A failed
  commit stops the engine, as the domain already says.
- **Secrets in a second store.** Refresh tokens, sign-in digests and the
  credentials of adopted systems are in a second skein-kv store, in a
  directory only the engine's user can read. The protocol layer alone
  writes it, and the domain names its records by number (section 9).
- **The real store in every world.** The worlds run the real protocol
  layer over skein-kv's in-memory mode, so they have no fake of the map.
  Only transcript files have a stand-in, since domain worlds do no io
  (section 12).
- **The domain keeps its shape.** Children keep their `Stored` records
  and ordered `Key`s (`domain/engine.md`, 5.6). Two domain changes come
  with this design. A transcript belongs to a session, not to an attempt.
  Transcript writes and reads join the root's store vocabulary
  (section 3).
- **Sized for years with retention.** By kv.md's estimate (section
  10.1), the map stays within 1–2 GB for a busy deployment when ended
  tasks shrink to summaries past a horizon. Transcripts grow by tens of
  gigabytes a year on disk, with a horizon of their own (section 11).

## 2. Where it sits

```
temper-engine-domain               the root: Commit, Load, Read; Committed, Loaded, Read; numbers its commits
        │ the root's store vocabulary (crates/temper-engine-domain/src/store.rs)
temper-engine-protocol-store       keys and records to bytes; ranges to intervals; transcript files; secrets
        │ skein-kv's vocabulary (skein's plan, 3.2)       │ io's file requests
skein-kv  ×2                       the store, and the secrets' store: map, log, snapshots
        │ io's file requests (skein's plan, 2.1)
skein-io                           files: the ring, or the simulator's disk
```

- **The engine's shell** runs both stores and the protocol layer on its
  loop, as it runs any machine, and routes their io to skein-io. Opening
  the stores comes before the root's `Start`.
- **On disk,** in the engine's data directory, which nothing else writes:

```
<data>/
├── store/          skein-kv: snapshot, log-<first number> segments
├── transcripts/    one file per session, sharded by task number
└── secrets/        a second skein-kv store: the directory 0o700, its files 0o600
```

- **Not here:** where `<data>` is and how the engine is told is
  configuration, which the design does not cover. Neither does a
  deployment of several engines (section 14).

## 3. The boundary with the domain

What the root already sends and hears (`domain/engine.md`, 5.1 to 5.6)
stays as it is:

| Root to store | Store to root |
|---|---|
| `Commit { number, writes }`: `Save(Record)`, `Erase(Key)` | `Committed { number }`, cumulative; `Uncommitted { number }` |
| `Load { owner, range, after, most, bytes }` | `Loaded { owner, rows, next }`; `Unloaded { owner }` |

This design adds what transcripts need. A transcript is not a record
loaded by key range: it is a sequence of turns read from either end.

| Root to store | Store to root |
|---|---|
| in a commit, `Append { session, turn }`: one accepted turn, at the end of its session's transcript | (the commit's terminal) |
| in a commit, `Drop { session }`: the transcript removed, past its horizon (section 11.2) | (the commit's terminal) |
| `Read { owner, session, from, toward, most, bytes }`: turns from the start, the end or a cursor, forwards or backwards | `Read { owner, turns, next }`; `Unread { owner }` |

- **A session** is one conversation with an LLM, as smith means it
  (smith's `session.md`): `(task, ordinal)`. A run that resumes appends
  to its task's current session, and a run that starts fresh opens the
  next one. The root keeps the current ordinal in the task's run proof,
  which each claim carries forward, and an ended task's record says how
  many it had. Today's turns are keyed by attempt
  (`Key::Turn { task, attempt, turn }`). That key cannot give a resumed
  run its whole conversation, which spans attempts, so it goes.
- **A turn** carries what `TurnRecord` carries today: its attempt, its
  number within the attempt, its cumulative spend, the last message it
  read, the wall time it was accepted, and its bytes (smith's turn,
  opaque).
- **The cursor** is opaque to the domain: a position the store gives
  and takes back. A read never goes past what is committed.
- **Reads see what is durable.** A `Load` or a `Read` sees every commit
  answered before it was sent, and nothing that is not durable, as
  skein-kv serves reads (kv.md, section 2).

What the domain relies on is unchanged (`domain/engine.md`, 5.5):
commits whole, in order and durable once answered; loads by key range in
bounded pages; versions per shape; no other writer.

## 4. Keys

### 4.1 The encoding

- **Bytes sort as the domain's keys do.** Every `Key` the root sends
  (its own and its children's, wrapped) is encoded so that byte order
  equals the order Rust derives for it: a variant's index, then its
  fields in order, numbers big-endian at fixed width, bytes escaped and
  terminated (skein's plan, 3.3). A property test holds the encoding to
  the derived order on generated keys.
- **A tag per owner, then per family.** The root's key enum already
  nests its children's (`Key::Tasks(tasks::Key)`, `Key::People(..)`).
  The encoding follows that nesting, so a child's keys form one interval,
  and each family one interval within it.
- **Keys never hold payload,** as today: numbers and small fixed
  identities only. Names (a note's name, a repository's path) are the
  exception, bounded by their limits.

### 4.2 Ranges to intervals

The domain's `Range` is a predicate over keys (`Range::contains`), and
today's fake store filters by it. The real store must read intervals, so:

- **each `Range` maps to a short, fixed, ordered list of intervals,**
  in the protocol layer, and a page that exhausts one with room left goes
  on into the next in the same step (skein-kv answers loads at once). A
  cursor is a key, so it works across intervals;
- **families loaded together are adjacent.** `Range::Tasks` (live tasks
  and the ledger) is two intervals today, because the tasks child declares
  `Live, Ended, Ledger, Closure`. Declaring the live families first makes
  every startup range one interval. This is a reordering in the domain,
  free while no store holds data (`plan.md`, S1);
- **a test checks each mapping** against `Range::contains` over generated
  keys: every key the predicate admits lies in an interval, and every key
  in an interval is admitted.

### 4.3 The families

What the store keeps, by owner. "At start" families are read by the
root's paged startup (`domain/engine.md`, section 6), and "on demand" ones
by a load when wanted. The rows marked *later* belong to routes not yet
built. Their keys are the children's to declare, under these rules.

| Owner | Family | Key | Loaded |
|---|---|---|---|
| protocol | the store's format | none: one row | at open, by the protocol layer |
| protocol | live sessions' committed lengths | task | at open, by the protocol layer |
| root | the deployment's header | none: one row | at start |
| root | run proofs, with each live task's current session | task | at start |
| root | terminal archive | task, attempt | on demand |
| root | escalation decisions | task, revision | on demand |
| tasks | live tasks | task | at start |
| tasks | funders' ledger | funder | at start |
| tasks | ended tasks | task | on demand |
| tasks | closed allotments | task, generation | on demand |
| tasks | *later:* history, archived messages, indexes for the web | task, then their own | on demand |
| people | people, sign-ins, roles, answered keys | as `people::Key` | at start |
| people | *later:* read positions over results and replies | person | at start |
| notes | *later:* entries, and an index row per entry | scope, name | per scope in use; entries on demand |
| forge | *later:* outbox entries, owned objects by key, repositories' state, procedures' states, projections' digests | the connector's | live ones at start; the rest on demand |
| accounts | *later:* accounts' state, no token | account | at start |

- **Live and history apart.** Within each child, families read at start
  come first and history after them, so a restart reads what is live
  without touching history (`domain/engine.md`, 5.5).
- **Indexes are records.** skein-kv offers no secondary index (kv.md,
  section 1). A view the web pages through, such as a project's ended
  goals newest first or a person's closed chats, is a family of small
  rows the child saves and erases in the same decision as the records
  they point at. Keys reversed in time (`rev_u64`) give newest first. Each
  index is the domain's own, so it is tested in the child's world.
- **No traces.** What runs report (the views child's traces of text,
  progress, calls, tools and usage) is not stored. With every turn
  committed, its transcript already holds the text, the calls, their
  results and the usage. Live views still stream what runs report
  (`domain/engine.md`, section 11), and the engine's own diagnostics go
  to the process's logs, outside the store (section 11.2).

## 5. Records

- **A codec per shape.** Each `Record` variant, and each child's
  `Stored` variant inside it, has a binary encoding in the protocol layer.
  The encoding starts with its shape's tag and version, and its fields
  follow in a fixed order, lengths before bytes. There is no
  self-describing format and no reflection: the codecs are written out,
  as the programming model's subset asks.
- **Versions are read, then upgraded.** A decoder accepts every version
  of its shape that a release has written, and upgrades it to the
  current shape as it decodes. The domain only ever sees the current
  shape, and writes always use it. A version stays readable until a
  release has rewritten every row of it (section 14).
- **The format row** records the store's format: the key encoding's and
  the codecs' generation, and the deployment's id. A store written by a
  newer format refuses to open, saying so. One whose deployment id differs
  from the header refuses too.
- **Sizes are derived, then checked.** The protocol layer computes, from
  the domain's limits, the largest encoded key, record and commit. It
  refuses to start when skein-kv's limits are smaller (its `key`,
  `value`, `ops` and `commit`; skein's plan, 3.4), so a commit the root
  admitted is never refused as too large.
- **Golden bytes.** Each shape's every version has a fixture of its
  bytes. A change to an encoding that breaks one fails the gate, and a
  deliberate change adds a version. A fuzz target decodes arbitrary bytes
  for each shape.

## 6. Commits

- **Numbers are shared.** The root numbers its commits, and skein-kv
  numbers its own contiguously from one, across restarts. Every root
  commit is one skein-kv commit, and nothing else writes the main store,
  so the numbers are equal. The protocol layer checks this as it submits
  and stops on a mismatch. At start, skein-kv's recovered `last` must
  equal the commit the deployment's header records, which every writing
  decision saves (`store.rs`, `Deployment`). A store that disagrees
  refuses the start.
- **A commit's writes become skein-kv ops** in order: a `Save` a `Put`
  of its encoded key and record, and an `Erase` an `Erase`. An `Append`
  becomes a write to its session's file, made before the commit is
  submitted (section 8.2), and a put of that session's new committed
  length. A `Drop` becomes the file's removal, made before the commit is
  submitted (section 8.5).
- **In order, whatever waits.** A commit with appends or drops goes to
  skein-kv once its files are synced, and commits after it queue behind
  it, so numbers reach skein-kv in order. The wait is one file sync, and
  only for commits behind a turn or a drop.
- **Group commit.** While a sync is in flight, commits queue, and the
  next write carries them all (kv.md, section 4). The root's bound on
  commits in flight is checked at start against skein-kv's queue
  (`queued`, `queued_bytes`), so the store never answers a commit busy.
- **`Committed { number }`** goes up when skein-kv answers that commit,
  which makes every earlier one durable too, as the root already reads
  it.
- **A failure stops the engine.** skein-kv answers `Failed` for every
  commit in flight when a write or a sync fails or stalls (kv.md,
  section 4), and a failed transcript write or sync fails its commit the
  same way. The protocol layer answers `Uncommitted` for each, the root
  stops (`domain/engine.md`, 5.1), and the shell ends the process. The
  next process recovers the store as it opens it. The engine never
  carries on over a store that has recovered beneath it.
- **A refusal is a failure.** Past the entrance checks above, a commit
  skein-kv refuses (too large, the map full) would be a decision already
  made and not kept, so it stops the engine as a failure does. Section
  11.3 keeps the map from filling.

## 7. Loads

- **Same step.** skein-kv serves a page from memory in the step that
  asks (skein's plan, 3.2). The protocol layer answers the root's `Load`
  in that step too, through the root's ready list, never by recursion.
- **The cursor is exclusive.** The root's `after` is the last key it
  has. The interval starts just past its encoding, and an absent `after`
  starts at the interval's beginning.
- **Two byte bounds.** skein-kv bounds a page by encoded bytes, and the
  root by decoded bytes (`Load::bytes`, measured as `record_bytes` does).
  The protocol layer asks skein-kv for at most `most` rows, decodes them
  in order, and keeps the longest prefix that fits the root's bound. The
  cursor is then the last key kept, as `domain/engine.md`, 5.3 says. A
  row that does not decode is a failed load (`Unloaded`), and the root's
  start refuses rather than skip it.

## 8. Transcripts

### 8.1 A file per session

- **One file per session,** named by `(task, ordinal)`, in
  `transcripts/`, sharded by task number so no directory holds more than
  a few thousand files. The name is derived, so nothing lists or indexes
  the files: a task's sessions are its ordinals, from one to the count
  its run proof or ended record keeps.
- **A turn per frame:** the turn's attempt, number, spend, read fence
  and wall time, then smith's bytes, with a checksum, and the frame's
  length at both ends so that the file can be walked from either end.
- **No row per transcript.** The map keeps one fact for each *live*
  session: how much of its file is committed. That is one row per live
  task, erased as the task ends, so it is bounded by live work and never
  grows with history. A closed session needs nothing in the map (8.4).

### 8.2 Written before its commit

The root commits a turn with its spend, its proof and the messages it
read, in one decision, and acknowledges the worker once that commit is
durable (`domain/engine.md`, 7.2). For a commit that appends:

1. the protocol layer writes each turn at its session's end. The end
   includes appends queued ahead that are not yet durable, since a
   session's next turn may be decided before its last one commits;
2. it syncs each file it wrote, and, for a session's first turn, the
   directory that holds the new file;
3. it submits the commit to skein-kv, with each session's new committed
   length among its ops;
4. skein-kv answers once the commit is durable, and the root hears it.

A crash after step 2 and before the commit is durable leaves a turn in
the file past its committed length. The worker was not acknowledged, so
it sends the turn again, and the next append writes over the old bytes.

### 8.3 Read back

- **Resuming:** a run that resumes is given its session whole, within
  the resume limit (`domain/engine.md`, 7.2). The root reads it
  forwards from the start, a page of turns at a time, while the task
  prepares.
- **A tail:** a fresh run's brief carries the end of its task's last
  session, read backwards from the end within the section's budget. The
  web reads the same way, newest first, a page at a time.
- **Never past what is committed:** a live session is read up to its
  committed length, and a closed one to its end.
- **A turn that fails its checksum** fails the read (`Unread`). The root
  then treats the transcript as one the agent cannot use: the run fails
  as transient and the next starts fresh (`domain/engine.md`, 7.2).
- **Open files are bounded:** the protocol layer keeps a bounded set
  open, the live sessions' first, and opens others to read them.

### 8.4 After a crash, and closing

- **At open,** before the root starts, the protocol layer reads the
  live sessions' committed lengths and cuts each session's file to its
  length, syncing it. Only a crash leaves bytes past a committed length,
  since a failed commit stops the engine. After the cut, every file holds
  exactly its committed turns.
- **A session closes** when its task's next run starts fresh, or when
  the task ends. That happens in a running engine, after the cut, so a
  closed session's file is exactly its turns and needs no record. The
  protocol layer replaces a task's length row when an append names the
  task's next session, and erases it in the commit that erases the task's
  run proof, which is the commit that ends the task.

### 8.5 Dropped

A `Drop` in a commit names a closed session past its horizon. The
protocol layer removes its file and syncs the directory before it submits
the commit. A crash before the commit leaves a task whose session is gone
while its records still count it. A read of it finds no file and answers
as for a dropped transcript, and retention's next pass, which still finds
the task, commits the drop again. Removing a missing file is not a
failure. Retention is the only writer of drops (section 11.2).

## 9. Secrets

- **A second skein-kv store,** in `secrets/`, created `0o700` with its
  files `0o600` (kv.md, section 10). The main store, its snapshots and
  any copy of it hold no secret.
- **What it keeps:**
  - LLM accounts' refresh tokens, by account and generation, moved from
    temper-oauth's file record (`credentials.md`, as the domain's README,
    section 5 revises it);
  - sign-ins' digests, by sign-in number, with their expiry;
  - the credentials of adopted systems (a forge's API token, a
    repository's webhook secret), by the number the domain gives each,
    once adoption at runtime is built.
- **Written by the protocol layer alone.** The domain names a secret by
  number and generation and never holds one (`domain/engine.md`, 5.4).
  The secrets' store has its own numbers, and the root never sees them.
- **A secret first, then the decision.** A rotated refresh token is
  durable before the access token it bought is granted. A sign-in's digest
  is durable before the root hears `SignedIn`, and the person's cookie is
  set only after the root's commit. A crash between the two leaves an
  orphan: a digest no sign-in names, or a credential no adoption names.
  An orphan is never presented or used, so it does no harm. Orphans expire
  with their time, and a sweep removes the rest (section 14).
- **Small:** a budget of a few megabytes, its own limits, its own worst
  case.

## 10. Opening and restart

1. **The shell opens both stores.** skein-kv recovers each: the
   snapshot, then the log's longest valid prefix (kv.md, section 3).
   Each answers `Opened { last }`.
2. **The protocol layer reads its own rows:** the format row, refusing
   a newer format or another deployment's store, and the live sessions'
   committed lengths, cutting each session's file to its length (8.4).
   An empty store gets its format row in the root's first commit, with the
   header.
3. **The root starts** (`domain/engine.md`, section 6): it pages the
   header, checks `last` against it (section 6), then pages the live
   families. Its paging is unchanged, and so is everything after it.
4. **The protocol layer serves secrets** to the engine's other protocol
   machines (accounts, the web) once its store is open.

A restart reads a snapshot of a few hundred megabytes in about a second
(kv.md, 10.1), and the live families are a small part of it. The cut
reads and writes only live sessions' files.

## 11. Growth and retention

### 11.1 How large

kv.md, 10.1 sizes a busy deployment: about 300 tasks a day, 5–15 KB of
map per task, 0.2–1 MB of transcript per agent run. Without retention the
map grows by 0.5–1.7 GB a year, which is too much for memory over years.
Transcripts grow by 20–40 GB a year on disk. Three to five times that
covers connectors beyond the forge.

### 11.2 Retention

Retention is temper's (kv.md, section 10), a core mechanism of the
engine. It is not the store's, so it is tested in the worlds:

- **Ended tasks shrink past a horizon** (30 days by default). A pass
  the root runs on a timer pages an index of ended tasks by end time. For
  each task past the horizon, one commit erases its full rows (its record,
  closures, terminal archive, history, archived messages) and saves a
  summary of about a kilobyte: its spec's first line, its result's words
  cut, its ending, its requester and its numbers. What the web shows
  of old work comes from summaries.
- **Transcripts go past a longer horizon** (180 days by default), by
  `Drop`, in the same pass. A chat that is still live keeps its
  sessions, whatever their age.
- **Answered keys** expire after the people child's retention, in the
  same way.
- **Diagnostics are not the store's.** The engine's own trace records
  (programming-model.md, section 3: diagnostics are data) go to the
  process's logs through its shell, with whatever retention those keep.

### 11.3 Keeping the map from filling

- **The store reports its fill** to the root with each commit's answer,
  as bytes of the budget.
- **Above a high-water mark** (90% by default) the root refuses new
  work at its entrances: new tasks and chats, new sign-ins. Existing work
  goes on, and a refusal is never a failed commit. The web tells owners
  why and how full the store is.
- **The budget is configured** (2 GB by default) and part of the
  engine's worst case. A store that needs more is a sign that the
  horizon should be shorter first. After that, values move to disk with
  only keys in memory (skein's plan, section 9).

## 12. Testing

Following `testing-strategy.md` (section 4: a fake of the service's own
component retires once the real one exists) and `docs/design/testing.md`:

- **The real store in the domain worlds.** The engine's worlds run the
  protocol layer over skein-kv's in-memory mode (skein's plan, 5.1:
  commits apply at once, no files), in place of today's fake store. The
  worlds stay free of io, and every story runs the real key encoding,
  codecs and paging.
- **Faults come from the world, not the store.** The world holds
  commits between the root and the store, in order. It delays them to
  make the store slow, and answers one `Uncommitted` to fail it. A cut
  keeps what reached skein-kv and loses what the world still held, so a
  restart over the same store starts from exactly the commits kept,
  whether or not their answers went out.
- **Transcript files have a stand-in** in the domain worlds: an
  in-memory file system serving the protocol layer's file requests, with
  the same cut rule. The real files are tested in the store's world.
- **The store's world,** `tests/engine/store`, runs the protocol layer
  over skein-kv and transcript files on the simulator's crashing disk
  (skein's plan, 2.2). A scripted root commits, loads, appends turns and
  reads them back, and is crashed at every file operation and sync. Its
  referee holds the domain's promises:
  - after recovery, the last acknowledged commit, or that and one whole
    uncertain one, never part of one;
  - no turn readable past its commit, and none missing before it;
  - numbers that match the header;
  - pages equal to a model map's.
- **Step tests** in the protocol crate: the key encoding against the
  derived order; ranges against `Range::contains`; every codec both ways,
  with golden bytes per version; refusals at open (format, deployment,
  limits, numbers).
- **Fuzz targets** for each shape's decoder and for a turn frame's.
- **On the real ring,** the engine killed around each phase of a turn's
  commit and reopened, and restart time measured on a store of the
  estimated size. As skein's plan says (section 7), a killed process
  keeps the page cache, so this checks integration, not power loss.

## 13. What the other documents now owe

- **`domain/engine.md`:** 5.4, transcripts by session and kept as
  files, secrets in their own store, traces no longer kept; 5.5,
  transcripts read from either end; 7.2, sessions and resuming the current
  one; 11, traces gone, views live only; 14, the store's layer named here,
  and the fake store retired; 15, the world's store.
- **`domain/README.md`, section 5:** the store's protocol is this
  directory, and `credentials.md`'s refresh token moves to the secrets'
  store.
- **`docs/design/protocol.md`:** the store as a boundary of the engine's
  protocol layer, with this document as its design.
- **`docs/design/testing.md`:** the real store in the engine's worlds,
  the transcripts' stand-in, and the store's world.
- **`docs/plans/next-domain/08-after.md`:** section 2, the views child
  loses its traces and their sweep; section 3, the store's plan is
  `plan.md` here.
- **skein's plan:** skein-kv's in-memory mode (5.1) is used by temper's
  worlds, so it stays when the files arrive; skein-io's file entity gains
  opening an existing file to write (its 2.3, needed now for transcripts)
  and cutting a file to a length; the fill with each `Committed`. temper
  does not need skein-kv's payload logs (its section 6).

## 14. Open questions

- **Rewriting old versions:** when a release may drop a shape's old
  decoder. A startup pass that rewrites every row of an old version is
  the likely way, run when the format row says one is due.
- **Backups:** copying the directory while the engine is stopped works.
  An online backup would copy the snapshot, then the segments from its
  start, then the transcripts. That would rest on recovery's rule, if
  snapshots did not delete segments while it ran and live sessions were
  cut at restore.
- **Sweeping orphan secrets:** a pass that lists the secrets' store and
  asks the domain which numbers it still names, or secrets that carry
  their own expiry only.
- **Retention's horizons,** the summary's shape, and whether people may
  pin a task or a transcript past them.
- **Run timings:** if what traces kept beyond the transcript (when a
  tool or an LLM call started and how long it took) is wanted after the
  run, it goes into the turn's frame or a file beside the transcripts,
  never into the map.
- **Search** over tasks, notes and transcripts (`docs/design/web/ux`):
  the store offers ordered keys only. Notes' descriptions are searched in
  memory per scope (`domain/engine.md`, section 10). Wider search would
  be an index of the domain's, or a file-based index beside the store.
- **Several engines:** the store is one process's. If a deployment ever
  needs more than one engine, the log of commits is what would be
  replicated.
