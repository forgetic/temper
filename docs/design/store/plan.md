# Building the store

Provisional, 2026-10-05. The plan for building the store of
`README.md` in this directory, in increments, each a branch through
temper's gate (`docs/development/workflow.md`). Some increments wait for
skein's: `skein-kv`'s plan (skein's `docs/plans/kv-implementation.md`,
here "skein's plan") builds the store itself, and this plan builds what
temper puts on it. The domain's half moves first, since it needs nothing
from skein.

## 1. In one page

- **Two tracks, joined late.** The domain track (S1) changes the root's
  store vocabulary and the fake store, and runs in the worlds as they are.
  The protocol track (S2 to S6) builds `temper-engine-protocol-store`,
  first over skein-kv's in-memory mode, then over its log, then with
  payload logs. The shell (S7) joins them in the engine's process.
- **Each step usable alone.** Every increment leaves the gate green
  and adds a world or a test that holds what it built. Nothing waits on a
  later increment to be tested.
- **What temper asks of skein** is listed once (section 3) and reaches
  skein's plan before its section 6 is built.

```
skein:   K1 memory ──► K2 log ──► K3 snapshots ──► K4 bounds ──► K6 payload logs (temper's asks)
          │              │                                          │
temper:  S1 domain ─────────────────────────────────────────────────┼──────────► S8 conformance
         S2 codecs ──► S3 store machine ──► S4 durable ──► S5 transcripts ──► S7 shell
                                              └──► S6 secrets ─────────────────┘
         S9 indexes, retention, fill: after S1, any time before the map nears half its budget
```

`K1` to `K6` are skein's plan's 5.1 to 5.4 and section 6, after its
prerequisites (5.0: io's file entity and the crashing disk).

## 2. The crate

```
crates/temper-engine-protocol-store/
├── Cargo.toml          skein-lib, skein-kv (its vocabulary and key writer), temper-engine-domain (its store vocabulary)
└── src/
    ├── lib.rs          the crate's doc: what it translates, what it never decides; re-exports
    ├── keys.rs         every domain Key to bytes and back, in the derived order (README.md, 4.1)
    ├── ranges.rs       every domain Range to its intervals (README.md, 4.2)
    ├── codec.rs        a bounded byte writer and reader, versions, the shape tags
    ├── records/        a codec per shape: root.rs, tasks.rs, people.rs, then each child that keeps records
    ├── turns.rs        a turn's frame inside a payload log (README.md, 8.1)
    ├── format.rs       the format row; refusals at open
    ├── limits.rs       the largest key, record and commit derived from the domain's limits; checks against skein-kv's
    ├── store.rs        the machine: the root's requests down, skein-kv's events up (README.md, sections 6 to 8)
    ├── secrets.rs      the secrets' store's vocabulary and machine (README.md, section 9)
    ├── trace.rs        trace records, as data
    ├── tests.rs
    └── tests/          step tests per area; golden/ holds each shape's bytes per version
```

It is `no_std` and a step machine with the shape of skein-kv's own:
`down` takes the root's requests, `up` takes skein-kv's events, and each
emits events above and requests below, within a declared `MAX_OUT`
(programming-model.md, sections 3 and 4).

## 3. What temper asks of skein-kv

For skein's plan, section 6, and kv.md, section 6, before they are
built. Each comes from `README.md`:

1. **Appends are ops of a commit** (`Op::Append { log, bytes }`), written
   at the log's end, counting appends queued ahead, synced before the
   commit's frame, and recorded in it as the log's new extent
   (README.md, 8.2). `Op::Drop { log }` as sketched.
2. **Frames readable from either end:** a frame's length at both ends,
   and `ReadLog { log, from, toward, max }` reading whole frames from the
   start, the end or a cursor, never past the extent.
3. **Many logs:** hundreds of thousands, in sharded directories, with a
   bounded set held open, opened again to read.
4. **The fill** (the map's bytes against its budget) with each
   `Committed`, or readable at any step.
5. **A second store in a process,** in a root of its own, created
   `0o700`, its files `0o600` (skein's plan, 2.1, `Create`'s mode). This
   is already planned and is listed here to confirm it.

## 4. Increments

### S1. Sessions in the domain

**What:** the root's store vocabulary takes transcripts by session
(README.md, section 3), with no store below it but the fake.

- `Write::Append { session, turn }` and `Write::Drop { session }`;
  `Request::Read` and its terminals `Read` and `Unread`; `Key::Turn`,
  `Record::Turn` and `Range::Turns` go.
- The run proof carries the task's current session ordinal. A claim that
  resumes keeps it, and a fresh one advances it. An ended task's record
  says how many sessions it had.
- `Committed` carries the store's fill (unused until S9).
- The tasks child declares its live families first (`Live, Ledger`, then
  `Ended, Closure`), and so do people's. No data exists yet, so this costs
  nothing.
- The fake store (`tests/engine/domain/src/commits.rs`) gains sessions,
  appends inside commits, reads from either end with an opaque cursor,
  and drops. A cut keeps appends with their commit or loses both.
- `domain/engine.md` 5.4, 5.5 and 7.2 are revised with it (README.md,
  section 13).

**Tests:** the engine's world's existing stories pass, with turns
committed into sessions. A new story covers a chat that parks and resumes
across two attempts and is given both attempts' turns. The referee adds a
check: no turn is visible before its commit, and every committed turn is
visible after a cut.

**Depends on:** nothing.

### S2. Keys and codecs

**What:** the crate's `keys.rs`, `ranges.rs`, `codec.rs`, `records/`,
`turns.rs`, `format.rs` and `limits.rs`, for every key, range and record
the root has after S1.

- Keys use skein-kv's key writer (skein's plan, 3.3). If S2 starts
  before K1 lands, it uses a copy of that writer's encoding, kept until
  K1 lands.
- Every shape's codec at version 1, and the decoders' upgrade hook, unused
  until a version 2.
- `limits.rs` derives the largest encodings from the domain's `Limits`.

**Tests:**
- step tests: the key encoding against the derived `Ord`, over generated
  keys of every variant;
- every `Range` against `Range::contains`, over generated keys;
- every codec both ways, with golden bytes per shape;
- the derived limits: the largest records the limits allow, encoded,
  never past them.

Fuzz targets: each decoder.

**Depends on:** S1 for the transcript shapes. The rest can start at
once.

### S3. The store machine, in memory

**What:** `store.rs` over skein-kv's in-memory mode (K1: commits apply
at once, no files), covering every rule of README.md, sections 6 and 7
that does not need a disk:

- commits translated op by op;
- numbers checked against skein-kv's, and against the header at start;
- loads paged across intervals, with both byte bounds and the exclusive
  cursor;
- `Uncommitted` for every commit in flight on `Failed`;
- refusals at open: format, deployment, limits, the queue's bound.

**Tests:** the world `tests/engine/store` begins here, with
`src/world.rs`, `src/root.rs` (a scripted root), `src/model.rs` and
`src/referee.rs`. Its first stories:

- commit, then load;
- paging across two intervals;
- a byte cut;
- a row that does not decode;
- each refusal at open.

**Depends on:** S2; skein's K1.

### S4. Durable

**What:** the same machine over skein-kv's log and snapshots on the
simulator's crashing disk (K2 to K4). Nothing changes in the crate but
what the disk teaches.

**Tests:**
- `tests/engine/store/tests/recovery.rs`: a crash at every file operation
  of a scripted run, each recovered and checked by the referee
  (README.md, section 12);
- `faults.rs`: failed writes and syncs, ending in `Uncommitted` and a
  stop;
- `fuzzy_crash.rs`: random workloads and crash points, within the fuzzy
  budget.

**Depends on:** S3; skein's K2 to K4.

### S5. Transcripts

**What:** `Append`, `Read` and `Drop` over skein-kv's payload logs
(K6, with section 3's asks): a turn's frame, appends inside commits,
reads forwards and backwards, a checksum failure as `Unread`.

**Tests:** the store's world gains these stories:
- a session written over two attempts, then read whole;
- a tail read backwards;
- two turns of one session decided before the first commits;
- a crash between an append's sync and its commit's frame, which leaves
  no visible turn, and the next append writing over it;
- a drop, with a crash before and after its commit.

The referee adds: no transcript byte past its commit, and none lost
before it.

**Depends on:** S4; skein's K6.

### S6. Secrets

**What:** `secrets.rs`: the second store's vocabulary for the engine's
protocol machines, not the root's.

- Put, get and erase a secret by its kind and number.
- Refresh tokens move from temper-oauth's file record into it, with the
  rule that a rotated token is durable before its grant
  (`credentials.md`).
- Sign-in digests and adopted credentials follow when the web's
  sign-in and adoption are built (`docs/plans/next-domain/08-after.md`,
  section 3).

**Tests:**
- step tests, and stories in the store's world: a rotation crashed
  before and after its commit, never granting an unkept token;
- the directory's and files' modes on the real ring.

**Depends on:** S4.

### S7. In the engine's process

**What:** the engine's shell opens both stores before the root's
`Start`, routes the root's store requests through the protocol layer,
and ends the process on `Uncommitted` (README.md, section 10). This is
part of the shell's own plan (`08-after.md`, section 3, "the engine's
`iterate` and shell"), which this increment joins.

**Tests:**
- on the real ring, in a scratch directory: start, a chat's turns
  committed, a kill around each phase of a turn's commit, a restart that
  resumes the chat from its transcript;
- restart time measured on a store generated at kv.md's estimated size,
  and recorded in `docs/design/performance.md`.

**Depends on:** S5, S6; the shell.

### S8. Conformance

**What:** one suite of scripted commits, loads and reads, run against
the fake store (S1) and the real one (S4, then S5 for transcripts),
comparing rows, pages, cursors' keys and turns. It lives in
`tests/engine/store/tests/conformance.rs`, and the fake store is reached
through the engine's world's crate.

**Depends on:** S1 and S4. Transcripts join with S5.

### S9. Indexes, retention, fill

**What:** the domain's side of README.md, section 11:

- index records for what the web pages through, in each child that
  owns them, as its routes are built (ended goals by project, chats by
  person);
- an ended-time index, and the root's retention pass that shrinks ended
  tasks to summaries and drops old sessions;
- traces with their quota and sweep;
- the fill's high-water mark refusing new work at the entrances.

**Tests:** stories in the engine's world:
- a task ended past its horizon, summarised and still shown;
- a session dropped and a live chat's kept;
- the store filling past its mark, with new chats refused and running
  work finishing.

Referee: no live task loses a row to retention.

**Depends on:** S1. It must be built before a real deployment's map
reaches half its budget, which kv.md's estimate puts months after first
use.

## 5. Budgets and checks

- **Tests:** the store's world's focused tests within a few tenths of a
  second in all, and its fuzzy tests within a few seconds, inside temper's
  15-second and one-minute suites (AGENTS.md). Each increment measures
  and records them.
- **Memory:** the crate's `worst_case`, and skein-kv's for both stores,
  join the engine's (programming-model.md, 6.3). A test at the limits
  holds the heap under the sum.
- **The gate** runs each increment's checks before main moves
  (`docs/development/workflow.md`).

## 6. Done when

- the engine runs in its own process with its records in skein-kv, its
  transcripts in files and its secrets in their own store;
- a killed engine restarts and resumes a chat from its transcript, with
  nothing it acknowledged lost;
- the fake store and the real one pass one conformance suite;
- the map's growth is bounded by retention, and its fill is refused at
  the entrances before it can fail a commit.
