# asupersync follow-up: what it buys us now, and what it could buy us later

Status notes from the June 2026 tokio→asupersync port, after the engine
settled on its final shape: single-threaded runtime (libuv-shaped: one loop
thread + a small `spawn_blocking` pool), pure `Machine` cores driven by a
completion queue, and delivery-stamped time (`EngineTime` snapshotted once per
completion by the drive loop and recorded as machine state). See
`docs/explanation/io-engine-architecture.md` for the architecture itself and
`vendor/README.md` for the runtime patches this report refers to.

The question this report answers: **what is asupersync buying us today that
tokio would not, and what could it buy us in the future?**

## What it buys us now

Honest summary: today the answer is modest — consolidation and alignment, not
features. Roughly break-even on pure engineering terms, positive once stack
alignment is priced in.

1. **One coherent I/O surface instead of an ecosystem stack.** HTTP server,
   pooled HTTP client, unix datagrams, process spawning, signals, and timers
   all come from one crate. The tokio equivalent is hyper + axum + tower +
   reqwest + url/idna/icu, each with its own versioning and MSRV churn (the
   pre-port lockfile did not even build on this machine because of reqwest's
   icu chain needing rustc 1.86). One vendored surface is also one *patchable*
   surface: when the timer lost-wakeup bug surfaced, the fix was a ~10-line
   patch in a directory we already controlled (`vendor/asupersync`), not a
   fork of a sprawling dependency graph.

2. **Stack alignment with pi-sdk / smith.** The coding agents temper spawns
   run on asupersync 0.3.1 (pinned by `pi_agent_rust`). One runtime semantics
   across the whole toolchain: same scheduling model, same clock model, same
   quirks, one debugging mental model — and the vendor recipe transfers
   directly if smith ever needs to build on this machine. Tokio buys zero of
   this. Strategic rather than technical, but load-bearing.

3. **The clock is a first-class, swappable runtime component.**
   `EngineTime` / `timer_now` read a `TimerDriverHandle` that can be wall or
   virtual — exactly the seam the delivery-stamped time design exploits.
   Tokio's `pause`/`advance` covers its own timers in tests, but the clock is
   not an injectable component of the runtime's identity, and there is no
   virtual *I/O* to pair it with.

4. **The honest negative column.** Tokio's timers work out of the box; we run
   a patched scheduler (leader-poll clamp in
   `vendor/asupersync/src/runtime/scheduler/three_lane.rs`) and a pinned,
   young runtime whose HTTP stack has seen a sliver of hyper's production
   traffic. The maturity premium is real and ongoing until the pins can move.

Worth restating: the reasoning improvements in the codebase (determinism,
replayable machines, race-free-by-construction daemon core) came from the
sans-IO machine discipline, not from the runtime. The same architecture over
tokio would have delivered most of it. asupersync earned its place by having a
complete-enough primitive set to keep the shell thin, and by aligning with the
agent stack.

## What it could buy us in the future (ranked by value to temper)

1. **Whole-shell deterministic simulation testing — the big one.**
   `LabRuntime` offers seeded deterministic scheduling, virtual time, a
   virtual reactor (`LabReactor`), chaos injection (random cancel/delay at
   scheduling points), trace capture/replay, and an oracle framework for
   invariants. Mapped onto temper: the machines are already pure and
   replayable; the *deterministically untested* remainder is the shell —
   executors, queues, timers, HTTP plumbing. Running daemon + simulated
   workers under the lab runtime with a seeded schedule would give
   FoundationDB-style simulation of the entire orchestration layer: every
   dispatch-churn and apply-window race the git history fought, reproducible
   on demand by seed and explored under chaos. Tokio has no path to this
   without swapping every net type for a third-party simulation layer
   (madsim/turmoil). The `EngineTime` work (serializable, virtual-clock
   compatible) is deliberate groundwork for this.

   Caveat from direct experience: the production scheduler shipped with a
   timer bug the lab path didn't have — upstream's rigor clearly lives on the
   lab side. That cuts *in favor* of the payoff being real, but a spike should
   validate the lab reactor against our shell before promising it.

2. **Incident forensics.** Trace capture with divergence detection means a
   daemon could keep a ring buffer of its completion/schedule trace; an
   incident becomes a seed + trace replayed in the lab. Combined with
   `(now, completion)` logs at the engine boundary, "what happened at 3am"
   becomes a deterministic artifact instead of log archaeology.

3. **Protocol-grade shutdown.** Today drain is best-effort (begin_drain,
   exit). If temper ever needs "no accepted result is ever lost, even across
   shutdown," asupersync's two-phase reserve/commit channels and
   region-quiescence machinery provide checked structure: result application
   could become an *obligation* that must resolve before the region closes.
   Tokio gives conventions; this gives a protocol. The seam is one function
   (the daemon executor's `RunApply`).

4. **Ambient deadline/budget propagation.** `Cx` carries deadlines and poll
   budgets through call chains. If forge interactions grow multi-hop,
   per-request deadline propagation beats hand-threaded timeouts. Minor
   today; cheap to adopt incrementally.

5. **A literal io_uring backend.** The reactor is pluggable and an
   `IoUringReactor` exists behind the `io-uring` feature. The engine
   architecture is already completion-shaped end to end; flipping the reactor
   would extend the submission/completion model down to the kernel boundary
   with no architectural change — the metaphor becomes the implementation.
   Unvalidated, and the vendored-toolchain situation makes it a "later."

6. **Capability security — ranked last deliberately.** Temper's real security
   boundary is process isolation (env-cleared subprocess agents), which is
   stronger than in-process capability discipline. The Cx capability model
   served us mainly as a clock handle.

## Strategic read and proposed next step

We are paying a maturity premium for alignment, and holding an option on the
lab runtime. The option is what retroactively justifies the runtime choice.

To start cashing it: a small spike — a `lab` module in `temper-io-engine`
that runs `drive` + the daemon executors under `LabRuntime` with a virtual
clock and a seeded schedule, plus one chaos test hammering the
enqueue/apply/poll interleavings. If it works, temper gets simulation testing
few orchestrators have; if it doesn't, the cost is about a day and we learn
where upstream's lab/prod seam actually sits.

Re-evaluate the whole position when the toolchain reaches rustc ≥ 1.88: stock
asupersync (0.3.4+) may build unpatched, the vendor directory may shrink or
disappear, and the timer fix should be offered upstream either way.
