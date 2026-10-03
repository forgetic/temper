# Performance

Provisional, 2026-10-03. Whether the memory strategy of
`programming-style.md` (section 6: counted entities, owned bytes, the
worst case checked at startup) costs temper anything that matters, where
it could, and what to do about each case. It reviews the strategy against
temper's workload; it adds no mechanics of its own. Each fix belongs in
the document or crate it changes, and moves there when it is made.

## 1. In one page

- **No significant problem.** temper's time goes to LLM calls, which take
  seconds each, and to forge calls under a rate limit, at modest
  concurrency. Each agent run is a process of its own (`worker-model.md`,
  section 6), so one process holds one run's sessions. What the memory
  strategy costs is microseconds to milliseconds per turn.
- **Four things are worth knowing,** each fixable by a contained change
  that touches no step code built so far:

  | Concern | What it costs | Fix | Effort | When |
  |---|---|---|---|---|
  | Bytes of unknown length (3) | quadratic copying, if done naively | two lib containers | small | before the protocol layer |
  | Copy at emission, every turn (4) | peak memory, up to three copies of a transcript | count the copies; drop the prompt once encoded | small | with the protocol layer |
  | The general allocator (5) | page faults, fragmentation | glibc tuning, or another allocator, in the shell | small | after measuring |
  | A worst case of maximums (6) | memory headroom, not speed | the byte budget of style 6.4 | moderate | only if it bites |

- **Only the first needs action before more is built.** The others are
  accounting to add with the protocol layer, or measurements to take once
  the shell runs.

## 2. The workload

- **The agent** runs sessions. Each turn sends the whole transcript to the
  provider (its API keeps no state between calls) and receives an answer
  streamed in pieces. Tool calls read files within `read_bytes`, run
  commands whose output is kept as a head and a tail (`shell_head`,
  `shell_tail`), and search. A session holds at most `session_bytes`.
- **The worker** runs agent processes and git, whose output its protocol
  layer parses.
- **The engine** is the long-lived process. It holds a bounded working set
  of forge items, read a page at a time, not the forge's contents.

All three wait far more than they compute. Copying a megabyte takes
around a tenth of a millisecond; an LLM call takes seconds.

## 3. Bytes of unknown length

A `Box<[u8]>` cannot grow, `List` allocates its whole capacity when it is
made, and `Writer::new` wants the final length up front. That suits bytes
whose length is known before they are made, which is most of them. Some
arrive in pieces with no total announced:

- a provider streams its answer as deltas: text a few tokens at a time,
  and a tool call's input as fragments of JSON, which the service's
  decoder joins into one block before the model sees it;
- a command's output arrives as pipe reads, of which the protocol layer
  keeps the first `shell_head` bytes and the last `shell_tail`;
- an HTTP body may be chunked.

Joining by making a new box for each piece copies everything received so
far each time, which is quadratic: a 10 MB build log in 64 KB pieces
copies about 800 MB, and a 32 KB answer in 1,000 deltas about 16 MB.
Making a `List<u8>` at the cap instead allocates the cap for every call in
flight, however short its answer.

Nothing does either today. Model code builds bytes in one pass:
`translate::concat` (`crates/temper-engine-model/src/translate.rs:339`)
adds up the lengths, then writes once. The pieces that need joining all
live in the protocol layer, which is not written yet. Below each machine,
the carry-over is already safe: skein's lib, where temper's lib is
moving with the stream machines (HTTP, server-sent events, JSON), has an
`Intake`, a buffer allocated once at its cap that delivers each demand as
a box of exactly its length.

**Fix: two more lib containers,** each with its `worst_case`, for what
happens above the intake:

- **A capped buffer that grows by doubling.** It starts small. A write
  that does not fit grows it to twice its size, or to what the write
  needs if that is more, but never past the cap; a write past the cap is
  refused whole, writing nothing, which the caller handles as any other
  limit. Finishing trims it to a box of exactly its length.
  - Each byte is copied in once, about once more on average as the buffer
    grows, and at most once more by the trim: linear in all.
  - While it fills it holds at most twice what it has received, and never
    more than the cap. Its worst case is the cap, as for a buffer made at
    the cap, but a short answer costs only its own length.
  - A list of pieces joined once at the end copies each byte only once,
    but a provider's deltas are a few bytes each: it would make one
    allocation per piece, and need a cap on pieces that is hard to
    choose.
- **`ByteRing`,** which `programming-style.md` (10.2) lists in lib and no
  lib has yet: a buffer of fixed size, made once, that keeps the last N
  bytes written, each write overwriting the oldest, where an intake
  refuses past its cap instead. A command's output fills its head first,
  a capped buffer of `shell_head` bytes; the rest goes through a ring of
  `shell_tail` bytes, and what falls out of the ring is counted as
  dropped. At exit the ring is read out, oldest first, as the tail.

Both are small modules, written in skein's lib before the first machine
that joins pieces, and they change no existing code.

## 4. Copy at emission, every turn

`complete` (`crates/temper-agent-model-session/src/session.rs:1318`)
copies the whole transcript on every turn, message by message and block
by block, as 6.3 of the style says it must: the session keeps the
transcript, and the protocol layer owns the copy. Over a session the
copying grows with the square of the number of turns.

**The time does not matter.** A session of 200 turns whose transcript
grows to 1 MB copies about 100 MB in a few hundred thousand allocations:
tens of milliseconds in all, over a session that lasts tens of minutes.
The protocol layer must encode the same bytes as JSON on every turn
anyway, so the copy is a constant factor on a cost the workload already
has.

**The peak memory does.** While a call is in flight a session's bytes are
held up to three times: the transcript in the session, the prompt in a
queue or the protocol layer, and the encoded body on its way out through
io. The session's worst case counts the transcript once
(`crates/temper-agent-model-session/src/limits.rs:85`); the copy is its
receiver's to count. **Fix,** when the protocol layer is written:

- its worst case counts, for each call in flight, a prompt at the session
  byte limit and an encoded body at its escaped worst case (a control
  byte escapes to six), or caps the encoded body with a limit of its own;
- it drops the prompt as soon as it has encoded it, so for most of a call
  the session's bytes are held twice;
- or it measures the whole body first (the sized writer does), sends the
  length, and encodes the rest piece by piece as io grants room, so the
  encoded body never exists whole.

**Removing the copy** is possible but not worth doing unless measured: the
transcript only grows at its end, so the protocol layer could keep each
session's encoded transcript and the session send only the messages
added since the last call. That changes the contract between the session
and the protocol layer, the fake LLM and their worlds: moderate work,
for a cost that does not show.

## 5. The general allocator

The style accepts the general allocator in the hot path (6.2): allocation
time is not constant, and fragmentation can push the resident size above
the live bytes. With glibc:

- an allocation above the mmap threshold (128 KB to start) is its own
  `mmap`, page faults as it is filled and a `munmap` when freed;
- after such a free, glibc raises the threshold to its size, up to 32 MB;
  large boxes then come from the heap, where a long-lived process making
  boxes of mixed sizes can fragment.

The engine is the process this could affect; an agent process lives for
one run. **Measure first,** as 6.4 says: the simulator's counting
allocator gives the live bytes, and a load run of the real shell gives
the resident size to compare them with. **If the gap matters,** in order:

1. tune glibc once at startup with `mallopt`, through `libc`, which the
   shell already depends on: a fixed mmap threshold and trim threshold
   suited to temper's sizes;
2. or give the shell another `#[global_allocator]`, a one-line change that
   adds a dependency `programming-style.md` (2.1) does not allow today, so
   the departure is recorded there.

Neither touches step code.

## 6. A worst case of maximums

The worst case (style, 6.4) is every entity at its cap at once. Transcript
sizes have a long tail: most sessions stay small and a few approach
`session_bytes`. With the copies of section 4 a process sets aside about
sessions × `session_bytes` × 3, so eight sessions of 4 MB come to about
100 MB. That costs headroom, not speed, and it bites only if one process
should run many sessions with contexts of a million tokens at once.

**Fix, if it bites:** the byte budget the style keeps as its fallback
(6.4).

- For io's queued output it is ordinary backpressure: io grants room
  within a global budget, as the style describes.
- For sessions it is harder. If sessions charged a shared pool as they
  grew, one could run out partway through its work because of the others,
  which is the refusal in the middle that section 7 of the style rules
  out. Done right, a session is granted bytes from the pool when it opens,
  and perhaps again between turns, where waiting is an ordinary state.
  That touches the agent model's top level, the session crate and their
  worlds: a few days' work, after a design decision.

It is probably never needed: a process holds one run's sessions, and its
limits are that run's.

## 7. What does not cost

- **Allocating slabs, queues and lists up front.** Their capacities are
  counts, and their items are small; what the items own is allocated as
  it is needed.
- **B-trees instead of hash maps.** Logarithmic lookups over hundreds or
  thousands of entries.
- **Moves between layers.** A payload crossing a boundary moves a pointer,
  not its bytes.
- **Copying out of io's buffers.** One copy of each byte received, which
  exact-size reads remove later without touching anything above io
  (style, 6.6).
- **Retiring at the reclaim point.** A mark during the iteration and a
  push onto a free list at its end.
