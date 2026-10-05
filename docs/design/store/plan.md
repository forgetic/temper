# Building the store

Provisional, 2026-10-05. The plan for building the store of
`README.md` in this directory, in increments, each a branch through
temper's gate (`docs/development/workflow.md`). Some increments wait for
skein's: `skein-kv`'s plan (skein's `docs/plans/kv-implementation.md`,
here "skein's plan") builds the store itself, and this plan builds what
temper puts on it. The domain's half moves first, since it needs nothing
from skein.

## 1. In one page

- **The domain first.** S1 changes the root's store vocabulary
  (sessions, transcripts, no traces) and runs on today's fake store,
  which stays only until S4.
- **Then the real store everywhere.** S2 and S3 build
  `temper-engine-protocol-store` over skein-kv's in-memory mode, and S4
  puts it under every world in place of the fake. From then on, every
  story runs the real key encoding, codecs and paging.
- **Then durability.** S5 runs the same machine over skein-kv's log on
  the simulator's crashing disk, S6 adds transcript files, S7 the
  secrets' store, and S8 the engine's process.
- **Each step usable alone.** Every increment leaves the gate green
  and adds a world or a test that holds what it built.

```
skein:   K1 memory        K0 io files, crashing disk ──► K2 log ──► K3 snapshots ──► K4 bounds
           │                   │                          │
temper:  S1 domain ──┐         │                          │
         S2 codecs ──┴► S3 store machine ──► S4 worlds on the real store
                                 └────────────────────────┴► S5 durable ──► S6 transcript files ──► S8 shell
                                                                   └──────► S7 secrets ──────────────┘
         S9 indexes, retention, fill: after S4, before the map nears half its budget
```

`K0` is skein's plan's 5.0 (io's file entity, the crashing disk), and
`K1` to `K4` its 5.1 to 5.4. skein-kv's payload logs (its section 6) are
not needed.

## 2. The crate

```
crates/temper-engine-protocol-store/
├── Cargo.toml          skein-lib, skein-kv, skein-io (its file vocabulary only), temper-engine-domain (its store vocabulary)
└── src/
    ├── lib.rs          the crate's doc: what it translates, what it never decides; re-exports
    ├── keys.rs         every domain Key to bytes and back, in the derived order (README.md, 4.1)
    ├── ranges.rs       every domain Range to its intervals (README.md, 4.2)
    ├── codec.rs        a bounded byte writer and reader, versions, the shape tags
    ├── records/        a codec per shape: root.rs, tasks.rs, people.rs, then each child that keeps records
    ├── format.rs       the format row; refusals at open
    ├── limits.rs       the largest key, record and commit derived from the domain's limits; checks against skein-kv's
    ├── store.rs        the machine: the root's requests down, skein-kv's and io's events up (README.md, sections 6 to 8)
    ├── turns.rs        a turn's frame (README.md, 8.1)
    ├── files.rs        transcript files: appends before commits, reads both ways, the cut at open, drops
    ├── secrets.rs      the secrets' store's vocabulary and machine (README.md, section 9)
    ├── trace.rs        trace records, as data, for the process's logs
    ├── tests.rs
    └── tests/          step tests per area; golden/ holds each shape's bytes per version
```

It is `no_std` and a step machine with the shape of skein-kv's own:
`down` takes the root's requests, `up` takes skein-kv's and io's events,
and each emits events above and requests below, within a declared
`MAX_OUT` (programming-model.md, sections 3 and 4).

## 3. What temper asks of skein

For skein's plan, before the increments named:

1. **skein-kv's in-memory mode stays** after the files arrive (its 5.1).
   temper's worlds run on it, with `Commit` applied at once and `Get` and
   `Load` as they are with files.
2. **The fill** (the map's bytes against its budget) with each
   `Committed`, or readable at any step (README.md, 11.3).
3. **A second store in a process,** in a root of its own, created
   `0o700`, its files `0o600` (its 2.1, `Create`'s mode). This is already
   planned and is listed here to confirm it.
4. **io's file entity** gains opening an existing file to write (its
   2.3, which comes forward from payload logs to temper's transcripts) and
   cutting a file to a length, with the simulator's crash model for both.

## 4. Increments

### S1. Sessions in the domain

**What:** the root's store vocabulary takes transcripts by session
(README.md, section 3), on today's fake store.

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
- The views child keeps no traces. Its batches, its store writes and its
  sweep go, and so do the capture policy and its place in the assignment.
  Live watches stay as they are.
- The fake store (`tests/engine/domain/src/commits.rs`) gains sessions,
  appends inside commits, reads from either end with an opaque cursor,
  and drops, for the few increments it has left. A cut keeps appends with
  their commit or loses both.
- `domain/engine.md` 5.4, 5.5, 7.2 and 11 are revised with it (README.md,
  section 13).

**Tests:** the engine's world's existing stories pass, with turns
committed into sessions. A new story covers a chat that parks and resumes
across two attempts and is given both attempts' turns. The referee adds a
check: no turn is visible before its commit, and every committed turn is
visible after a cut. The views' world loses its trace stories.

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

`files.rs` comes in S6. Until then, the machine's transcript requests
are served by an in-memory file system behind the same file vocabulary,
which is the stand-in the domain worlds keep (README.md, section 12).

**Tests:** the world `tests/engine/store` begins here, with
`src/world.rs`, `src/root.rs` (a scripted root), `src/model.rs` and
`src/referee.rs`. Its first stories:

- commit, then load;
- paging across two intervals;
- a byte cut;
- a row that does not decode;
- each refusal at open.

**Depends on:** S2; skein's K1.

### S4. Every world on the real store

**What:** the fake store retires (testing-strategy.md, section 4).

- The engine's world puts the protocol layer and skein-kv's in-memory
  mode where the fake was, with the transcripts' in-memory files.
- Between the root and the store, the world holds commits in order. It
  delays them for a slow store, answers one `Uncommitted` for a failing
  one, and at a cut keeps what reached skein-kv and drops what it held
  (README.md, section 12).
- Each child's world that keeps its child's rows (`domain/engine.md`,
  5.6) keeps them in skein-kv's in-memory mode too, through that child's
  codecs.

**Tests:** every story and referee of the worlds passes unchanged. The
fake store's tests go. A focused test checks that a cut before and after
a commit reaches skein-kv restarts from the right rows.

**Depends on:** S1, S3.

### S5. Durable

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

**Depends on:** S3; skein's K0, then K2 to K4.

### S6. Transcript files

**What:** `files.rs` on io's file entity, with section 3's ask 4:

- appends written and synced before their commit is submitted, and
  later commits queued behind;
- the live sessions' committed lengths, saved with each append;
- reads forwards and backwards, never past what is committed;
- the cut at open;
- drops before their commit;
- a checksum failure as `Unread`.

**Tests:** the store's world gains these stories:
- a session written over two attempts, then read whole;
- a tail read backwards;
- two turns of one session decided before the first commits;
- a crash between a turn's sync and its commit, which leaves no readable
  turn, then the cut at open and the resent turn written over it;
- a session closed and read to its end with no length kept;
- a drop, with a crash before and after its commit.

The referee adds: no turn readable past its commit, and none lost before
it.

**Depends on:** S5; skein's K0 with section 3's ask 4.

### S7. Secrets

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

**Depends on:** S5.

### S8. In the engine's process

**What:** the engine's shell opens both stores before the root's
`Start`, routes the root's store requests through the protocol layer,
sends the engine's trace records to the process's logs, and ends the
process on `Uncommitted` (README.md, section 10). This is part of the
shell's own plan (`08-after.md`, section 3, "the engine's `iterate` and
shell"), which this increment joins.

**Tests:**
- on the real ring, in a scratch directory: start, a chat's turns
  committed, a kill around each phase of a turn's commit, a restart that
  resumes the chat from its transcript;
- restart time measured on a store generated at kv.md's estimated size,
  and recorded in `docs/design/performance.md`.

**Depends on:** S6, S7; the shell.

### S9. Indexes, retention, fill

**What:** the domain's side of README.md, section 11:

- index records for what the web pages through, in each child that
  owns them, as its routes are built (ended goals by project, chats by
  person);
- an ended-time index, and the root's retention pass that shrinks ended
  tasks to summaries and drops old sessions;
- the fill's high-water mark refusing new work at the entrances.

**Tests:** stories in the engine's world, now on the real store:
- a task ended past its horizon, summarised and still shown;
- a session dropped and a live chat's kept;
- the store filling past its mark, with new chats refused and running
  work finishing.

Referee: no live task loses a row to retention.

**Depends on:** S4. It must be built before a real deployment's map
reaches half its budget, which kv.md's estimate puts months after first
use.

## 5. Budgets and checks

- **Tests:** temper's focused suite stays within 15 seconds and its
  fuzzy suite within one minute (AGENTS.md). S4 puts the codecs and
  skein-kv under every world, so it measures both suites before and after
  and records the difference. The store's world's focused tests take a few
  tenths of a second in all, and its fuzzy tests a few seconds.
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
- every world runs on the real store, with no fake of the map left;
- the map's growth is bounded by retention, and its fill is refused at
  the entrances before it can fail a commit.
