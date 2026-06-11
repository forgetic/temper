# The I/O engine: functional core, imperative shell

Temper's services run on `temper-io-engine`, an io_uring-style completion
engine layered over the [asupersync](https://crates.io/crates/asupersync)
runtime. The design splits every service into two halves:

- a **functional core** — a deterministic state machine implementing
  `temper_io_engine::Machine`:

  ```text
  (state, now, <io-event-completion>) -> (new state, [<io-event-request>])
  ```

  No sockets, no clocks, no spawning, no side effects. Time only enters as
  data: the drive loop snapshots the runtime's monotonic clock exactly once
  per delivery (`EngineTime`, nanoseconds since runtime start) and hands it to
  the transition, which records it as a state field.

- an **imperative shell** — a `temper_io_engine::Executor` that performs each
  request on the asupersync runtime (HTTP responses, timers, child processes,
  forge calls, wake scans) and eventually submits the outcome back to the
  completion queue.

The arrow loops, exactly like an io_uring submission/completion queue pair:

```text
  <io-event-completion> ──▶ Machine::on_completion (pure)
           ▲                          │
           │                          ▼
    Executor (asupersync I/O) ◀── <io-event-request>
```

`temper_io_engine::drive` is the only loop: receive one completion, run the
pure transition, hand each produced request to the executor. The executor
never calls back into the machine, so the core stays single-owner and
deterministic — replaying a recorded completion sequence into a fresh machine
reproduces the exact same requests, with no runtime involved. That is also how
machines are unit-tested (see `crates/temper-daemon/tests/transport.rs` for
the service-level tests and `crates/temper-io-engine/tests/engine_loop.rs` for
the reference pattern).

## Where the pattern shows up

| service | functional core | shell executors |
|---|---|---|
| `temper-daemon` | `DaemonMachine`: worker protocol, long-poll waiters, apply windows, webhook verification | HTTP responder, poll-deadline timers, result appliers, wake scans |
| backstops | `CadenceMachine`: tick → wait cadence → tick | one scan executed per `RunTick` request |
| `temper-worker` | scan/decision logic in `temper-runner` (already pure) | wake-socket waits, poll timers, subprocess decisions |
| `temper-interaction` | conversation/session logic | one `ProcessCall` per responder turn |
| forge backends | request building + response interpretation (pure) | one pooled HTTP exchange per `execute` |

## Boundary rules

1. Machines never perform I/O, read clocks, or block. Anything they want done
   leaves as a request; anything they learn arrives as a completion.
2. Executors never decide. They perform exactly the requested operation and
   submit the result; policy (retries, dispositions, routing) lives in the
   machine.
3. Time is data. The drive loop snapshots the runtime clock once per
   delivery and passes it into the transition; machines keep it as a state
   field (`self.now`) and never call `Instant::now()`. Because one clock is
   read at one place, the machine observes time monotonically regardless of
   which shell task produced each completion — and `EngineTime` is a plain
   serializable value, so recorded completion sequences replay exactly.
   Timers are requested (`StartPollTimer { delay }`), never slept.
4. Capabilities are data. An `HttpResponder` is an opaque single-use token a
   machine holds in its state and returns inside a respond request; only the
   shell consumes it.

## The runtime underneath

The shell runs on asupersync 0.3.1 (vendored; see `vendor/README.md` for the
rustc-1.85 compatibility patches and a timer lost-wakeup fix), configured
**single-threaded** (libuv-shaped): one loop thread runs every task, so while
a machine transition executes nothing else in the engine progresses —
concurrency without parallelism. Blocking work goes through `spawn_blocking`'s
separate small pool; never block the loop thread. If a service ever saturates
its loop, partition it into more machines (shard by repo) rather than
re-enabling worker parallelism — the serialized core wouldn't benefit from
threads. Engine binaries boot via `temper_io_engine::block_on`, which runs the
main future as a task so it holds an ambient capability context (`Cx`);
`#[test]` bodies use the same entry point in place of the old
`#[tokio::test]`.
