# Programming style (Rust)

Provisional, 2026-10-01. The shape a network service on Linux io_uring
should have when it is written in Rust.

**How to read this.** This is the language-agnostic model (saf's
`PROGRAMMING-MODEL-AGNOSTIC-RELAXED.md`) with its open choices made for
Rust: the memory strategy (section 6), the subset of the language (section
10), the crate layout (section 12), and what checks each rule. It stands
alone; section 13 lists where it departs from the agnostic model. The
subset is deliberately small: Rust is used for ownership and borrow
checking, enums with data and exhaustive matching, and crate boundaries,
and for little else. Where the compiler or clippy can check a rule, the
check is named; where neither can, the rule is a convention held in
review. Section 10.1 says which is which.

## 1. In one page

- **One thread, one loop, no async.** The loop reaps completions from
  io_uring, runs the step functions, and submits what they asked for. It is
  the only code that talks to the kernel, and the only thing that schedules
  work: no `async`, no futures, no runtime, no callbacks.
- **Three layers, three crates.** `io` (kernel operations and their
  buffers), `protocol` (bytes to typed messages and back), `model` (domain
  logic). The crate graph is the layer diagram: the model crate does not
  depend on io, so it cannot name a file descriptor. Below its top-level
  crate, a large model is a tree of sub-model crates (4.5).
- **The model is complete.** It runs the service's whole behaviour in a
  world of models and fakes, with no protocol and no io. The protocol layer
  translates between bytes and model entities, and decides nothing.
- **Step functions are sans-io, and the compiler knows it.** Step crates
  are `#![no_std]` with `alloc`: no syscalls, no clock, no threads, no
  printing, no hash maps with random seeds. Time and randomness are inputs;
  effects are outputs. The same state and input give the same output.
- **Entities are named, not referenced.** Each entity lives in a slab owned
  by its layer and is named everywhere else by a typed handle, `Id<T>`,
  that is never reused for another entity. No application type has a
  lifetime parameter, so no reference can be stored and no borrow outlives
  a step.
- **Flow control is explicit.** Reading is a demand, a connection has one
  request in flight, and a peer that does not read stops being read from.
  Whatever the service limits, it refuses at the entrance, where saying no
  is cheap.
- **Counted entities, owned bytes.** Slabs and queues are sized at startup,
  and a full slab is a refusal. Bytes are a `Box<[u8]>` allocated at its
  final length after validation, owned by one place at a time and moved,
  never shared. The worst-case footprint is computed at startup and must
  fit.
- **One lifecycle everywhere.** Every entity is active, then closing, then
  closed. Close goes down, closed comes up, reclaiming is bottom-up, and
  every request gets exactly one terminal event. Nothing happens in a
  destructor.
- **A small Rust.** Structs, enums, `match`, functions, references, moves
  and a short list of library types. No `async`, closures, trait objects,
  user-defined traits or generics, `Rc` or `RefCell`, `Drop` impls, or
  `unsafe` outside one module. Panics abort.

## 2. The loop

```rust
// shell: the only impure code
loop {
    ring.reap(&mut svc.completions);         // completions and effect results
    let now = clock.now();                   // read once: time for the iteration
    service::iterate(&mut svc, now);         // pure: both passes, the reclaim point
    let wait = !svc.work_pending();          // block only when there is nothing to do
    ring.submit(&mut svc, wait);
}
```

```rust
// service::iterate: pure, and the same function the simulator drives

// up pass
for c  in take(completions) { io::up(&mut io, &io_env, c, &mut proto_in, &mut subs) }
for ev in take(proto_in)    { protocol::up(&mut proto, &proto_env, ev, &mut model_in, &mut proto_out) }
for ev in take(model_in)    { model::step(&mut model, &model_env, ev, &mut model_out) }

// down pass
for rq in take(model_out)   { protocol::down(&mut proto, &proto_env, rq, &mut proto_out) }
for rq in take(proto_out)   { io::down(&mut io, &io_env, rq, &mut subs) }

// reclaim point
io.reclaim(); proto.reclaim(); model.reclaim();
```

The sketch shows the main flow only. `take` hands a stage its inputs one
at a time while the step's output queues have room for the most one input
can produce, a bound each entry point declares as `MAX_OUT`; whatever does
not fit waits for the next iteration. A step in the up pass may also queue
requests downward (a protocol machine closing on a framing error), and
those join the down pass of the same iteration. The reverse happens too: a
step in the down pass may produce an event that must go up, such as a read
demand already met by buffered bytes, or a request refused without
reaching the kernel. That event is held as state of the entity that will
emit it, not as a queued record, so it outlives the iteration's queues:
the entity goes on its layer's ready list, which the layer drains at the
start of its stage in the next up pass. A stage then takes its input
events, then fires its expired timers (section 9).

```
completions -> io::up   -> protocol::up   -> model::step     (up pass)
submissions <- io::down <- protocol::down <- model::step     (down pass)
```

Properties the rest of the model relies on:

- **Work per iteration is bounded.** Every queue is a `lib::Queue` whose
  capacity is fixed at startup; whatever does not fit waits for the next
  iteration.
- **`now` is read once** and is the same for every step in the iteration.
- **Requests made in the down pass are submitted at the end of the
  iteration;** the events they cause arrive in a later one. A step never
  waits for its own effect.
- **The loop never blocks while a queue or a ready list is non-empty,** so
  it does all the work it can before waiting on the kernel. Going round
  costs a completion-queue read, a clock read, and a ring syscall only when
  there is something to submit or the kernel has completions to flush.
  Reaping every round keeps one peer's backlog from holding up other
  connections' completions and timers.
- **Nothing is reclaimed mid-iteration.** An entity that reaches *closed*
  is retired, not removed; slabs free retired slots at the reclaim point,
  so every entity present when an iteration begins can still be looked up
  until the iteration ends.

### 2.1 The shell

The shell is the only impure code: the loop above, the ring, the clock,
the random seed, and a small closed list of syscalls that are not ring
operations (spawning a process, sending a signal). Shell effects use the
same request and event shapes as ring operations, so step code cannot tell
them apart. Anything that can be a ring operation is one: accept, connect,
read, write, open, close, waiting for a child.

- **The ring adapter is the only `unsafe` code in the service.** It is
  built on the `io-uring` crate; the shell's other syscalls go through
  `libc`. These are the only dependencies from outside the workspace.
- **No std handle types that close on drop.** `OwnedFd`, `File`,
  `TcpStream`, `TcpListener` and `std::process::Child` release kernel
  resources in their destructors, at a moment the lifecycle did not
  choose. A file descriptor is a plain `Fd(i32)` inside io, closed by a
  ring `close`.
- **Child processes are handled through pidfds:** spawned with
  `clone3(CLONE_PIDFD)`, signalled with `pidfd_send_signal`, waited for
  with `waitid(P_PIDFD)` on the ring. A pidfd is the kernel's own
  stale-handle check against PID reuse. `std::process` is not used.
- **Panics abort.** `panic = "abort"` in every profile: a panic anywhere is
  fail-stop, and a supervisor restarts the process.

## 3. Step functions

```rust
pub fn step(model: &mut Model, env: &Env<Limits>, ev: Event, out: &mut Queue<Request>)
```

```
model   this layer's state, updated in place: the only &mut the step receives
env     now and this layer's limits, behind a shared borrow, so read-only
ev      one self-contained input, moved in
out     a bounded queue of owned requests, with MAX_OUT slots reserved by the loop
```

A step returns nothing. Every outcome of an event is a change to the
layer's state or a request in `out`; the loop reserved the room, so
emitting cannot fail, and running out of heap aborts (section 6). Inside a
step, every fallible operation returns a `Result` or an `Option`, which the
caller cannot silently drop (section 10).

The protocol layer has two entry points of this shape: `protocol::up` for
events from io and `protocol::down` for requests from the model. io has the
same pair. The model has one.

What "pure" means, and what holds it:

- **No syscalls,** directly or through a library: no printing or logging,
  no clock reads, no entropy, no file, socket or process operations, no
  threads. Step crates are `#![no_std]` with `extern crate alloc`, so
  `std::{io, fs, net, time, thread, process, env}`, `println!` and
  `thread_local!` do not exist there, and they depend on nothing outside
  the workspace.
- **Time is data.** The step reads `env.now`, a `lib::Time`; a deadline is
  a value it computes and arms in its own layer's table (section 9).
- **Randomness is injected state:** a `lib::Rng` in the layer's state,
  seeded by the shell or the simulator.
- **Nothing observable depends on memory.** No output may depend on
  addresses or on allocation order. `alloc` has no `HashMap`; maps are
  `BTreeMap`, whose order is the keys'. There is no formatting in step
  code, so no address can be printed.
- **No hidden state.** No `static` (a mutable one needs `unsafe`, and
  atomics and cells are disallowed types), no closures carrying state
  between calls; all state is in the arguments.
- **No hidden control flow or effects.** Outcomes are returned; panics are
  fail-stop. There are no `Drop` impls in the service: closing is a
  request, and reclaiming follows *closed* (5.2). Dropping a `Box<[u8]>`
  frees memory and does nothing else.
- **Effects are data.** To send bytes or close a socket, a step pushes a
  request into its output queue. The outcome arrives later as an event.
- **Bounded.** No `loop` or `while` in step code: `for` over a range or a
  slice, whose bound is a configured limit or the size of an input already
  validated. No scan over an unbounded structure, no recursion.
- **Diagnostics are data too.** Trace records are enum values pushed into a
  bounded queue the shell writes out. Assertions are fail-stop.

What it buys: a simulator can own the clock, the random state and the
"kernel", and run the service deterministically under fault schedules; a
recorded event log replays to the same state; each layer can be fuzzed or
unit-tested by feeding events and inspecting the requests that come out;
and a step that cannot block cannot stall the loop.

## 4. The three layers

| Layer | State is bound to | State dies when | Main risk | Best check |
|---|---|---|---|---|
| io | the kernel | the operation completes | pinned buffers, cancellation | fault-injection simulation |
| protocol | the peer | the connection closes | untrusted input | fuzzing |
| model | the domain | the session ends | logic, policy | simulation, replay |

```
crate      depends on
lib        nothing
io         lib
model      lib                   and its sub-models (4.5)
protocol   lib, io, model        the one crate that sees both vocabularies
service    lib, io, protocol, model
shell      service, io-uring, libc
sim        service
```

- **Every completion goes through io first.** The model never sees a raw
  completion, a file descriptor, a kernel error code or a half-filled
  buffer; it cannot, since it does not depend on io.
- **The model never parses.** Bytes are untrusted until the protocol layer
  has turned them into typed, size-bounded messages. Structure inside a
  payload (JSON in a body) is one more protocol machine, not model code.
- **The model is complete.** Everything a peer can cause arrives as a
  model entity, and everything the model wants done leaves as one, so a
  world of models and fakes runs the service's whole behaviour with no
  protocol and no io (section 11). The protocol layer only translates
  between bytes and model entities: it decides nothing, and it never sits
  between two pieces of model logic. Structure the domain acts on is
  decoded on the way in, all of it: a tool call inside an LLM's answer
  reaches the model as a typed call, not as JSON to be sent back down for
  decoding later.
- **Policy above, mechanism below.** The model decides the deadline and
  whether to retry; the protocol layer runs the timer and the attempt.
- **A lower layer absorbs mechanics, not information.** The model still
  learns *that* a call timed out; it does not see the cancel race behind
  it. And nothing hides cost: an entity that lingers still holds what it
  holds.
- **Three stages, not three functions.** The protocol stage may be a stack
  of machines per connection (TLS, HTTP, event framing, JSON), each with
  entry points of the same shape, each fuzzable alone, each pulling from
  the one below only when it has room to push up. The stack is static: the
  outer machine calls the inner machine's functions directly. No trait, no
  `dyn`.
- **io and lib are nearly application-independent.** io's vocabulary is
  fixed (4.3); write them once and test them hard.

### 4.1 Each layer has its own entities

There is no shared "connection" across layers. A model client may outlive
many connections; one socket may carry many protocol streams; one protocol
connection may serve many model calls over time; a child process is three
pipes and an exit status. Name them differently per layer so they are not
conflated: *socket* (io), *connection* (protocol), *client* or *session*
(model).

### 4.2 Bindings are echoed tokens

Adjacent layers link the way io_uring's `user_data` works, applied at each
boundary:

- Going down, the upper layer passes a **token**, its own handle for the
  entity concerned. The lower layer stores it without interpreting it.
- Going up, the lower layer **echoes the token** on every event.
- The upper layer stores the lower layer's handle, also as a token, to
  address requests to it.

Every name that crosses a boundary is a `lib::Token`, an opaque `u64`. A
layer makes one from its own handle (`id.token()`) and turns it back into
one (`Id::<Conn>::from_token(t)`), and only for tokens it issued, in the
record variant it issued them for. Within a layer, names are `Id<T>`. Each
side holds the other's name as an opaque field; there are no mapping
tables. For entities created from below (an accepted socket), the lower
layer announces the new entity and the upper layer either binds to it or
asks for it to be closed.

One token type keeps io free of generics and the model free of protocol
types. The price is that the kind of entity a token names is carried by the
record variant, not by the token's type: nothing but that variant stops a
layer from decoding a token as the wrong kind.

### 4.3 io to protocol

```rust
pub enum Event {                                  // io -> protocol
    Accepted { listener: Token, socket: Token },
    Bytes    { owner: Token, bytes: Box<[u8]> },  // exactly the demand
    Room     { owner: Token },
    End      { owner: Token },
    Failed   { owner: Token, error: Error },
    Closed   { owner: Token },                    // terminal
    Effect   { owner: Token, result: EffectResult },
}

pub enum Request {                                // protocol -> io
    Listen  { owner: Token, addr: Addr },
    Connect { owner: Token, addr: Addr },
    Bind    { socket: Token, owner: Token },      // attach to an accepted socket
    Reject  { socket: Token },                    // close a socket never bound
    Demand  { socket: Token, read: Read, room: u32 },
    Send    { socket: Token, bytes: Box<[u8]> },
    Close   { socket: Token },                    // flush, then close, within a close deadline
    Abort   { socket: Token },
    // file operations, spawn, signal, wait for child
}

pub enum Read { Nothing, Fill(u32), Scan { until: Delimiter, max: u32 } }
```

- **Reading is a demand, not a request.** Each protocol state says what it
  needs, "fill N bytes" or "scan to a delimiter, at most M bytes", and how
  much output room. io sends `Bytes` only when that demand can be met, and
  `Room` likewise; a state with no demand receives neither. `Bytes` carries
  exactly the demanded bytes as an owned `Box<[u8]>`, which the protocol
  layer reads through a `lib::Reader`. It never handles io's receive
  buffers: unparsed input belongs to io, under io's cap.
- **Writing is a move.** The protocol layer encodes a message with a
  `lib::Writer` into a `Box<[u8]>` of exactly its length and moves it down
  in `Send`. io queues it against the connection's output cap, then moves
  it into the send operation (6.3).
- **Accepts are batched:** at most a configured number per iteration; the
  rest wait in the kernel's backlog. A flood of connections is then
  ordinary backpressure, not a flood of rejects.
- With multishot receive and a provided buffer ring, idle connections hold
  no read buffer at all, and an empty buffer ring is ordinary
  backpressure. The provided buffers are one region io allocates at
  startup. None of this is visible above io.
- **Timers are not io requests.** Each layer keeps its own deadline table
  (section 9).

### 4.4 Protocol to model

Events up and requests down are owned values with no lifetime parameters,
so they hold no references into either layer's state: a token, a variant,
and a payload the record owns, size-bounded by the protocol's limits. Such
a queue is loggable and replayable as it stands. The types belong to the
model: the model crate defines what it accepts and emits, and the protocol
crate depends on it.

```rust
pub enum Event {                                  // protocol -> model
    Call { reply_to: ReplyTo, op: Op, key: Box<[u8]>, value: Box<[u8]> },
    // terminal events for requests the model made
}

pub enum Request {                                // model -> protocol
    Reply { to: ReplyTo, status: Status, value: Option<Box<[u8]>> },
    // requests the model makes: outbound calls, each with its own token
}
```

Two shapes cross this boundary:

- **Requests down, one terminal event up,** as at every boundary.
- **Calls up, replies down.** A peer's request arrives at the model as a
  `Call` carrying a `ReplyTo`; the model answers with exactly one `Reply`,
  which consumes it, now or later (a model that answers later keeps the
  `ReplyTo` in the state that waits for the answer). `ReplyTo` is neither
  `Copy` nor `Clone`, so the compiler rejects a second reply; the
  simulator catches a missing one. Wire correlation ids stay in the
  protocol layer; opcodes and statuses cross as domain enumerations. The
  model knows nothing about connections.

### 4.5 Sub-models

A model too large for one crate is a tree of sub-models under one
top-level model crate.

- **Each sub-model is a step machine of its own:** its own vocabulary,
  limits, worst case, state machines, entry points and `MAX_OUT`, and its
  own tests. It depends on lib and on its children, never on a sibling or
  a parent.
- **A parent owns its children's states and routes between them** within
  its step. Hand-offs inside the model are short and acyclic, so a step
  completes them before it returns; its `MAX_OUT` follows from its
  children's along the longest chain.
- **Siblings share no domain types.** The parent translates between their
  vocabularies with small total functions; an exhaustive match makes a
  change on either side break the build in one place.
- **Only the top-level model faces the protocol layer,** and its worst case
  is the sum of its sub-models'.
- **Each sub-model has a world of its own** (section 11), and so does the
  top-level model.

## 5. Entities, handles and lifecycle

### 5.1 Handles

A handle is typed, opaque, and never reused for another entity: a slot
index plus a generation, typed by the entity it names.

```rust
conns.insert(conn)  -> Result<Id<Conn>, Conn>   // Err hands the value back: full, so refuse
conns.get_mut(id)   -> Option<&mut Conn>        // None if the slot holds another entity now
conns.retire(id)                                // freed at the reclaim point (5.2)
```

- **One handle type per entity kind.** `Id<Conn>` and `Id<Session>` are
  different types, so passing one where the other is expected does not
  compile. Never a bare integer.
- **Lookup checks the generation** and returns an `Option`: a completion
  for slot 27 generation 12 cannot act on the unrelated entity now in slot
  27 generation 13. A slot whose generation would wrap is retired for good.
- **The reference a lookup returns is borrowed for the current step only.**
  The compiler holds this: no application type has a lifetime parameter,
  so the reference cannot be stored in state, in a record or in a queue.
- **One entity per slab at a time.** `get_mut` borrows the whole slab, so a
  step works on one entity, copies out the handles and values it needs,
  and then looks up the next. The rare step that needs two at once (the
  two halves of a proxy) uses `Slab::get2_mut`, which fails on equal
  handles.
- **Handlers are free functions over the fields they touch,** not `&mut
  self` methods on the whole layer: the borrow checker splits borrows
  across fields at a call site, but not through a method that takes all of
  them.
- **Long-lived state holds handles, not references to other entities.** It
  is then snapshottable and comparable, and it can neither dangle nor keep
  a closed entity alive.

### 5.2 Lifecycle

Every entity is *active*, then *closing*, then *closed*, at every level:
operations in io, connections and requests in protocol, sessions and
exchanges in the model.

- **Close is a request going down; closed is an event coming up.** An
  entity is never retired on *end of stream*: the kernel may still hold
  its buffers.
- **Retire only when no bindings remain;** each binding ends with a closed
  (or detached) event from below. `slab.retire(id)` marks the slot, and the
  slab frees it at the reclaim point. Freeing is therefore bottom-up.
- **Every request gets exactly one terminal event:** success, failure,
  cancelled or timed out, whatever happened below.
- **Stale-handle asymmetry.** A token travelling up is never stale, so a
  stale one is a bug and an assertion (`expect`). A handle travelling down
  may be stale (a reply to a client that just left), so a failed lookup is
  a silent drop.
- **Ownership forms a tree** (server owns connections, a connection owns
  its requests, a request owns its exchanges); every other link is a
  handle. The owner closes what it owns. Ownership is a property of
  states: it is written down per state and moves only in transitions. In
  Rust terms a layer owns its slabs, a slab owns its entities and an entity
  owns its bytes; the model's tree sits on top, and its links are handles.
- **Lifetime is the lifecycle.** An entity ends when it is reclaimed after
  *closed*, never earlier and never later. Rust's ownership decides when
  memory is freed, when a `Box` is dropped or a slot reclaimed, never when
  an entity ends; and with no `Drop` impls, freeing memory has no other
  effect.

### 5.3 Races

A request with a deadline has two competing outcomes. If the timer wins,
the operation is still in flight, cancelling it is asynchronous, and a
late event always arrives (sometimes a completion, when the cancel lost in
the kernel). The lowest layer that knows both competitors runs the race:
it reports the winner upward at once, keeps its entity in a *settling*
state until the loser's terminal event has arrived, then retires it.
Layers above see one event.

### 5.4 State machines

- **States are explicit:** an enum per machine, each variant holding
  exactly what that state holds (bytes, handles, a `ReplyTo`). A state and
  what it holds are one value; no field is meaningful in some states only.
- **The table is total.** Every state × event cell is a transition, an
  ignore, or impossible. "Impossible" is only for cells the loop's own
  rules make unreachable (bytes delivered to a state with no read demand),
  written `unreachable!("why")`; anything a peer can cause is handled.
- **Matches over states and events are exhaustive,** with no `_` arm, no
  `matches!` and no `if let`, and no `#[non_exhaustive]` on the enums, so a
  new state or event is a build error at every site. Match on one enum,
  then on the other inside each arm, never on a tuple of both: a `_`
  inside a tuple pattern escapes clippy's wildcard lint.
- **One small handler per cell:** a free function that takes the source
  state's data by value and returns the target state. Small handlers are
  easier to test, fuzz and review, and function coverage of the handlers
  is transition coverage (section 11).

```rust
enum ConnState {
    Header,                        // demand: fill HEADER_LEN; progress deadline runs
    Body { header: Header },       // demand: fill header.body_len; progress deadline runs
    Waiting { correlation: u32 },  // one request in flight: no demand, no deadline
    Closing,                       // close requested, waiting for closed
    Closed,                        // terminal: holds nothing
}
```

Every transition uses the same idiom: move the state out, leaving the
terminal state in its place, and assign the result of the match back.

```rust
let state = mem::replace(&mut conn.state, ConnState::Closed);
conn.state = match state {
    ConnState::Header => header_read(&bytes, env, conn.socket, down),
    ConnState::Body { header } => body_read(header, bytes, id, up),
    ConnState::Waiting { .. } | ConnState::Closing | ConnState::Closed => {
        unreachable!("bytes delivered without a read demand")
    }
};
follow_state(conn, id, env, timers, down);  // demand and progress deadline, in one place
```

- The placeholder is the terminal state, which holds nothing, so putting it
  in costs nothing. The match is the right-hand side of the assignment, so
  no cell can forget to produce a target state. A panic in between is
  fail-stop, so the placeholder is never observed.
- The handler owns the source state's data, so everything the source held
  is visibly moved into the target, moved out in a request, or dropped
  there. That is "a transition releases whatever the source state held
  and the target does not", with the compiler pointing at every case.
  Dropping is right for bytes; for a handle to something that must be
  closed, the handler requests the close, and the simulator checks that
  nothing is left open.
- What a state implies (its read demand, whether the progress deadline
  runs) is an exhaustive function of the state, applied in one place after
  every transition, not repeated in every handler.

## 6. Memory and data

### 6.1 The strategy: counted entities, owned bytes

- **Entities live in slabs sized at startup.** Every entity kind has a
  `lib::Slab<T>` whose capacity comes from the limits and never changes. A
  full slab is a refusal at that layer's entrance, which is where the model
  wants refusals anyway (section 7). Handles are slot index plus
  generation.
- **Queues and tables are bounded at startup:** every `Queue`, ready list and
  deadline table has a capacity from the limits and refuses past it. Slabs
  and queues allocate that capacity up front; a table may allocate as it
  fills (`lib::Deadlines` is a pair of B-trees), and its worst case counts
  its container overhead, not just its entries.
- **Bytes live on the heap as `Box<[u8]>`,** allocated at their final
  length after the length has been checked against the limits. A
  `Box<[u8]>` cannot grow, has one owner, and keeps its address when it is
  moved. Bytes are moved from owner to owner, never shared.
- **Domain storage is ordinary owned data under domain limits:** a
  `BTreeMap<Box<[u8]>, Box<[u8]>>`, say, with the model counting entries
  and bytes and answering "full" as a domain result.
- **Running out of heap aborts.** Allocation failure is never a status.
  Short of a bug it cannot happen, because the worst case is computed and
  checked at startup (6.4).

### 6.2 Why this strategy

Every entity's bytes are already capped by protocol limits: the largest
message, the unparsed input, the queued output. Capping how many entities
exist therefore caps the bytes, without pooling them. Counts are where
fixed budgets are cheap and useful; bytes are where they are expensive.

Against budgets fixed at startup for bytes as well (byte pools):

- **No mutable state shared between layers.** A payload moves from io to
  protocol to model as a `Box`: a move the compiler checks, no copy, and
  every layer's state stays private. With byte pools, either every step
  gets `&mut` to one shared pool, or each layer has its own and copies at
  every boundary.
- **Exact sizes.** A 100-byte message takes 100 bytes, not a slot of a size
  class, and there are no size classes to tune.
- **Fewer cells.** Making a payload cannot fail, so there is no "pool
  empty" transition at every point that makes one; refusals happen at the
  entrances only.

Against the language's heap for everything:

- **The admission check is the container.** A full slab is the refusal;
  there is no separate counter to keep in step with the entities.
- **Handles come with the slab:** a generation check and constant-time
  lookup, instead of a map from id to entity.
- **Entity tables never grow,** so nothing reallocates under load.
- **Exhaustion is testable:** a simulation with a capacity of 2 reaches
  every admission point.

Against an accounted heap: it is the same for counts; for bytes it relies
on per-entity caps instead of quotas, and keeps accounting as the fallback
(6.4).

What it gives up: running out of heap aborts rather than refuses, so the
worst case must fit (6.4); and the general allocator sits in the hot path,
so allocation time is not constant, and fragmentation can push the
resident size above the live bytes.

### 6.3 What holds

- **Memory the kernel touches belongs to io and stays put.** Every buffer,
  socket address, iovec or msghdr a submission refers to is either a
  separate heap allocation owned by the operation's slot from submission
  to completion, or part of the provided-buffer region, which io allocates
  at startup and keeps for the life of the ring. Moving a `Box` moves the
  pointer, not the bytes, so the address the kernel holds stays valid;
  nothing the kernel touches is stored inline in a struct that a slab
  could move.

  ```
  owned by the socket --move into op--> owned by the op, kernel holds its address --completion--> moved out: freed or reused
  ```

  The compiler checks io's side: once the `Box` is in the op slot, no
  other code can name it. The ring adapter's `unsafe` covers the kernel's
  side: it takes addresses only from op slots, and io moves nothing out of
  an op slot before its completion.
- **The kernel never holds model memory.** Only io's transit memory is
  referenced by a submission. Model state is always safe to mutate, evict
  or snapshot.
- **Layers share nothing by reference; a move is the copy.** Records are
  owned values. A payload that crosses a boundary is moved into the
  record, and the emitter cannot reach it afterwards, so the move gives
  what the agnostic model gets from copying, at no cost. A body io reads
  is moved into the protocol's message, then into the model's `Call`, then
  into the model's store, and is copied nowhere on the way.
- **Copy at emission.** Data the model keeps and also sends goes out as a
  copy made when the reply is emitted (`lib::bytes::copy_of(&stored)`),
  because a later event in the same batch may change the stored value
  before the down pass runs. Do not reach for `Rc` or `Arc` to save the
  copy.
- **No borrow outlives a step.** No application type has a lifetime
  parameter (5.1).
- **Validate before allocating.** A length from a peer is checked against
  the limits before a buffer is allocated for the data it announces. Every
  `Box<[u8]>` is made by io for a demand the protocol layer validated, by
  `Writer::new(len)` for a length the service computed itself, or by
  `copy_of`.

### 6.4 The worst case

```
worst case =   Σ over entity kinds:  slab + capacity × the bytes each entity may hold
             + queues and tables:    their containers
             + the provided-buffer region
             + the model's stored-data limit
```

Each layer exports `fn worst_case(limits: &Limits) -> Option<u64>`
(checked arithmetic, `None` on overflow), and the shell refuses to start
when the sum exceeds the configured memory. An allocation failure is then
a bug or fragmentation, never load. A layer adds up what its containers
report, not `size_of` times a capacity: each lib container has its own
`worst_case(capacity)` (`Slab`, `Queue`, `List`, `Map`, `Set`,
`Deadlines`), which counts its bookkeeping too: a slab's slot tags and
free lists, the tree nodes of a map, a set or a deadline table. The
formula counts containers and payload bytes, not allocator overhead: leave
headroom, and measure the resident size under load before trusting it. The
simulator checks the formula at every iteration (section 11).

If the worst case forces the limits too low for a service, the fallback is
a byte budget for the large consumers, each kept in the layer that owns
them. io grants output *room* only within a global queued-output budget,
so pressure turns into the ordinary backpressure chain (section 7) rather
than a new kind of refusal; the model already answers "full" against its
stored-data limit. Do not add the budget before the worst case demands
it.

### 6.5 Rejected

- **`Rc<RefCell<T>>`, `Arc<Mutex<T>>`.** Reachability decides lifetime,
  which 5.2 forbids, and borrow errors move from compile time to runtime
  panics.
- **References in long-lived state, arenas with lifetimes** (`bumpalo`,
  `typed-arena`). Lifetime parameters spread to every type that touches
  them, and state stops being a plain value that can be snapshotted and
  compared. Slabs and boxes need none.
- **Byte pools fixed at startup.** See 6.2. Reconsider only if the
  allocator is measured to be the problem.
- **Custom allocators per layer.** The allocator API is unstable, and the
  service builds on stable Rust.
- **Shared immutable buffers (`Arc<[u8]>`) as the ownership system.** At
  most an optimisation, below.

### 6.6 Deferred optimisations

These keep application code unchanged:

- **Exact-size reads:** for a "fill N" demand, io allocates the `Box`, the
  kernel reads straight into it, and io moves it up. The copy out of io's
  receive buffers disappears and nothing above io changes.
- **Registered buffers** for the provided-buffer region and frequent sends.
- **Shared immutable buffers** (`Arc<[u8]>`, confined to io) for large
  values sent to many slow readers, or zero-copy relaying: measured first.
- **Kernel TLS.**
- **Batched submits:** the shell submits every few rounds, or when it is
  about to wait, rather than every round, which saves ring syscalls under
  pipelining at the cost of delaying each submission by up to that many
  rounds.

## 7. Flow control, admission and backpressure

**Limits are configured.** Each layer defines a `Limits` struct; the
shell's configuration record holds one per layer and hands each to its
layer as `&Env<Limits>`, so a simulation with tiny limits means something.
The limits are also the inputs of the worst case (6.4), and slab
capacities are the admission limits. The flow-control limits are: maximum
message size, cap on queued output, cap on unparsed input, concurrent
streams, accept batch, progress chunk and timeout.

**Refuse at the entrance, never in the middle.** Whatever the service
limits, it checks at the last point where saying no has no consequences:
accept for connections, request start for requests. A request refused
there leaves nothing half-done; a request abandoned in the middle leaves
partial state behind and a peer with half an answer.

**Each layer refuses at its own entrance.** An accepted socket the
protocol layer has no slot for is rejected; a request the service cannot
take on gets a busy response; a store that is full answers "full" as a
domain result.

**One request in flight per connection** beyond the protocol layer: the
next is parsed only after the previous response is queued. For multiplexed
protocols the unit is the stream, with a cap on concurrent streams.

**The backpressure chain,** for a client that sends faster than it reads:

1. The connection's queued output has a cap; sends do not complete, so it
   stays full.
2. The protocol layer asks for output room (queued output plus one
   worst-case response within the cap) **before parsing the next
   request**, and otherwise leaves input unparsed.
3. Unparsed input grows to its cap; then io stops receiving.
4. The socket buffer fills, the TCP window closes, the peer blocks.

Release runs the other way. Without the cap in step 1, one pipelining peer
that never reads turns the service's memory into unbounded queued output,
and the worst case of 6.4 is no longer a bound.

**Full duplex.** The output check gates the *next* request only. In a
relay the two directions have independent credit, or two peers that both
write before reading deadlock through it. Credit is a grant-and-consume
message; where two connections are coupled (a proxy), it flows through the
model, since only the model knows they are related.

**Progress deadlines.** A peer that holds resources without making
progress is closed. Progress is counted in chunks, by the peer: a read
demand met, a chunk of queued output drained. The service's own writes do
not count, or a peer that never reads is kept alive by the responses it
provokes; single bytes do not count, or a trickling peer lives forever.
The chunk size is therefore the minimum rate below which a peer is cut
off; many peers holding resources at exactly that rate are a matter for
per-peer limits. Whether the deadline runs is an exhaustive function of
the state, and one place arms and re-arms it (5.4), not every handler.

## 8. Protocols

- **Any protocol we design is sized:** a fixed header carrying every
  length. Parsing a header is a fixed-size decode with a `lib::Reader` into
  a plain struct; lengths are validated against the limits *before*
  anything is set aside for the body; the machine is header, body,
  dispatch.
- **Encoding is sized too:** compute the message's length,
  `Writer::new(len)`, write the fields, `finish()`. The length is the
  service's own, so the caller expects every write to fit (a write past
  the end is refused whole, writing nothing), and finishing short is an
  assertion.
- **Lengths are never trusted by construction.** No `[]` indexing and no
  `as` on anything a peer sent: the `Reader` returns `Option`, a narrowing
  is `u32::try_from`, and a failure is a framing error, not a panic.
- **Scanned framing** (delimiters, HTTP/1 heads, line protocols) is only
  for foreign protocols: a "scan to a delimiter, at most M bytes" demand,
  with the carry-over held by io.
- **Machines express demand, not buffer handling** (4.3). The byte source
  can then change (copying today, exact-size kernel reads later) without
  touching protocol or model code.
- **One request per delivery.** Not a loop that parses while enough bytes
  are buffered: one in flight, and flow control assumes it.
- **Nested formats use an explicit bounded stack** (`lib::Stack`), never
  recursion: the nesting depth is the peer's choice. JSON and friends will
  tempt recursive descent.
- **Unknown opcode with valid lengths:** refuse it with a status and skip
  the body, so an older server survives a newer client. Bad lengths are a
  framing error and close the connection.
- **Refusals are small and fixed-size,** so a service under pressure can
  still say no.

## 9. Time, timers and randomness

- **`now` is read once per iteration** by the shell, from
  `CLOCK_MONOTONIC`, and passed to every step as `env.now`, a `lib::Time`
  (nanoseconds in a `u64`). Spans are `lib::Duration`. `std::time::Instant`
  is not used: it is opaque, and a simulator cannot make one.
- **Each layer owns its deadline table** (`lib::Deadlines`, in the
  layer's state) for its own concerns: close deadlines in io, idle and
  handshake timeouts and call deadlines in protocol, backoff and session
  expiry in the model. The expiry goes to the layer that armed it because
  no other layer holds the table.
- **Timers are not ring operations.** The shell sets one ring timeout for
  the earliest deadline over all the layers (`svc.next_deadline()`). One
  ring operation per timer would make resetting an idle timeout on every
  read cost a cancel, a re-arm, two completions and a settling state.
- **Arm and cancel are synchronous calls on the layer's own table.** A
  stage fires its expired timers at one point, after its input events, so
  progress that arrived in the same iteration wins over a deadline that
  passed while the loop waited. Firing removes the timer before its
  handler runs, so a timer ends with exactly one of *fired* or
  *cancelled*, and timers never need settling. Cancelling a timer that has
  already fired is a stale handle, dropped silently.
- **Randomness comes from injected state:** a `lib::Rng` in each layer's
  state, seeded by the shell from `getrandom`, or by the simulator, which
  then owns both the clock and the random state.

## 10. The Rust subset

### 10.1 What enforces each rule

| Rule | How Rust holds it | Checked by |
|---|---|---|
| Steps make no syscalls, read no clock, spawn nothing, print nothing | step crates are `no_std`; those APIs do not exist there | compiler |
| No global or thread-local state | `static mut` needs `unsafe`; `thread_local!` is std; atomics and cells are disallowed | compiler, clippy |
| Output does not depend on hash seeds or addresses | no `HashMap` in `alloc`; no formatting | compiler, clippy |
| The model never sees an fd or a kernel error | the model crate does not depend on io | compiler |
| A step touches only its own layer | it receives `&mut` to its own state and nothing else | compiler |
| Configuration is read-only | `&Env<Limits>` | compiler |
| No borrow outlives a step; records hold no references | no lifetime parameters on application types | review (visible syntax) |
| Layers share nothing by reference | payloads are owned and moved | compiler |
| A buffer the kernel holds is touched by nothing else | the `Box` is moved into the op slot | compiler; the ring adapter's `unsafe` contract |
| A reply is sent at most once | `ReplyTo` is not `Copy` or `Clone`; replying consumes it | compiler |
| A reply is sent at least once; one terminal event per request | | simulator |
| Handles are typed | `Id<T>` | compiler |
| A stale handle is detected | the generation | runtime |
| A state and what it holds are one value | enums with data | compiler |
| Matches are exhaustive, with no catch-all | `match`; `wildcard_enum_match_arm`; `matches!` disallowed | compiler, clippy; review for `if let` and tuples |
| Statuses are not ignored | `#[must_use]`, `unused_must_use`, `let_underscore_must_use` | compiler, clippy |
| Arithmetic and narrowing are checked | `arithmetic_side_effects`, `as_conversions`; `overflow-checks` traps the rest | clippy, runtime |
| Bytes go through bounds-checked cursors | `indexing_slicing`; `Reader`, `Writer` | clippy |
| Fail-stop | `panic = "abort"` | build profile |
| Nothing closes or frees in a destructor | no `Drop` impls; no `OwnedFd`, `File`, `TcpStream` | review |
| `unsafe` is confined | `forbid(unsafe_code)` everywhere but the ring adapter | compiler |
| Execution is bounded | no `loop` or `while` in step code; no recursion | review |
| State is plain | no closures, function pointers or `dyn` | review |
| No leaks; ownership is a tree | | simulator |

### 10.2 What is in

Step code (io, protocol, model, service) uses:

- `struct` with named fields; tuple structs only as one-field newtypes
  (`Fd(i32)`).
- `enum` with data, for states, events, requests, statuses and errors.
- `match`, exhaustive; `if` and `else`; `if let` and `let … else` on
  `Option` and `Result` only.
- `for` over a range, a slice or a lib container.
- Free functions, inherent `impl` blocks, `const` items, modules.
- `&` and `&mut` in parameters, locals and return values, with elided
  lifetimes, written `'_` where a type borrows (`Reader<'_>`).
- Moves; `Copy` for handles, tokens and small plain values.
- `?` on `Option` and `Result`, with the same error type on both sides (no
  `From` conversions).
- Integers, `bool`, arrays, slices, `Option`, `Result`, `Box`, `BTreeMap`,
  `BTreeSet`, `core::mem::replace`.
- `#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]`
  and no other derive. Entity state does not derive `Clone`: an entity
  exists once.
- `assert!`, `unreachable!("why")`, `expect("the invariant relied on")`.
- From lib: `Id<T>`, `Slab<T>`, `Token`, `ReplyTo`, `Queue<T>`,
  `List<T>`, `Map<K, V>` and `Set<K>` (bounded and ordered, over
  B-trees), `Stack<T>`, `ByteRing`, `Reader`, `Writer` (sized),
  `bytes::copy_of`, the byte search `bytes::find`, `find_from` and
  `count` (linear time, no allocation), `Deadlines`, `Time`, `Duration`,
  `Rng`, `Env<L>`.

lib uses the same, plus generic types, lifetime parameters on its cursors,
hand-written impls of std traits for its own types, `Vec` inside its
containers, and `while` loops bounded by a container's capacity. It is
written once and tested hard. Application code does not hand-roll data
structures: what is missing goes into lib.

The shell, the simulator and tests are ordinary Rust, within the habits of
12.1, with `unsafe` confined to the ring adapter.

### 10.3 What is out

| Out of step code | Why | Checked by |
|---|---|---|
| `std` | syscalls, clock, threads, printing, `HashMap` | compiler (`no_std`) |
| `unsafe` | one audited place: the ring adapter | compiler (`forbid(unsafe_code)`) |
| `async`, `.await` | the loop schedules; state hides in generated futures, neither total nor snapshottable | review |
| closures, function pointers | captured state, and calls the reader cannot follow | review |
| `dyn Trait`, `impl Trait` | dynamic dispatch, hidden types | review; clippy `impl_trait_in_params` |
| defining traits, generic items, hand-written trait impls | one way to call code, on concrete types | review |
| lifetime parameters on types, references in fields | a stored borrow; state stops being a plain value | review |
| `Rc`, `Arc`, `Cell`, `RefCell`, atomics | shared ownership, interior mutability, global state | clippy `disallowed_types` |
| `static`, `thread_local!` | global state | review; compiler (`no_std`) |
| `Vec`, `VecDeque`, `String`, `vec!`, `format!` | growth without a check; formatting | clippy `disallowed_types`, `disallowed_macros` |
| `impl Drop`, `Deref`, operator overloading, `Index` | hidden calls and hidden effects | review |
| `macro_rules!`, procedural macros beyond the derives | code the reader cannot see | review |
| `loop`, `while`, recursion | unbounded execution | review |
| a `_` arm on an own enum, `matches!` | a new case builds silently | clippy `wildcard_enum_match_arm`, `disallowed_macros` |
| `if let` on an own enum, a tuple as scrutinee | the same, out of the lint's sight | review |
| `#[non_exhaustive]` on own types | forces a `_` arm | review |
| `as` casts | silent truncation | clippy `as_conversions` |
| unchecked `+ - * /` on integers | a silent wrap, or a panic a peer can trigger | clippy `arithmetic_side_effects` |
| `[]` indexing and slicing | a panic a peer can trigger | clippy `indexing_slicing` |
| floats | no total order, nothing in this domain needs them | clippy `float_arithmetic` |
| `unwrap()` | an assertion without its reason | clippy `unwrap_used` |
| `let _ =` on a `#[must_use]` value | a silently ignored status | clippy `let_underscore_must_use` |
| `mem::forget`, `todo!`, `unimplemented!` | a leak; unfinished cells | clippy |

Integers go through `checked_*`, or through `wrapping_*` and
`saturating_*` where that is the intent (generations wrap on purpose).
`overflow-checks = true` in every profile traps whatever the lint does not
see, in lib and the shell.

### 10.4 Configuration

```toml
# Cargo.toml (workspace)
[profile.dev]
panic = "abort"
overflow-checks = true

[profile.release]
panic = "abort"
overflow-checks = true

[workspace.lints.rust]
unsafe_code = "deny"                  # forbid in every crate root but the shell's
elided_lifetimes_in_paths = "deny"    # a type that borrows says so: Reader<'_>
unused_must_use = "deny"

[workspace.lints.clippy]
wildcard_enum_match_arm = "deny"
indexing_slicing = "deny"
arithmetic_side_effects = "deny"
as_conversions = "deny"
float_arithmetic = "deny"
unwrap_used = "deny"
let_underscore_must_use = "deny"
mem_forget = "deny"
todo = "deny"
unimplemented = "deny"
impl_trait_in_params = "deny"
allow_attributes_without_reason = "deny"
undocumented_unsafe_blocks = "deny"
multiple_unsafe_ops_per_block = "deny"
disallowed_types = "deny"
disallowed_macros = "deny"
```

```toml
# clippy.toml (workspace root: applies to the step crates)
disallowed-types = [
  { path = "alloc::vec::Vec",              reason = "Box<[u8]> for bytes, lib::Queue for sequences" },
  { path = "alloc::collections::VecDeque", reason = "lib::Queue" },
  { path = "alloc::string::String",        reason = "bytes are Box<[u8]>" },
  { path = "alloc::rc::Rc",                reason = "one owner" },
  { path = "alloc::sync::Arc",             reason = "one owner" },
  { path = "core::cell::Cell",             reason = "no interior mutability" },
  { path = "core::cell::RefCell",          reason = "no interior mutability" },
  { path = "core::sync::atomic::AtomicU64", reason = "no global state" },
  # ... and every other atomic type
]
disallowed-macros = [
  { path = "alloc::format", reason = "no formatting in step code" },
  { path = "alloc::vec",    reason = "no Vec in step code" },
  { path = "core::matches", reason = "a catch-all match in disguise" },
]
```

- Each step crate's root starts with `#![cfg_attr(not(test), no_std)]`
  (unit tests get std) and `#![forbid(unsafe_code)]`, and lib's does the
  same. The shell allows `unsafe` in its ring adapter module only, with
  the reason stated.
- lib and the shell have their own `clippy.toml`; clippy reads the one
  nearest the crate, so lib may use `Vec` inside its containers.
- Warnings are errors: `cargo clippy --all-targets -- -D warnings` in CI.
- Stable Rust, edition 2024. Nightly is used only for the fuzz targets.

### 10.5 Discipline

- **Errors are values.** Expected outcomes (busy, full, not found,
  refused) are enum variants, never panics. Every fallible operation
  returns a `Result`, an `Option` or a `#[must_use]` status.
- **One side effect per statement.**
- **Assertions are fail-stop** and are for the service's own bugs, never
  for inputs a peer controls. `expect` names the invariant it relies on.
- **Runtime checks stay on in production:** bounds checks, overflow
  checks, assertions. Safe Rust has no undefined behaviour left to trap;
  the ring adapter, the one place that could have some, is tested against
  the real kernel.

## 11. Testing

- **Model worlds.** Each model, and each sub-model (4.5), runs in a world
  of its own: the model, the clock, the seeds, and fakes standing in for
  its neighbours (another party's model, a filesystem), with no protocol
  and no io. A fake shares no domain types with what it stands in for; the
  world translates between them, as a protocol layer would. Model worlds
  come first and test behaviour; the simulator below comes with the lower
  layers and tests mechanics.
- **Deterministic simulation.** The simulator drives `service::iterate`,
  the function the shell runs, and plays everything around it: the ring
  (it reads submissions, writes into op buffers through io's API, and
  produces completions), the clock and the seeds. It runs with tiny limits
  (slabs of capacity 2) and injects cancellation and timeout in every
  state, completions after cancel, refusal at every admission point, and
  short reads and short writes.
- **Universal invariants,** checked by the simulator: no live entities at
  quiescence (every slab empty); ownership is a tree with no orphans; one
  terminal event per request; every `ReplyTo` answered.
- **Transition coverage.** Each cell is a handler function, so function
  coverage of the handlers (`cargo llvm-cov` over a simulation run) is the
  list of transitions exercised and of those never reached.
- **Memory.** A counting `#[global_allocator]` in the simulator records
  the peak of live heap bytes, and the simulator asserts at every
  iteration that it stays within the worst case of 6.4.
- **Fuzzing** each protocol machine alone with `cargo fuzz`, feeding
  `Bytes` events under every demand, and each step function with recorded
  event sequences.
- **Replay:** a recorded run replays to the same state. State types derive
  `Hash`, and lib's fixed-key hasher gives a field-wise digest of the
  logical state, independent of its layout in memory.

## 12. Starting a new service

Before code:

1. The wire protocol, sized, with every length and limit.
2. The limits of each layer, and the worst case they imply (6.4).
3. The entities of each layer, who owns each, and how they bind.
4. The state machines: states, what each holds, the total transition
   table, demands and deadlines per state.

Then, in order: lib (handles, slabs, queues, cursors, deadline table);
the model, unit-tested by feeding events and inspecting requests, and run
in model worlds (section 11); the protocol layer, fuzzed alone; the io
layer, the service and the shell last, with the simulator standing in for
the kernel until then.

```
Cargo.toml     workspace: profiles and lints
clippy.toml    disallowed types and macros for the step crates
lib/           Id, Slab, Token, ReplyTo, Queue, List, Map, Set, Stack, ByteRing,
               Reader, Writer, bytes, Deadlines, Time, Duration, Rng, Env
io/            io::up, io::down: sockets, operations, transit memory
protocol/      protocol::up, protocol::down: machines, codecs
model/         model::step: domain machines, with any sub-models below it (4.5)
service/       the three layers wired together; service::iterate
shell/         main, the ring adapter (the only unsafe), clock, seed, spawn
sim/           simulated ring and clock, fault schedules, counting allocator
fuzz/          one target per protocol machine
```

### 12.1 Habits to avoid

| Habit | Why it is wrong | Instead |
|---|---|---|
| An `async fn` handler, a runtime | the runtime schedules instead of the loop; state hides in generated futures | an enum state machine driven by the loop |
| Registering a `Box<dyn FnMut>` with the loop | hidden control flow, captured state | events in, requests out |
| `Rc<RefCell<T>>` to share an entity | reachability decides lifetime; borrow errors become panics | one owner; everyone else holds an `Id<T>` |
| A struct with a lifetime parameter to hold `&Conn` | a stored borrow drags lifetimes through every type | store the `Id<T>`; look it up when needed |
| `&mut self` methods on the whole layer | the borrow checker cannot split the borrow; `clone` or `RefCell` follow | free functions over the fields they touch |
| `.clone()` on entity state to calm the borrow checker | now the entity exists twice | copy the handle out, end the borrow, look it up again |
| `Instant::now()` or `rand` in a step | not replayable | `env.now`, the layer's `Rng` |
| `HashMap` | iteration order depends on a random seed | `BTreeMap` |
| `unwrap()` or `[]` on something a peer sent | a remote crash | `Reader`, `try_from`, a framing error |
| `as u32` on a length | silent truncation | `u32::try_from`, and refuse |
| `impl Drop` that closes or frees | a hidden effect outside the lifecycle | request *close*; reclaim on *closed* |
| `OwnedFd`, `File`, `TcpStream` in io | closes on drop, outside the ring | an `Fd(i32)`, closed by a ring `close` |
| Retiring a connection on end of stream | *closed* has not arrived; the kernel may hold its buffers | go to *closing*; retire on *closed* |
| A `_` arm, `matches!` or `if let` over states or events | a new case builds silently | an exhaustive `match` |
| `match (state, event)` | a `_` in a tuple escapes the lint | match one, then the other |
| A placeholder that is a live state in `mem::replace` | a forgotten assignment leaves a plausible state | the terminal state, with the match as the right-hand side |
| Queuing output without a cap | a peer that never reads grows it without bound | cap queued output; check room before parsing |
| Parsing every request available in one loop | one in flight; flow control overshoots | one request per delivery |
| Recursive descent on peer input | the peer chooses the depth; a remote crash | `lib::Stack` |
| `Vec::push` while reading a body | the peer chooses the size | validate the length, then demand exactly it |
| Sharing stored data with a reply through `Arc` | the kernel would hold model memory | copy at emission |
| Asserting on input a peer can send | a remote crash | handle the cell |
| Re-arming the idle timeout by hand in every handler | one forgotten re-arm closes a connection mid-body | derive the deadline from the state, in one place |
| One ring timeout per timer | cancel races, settling, two completions per reset | the layer's `Deadlines` |

## 13. Departures and open questions

Where this document departs from the agnostic model:

1. **Timers live in each layer,** not in io (section 9). Arm and cancel
   never cross a boundary, and the expiry reaches the layer that armed it
   without routing; in Rust, io could not name the model's queue anyway.
   io's vocabulary loses *arm timer* and *cancel timer*.
2. **io delivers the demanded bytes in the event,** as an owned
   `Box<[u8]>`, instead of the protocol layer reading io's buffers through
   a cursor in place (4.3). Every boundary record is then self-contained,
   and exact-size reads later become a move.
3. **A move replaces the copy at every boundary** (6.3). Copy at emission
   still applies to data the model keeps.
4. **A step returns nothing** (section 3). Each entry point declares
   `MAX_OUT` and the loop reserves the room, so there is no status left to
   return.
5. **The random generator lives in each layer's state,** and the limits
   reach the step read-only through `&Env<Limits>`.
6. **One opaque `Token` crosses every boundary** (4.2), and reply tokens
   are affine (`ReplyTo`).
7. **The memory strategy is chosen:** counted entities and owned bytes,
   with the worst case checked at startup (section 6).

Open questions:

- **Affine owner handles.** An `Owned<T>`, neither `Copy` nor `Clone`,
  returned by `insert` and consumed by `retire`, would let only an
  entity's owner end it, checked by the compiler. It is awkward for
  top-level entities (who holds the `Owned<Conn>` of an accepted
  connection?). Decide after the first service.
- **A checker for the review-only rules.** No `async`, closures, `loop` or
  `while`, recursion, tuple scrutinees, or trait and generic definitions
  in step code. Without closures, `dyn` or traits in step code every call
  is static, so a small `syn`-based check could find recursion too. Write
  it if review proves not to be enough.
- **Kinds in tokens.** If decoding a token as the wrong kind turns out to
  be a real mistake, give `Token` a kind tag that `from_token` checks.
- **The byte budget** of 6.4, for a service whose worst case is too large
  to provision.
