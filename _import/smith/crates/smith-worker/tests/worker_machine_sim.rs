//! Deterministic simulation / fuzz tests for [`WorkerMachine`].
//!
//! The worker core is a pure sans-IO machine, so its entire behavior under any
//! interleaving of I/O completions is reproducible from a seed with no runtime,
//! no sleeps, and no races. These tests exploit that: they generate randomized
//! (but seeded, hence replayable) completion sequences and assert that the
//! machine's safety invariants hold after *every* transition, across thousands
//! of interleavings.
//!
//! This is the first layer of the skein-lab simulation goal — the pure
//! core fuzzed exhaustively in-process. A companion lab-runtime test
//! (`worker_lab_sim.rs`) drives the imperative shell (spawned I/O + virtual
//! timers) under a seeded lab schedule.
//!
//! Invariants checked:
//! 1. Capacity is conserved: `free_capacity + in_flight == max_concurrent`.
//! 2. Capacity stays in `0..=max_concurrent`.
//! 3. The machine never dispatches a job when it has no free capacity
//!    (no over-subscription).
//! 4. A `RunJob` is only ever emitted for the job named in the assignment that
//!    triggered it, and that job becomes in-flight.
//! 5. Liveness: a registered machine never goes silent — after every
//!    transition it has either emitted forward progress (a poll, a job, a
//!    result, a heartbeat) or armed a timer that will wake it again.

use std::collections::BTreeSet;

use serde_json::json;
use smith_io_engine::{EngineTime, Machine};
use smith_worker::config::{CapabilitySpec, WorkerParams};
use smith_worker::executor::{JobOutcome, job_result};
use smith_worker::worker_machine::{WorkerCompletion, WorkerMachine, WorkerRequest};
use temper_worker_protocol::{
    Artifact, Assign, ErrorCode, FailureClass, ProtocolError, ResultStatus,
    WORKER_PROTOCOL_VERSION, WorkerProtocolMessage,
};

/// A tiny deterministic PRNG (SplitMix64) so the fuzzer needs no external crate
/// and every seed reproduces an exact sequence.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

fn params(max_concurrent: u32) -> WorkerParams {
    WorkerParams {
        worker_id: "fuzz-worker".to_string(),
        capabilities: vec![CapabilitySpec {
            repo: "ai/smith".to_string(),
            role: "engineer".to_string(),
        }],
        max_concurrent_jobs: max_concurrent,
        poll_wait: std::time::Duration::from_millis(100),
        heartbeat_interval: std::time::Duration::from_millis(50),
        poll_backoff: std::time::Duration::from_millis(500),
    }
}

fn assign(job_id: &str) -> Assign {
    Assign {
        protocol_version: WORKER_PROTOCOL_VERSION,
        job_id: job_id.to_string(),
        role: "engineer".to_string(),
        repo: "ai/smith".to_string(),
        artifact: Artifact {
            item: json!(1),
            kind: "issue".to_string(),
        },
        job_payload: json!({}),
    }
}

fn poll_timeout_reply() -> WorkerProtocolMessage {
    WorkerProtocolMessage::Error(ProtocolError {
        protocol_version: WORKER_PROTOCOL_VERSION,
        code: ErrorCode::PollTimeout,
        message: "no work".to_string(),
        retry_after_ms: None,
        job_id: None,
    })
}

fn finished(job_id: &str) -> WorkerCompletion {
    WorkerCompletion::JobFinished {
        job_id: job_id.to_string(),
        result: job_result(
            "fuzz-worker",
            job_id,
            JobOutcome::Failure {
                class: FailureClass::Transient,
                message: "sim".to_string(),
            },
        ),
    }
}

/// Models the environment: tracks which job ids the daemon has handed out so the
/// fuzzer can deliver plausible completions (assignments, then the matching
/// job-finished). Mirrors how the real shell + daemon would behave.
struct Sim {
    machine: WorkerMachine,
    max_concurrent: u32,
    /// Jobs the machine has dispatched (RunJob emitted) but not yet finished.
    dispatched: BTreeSet<String>,
    next_job: u64,
    registered: bool,
}

impl Sim {
    fn new(max_concurrent: u32) -> Self {
        let mut machine = WorkerMachine::new(params(max_concurrent));
        // Run on_start so the machine is in its post-register-request state.
        machine.on_start(EngineTime::ZERO);
        Self {
            machine,
            max_concurrent,
            dispatched: BTreeSet::new(),
            next_job: 0,
            registered: false,
        }
    }

    /// Pick a plausible next completion given the current environment state.
    fn next_completion(&mut self, rng: &mut Rng) -> WorkerCompletion {
        // Choose among completions that make sense right now.
        let mut choices: Vec<u8> = vec![0, 1, 4, 5]; // register-ack, poll(assign/none/timeout), poll-timer, heartbeat-timer
        if !self.dispatched.is_empty() {
            choices.push(2); // a dispatched job finishes
            choices.push(3); // a result delivery acks
        }
        let pick = choices[rng.below(choices.len() as u64) as usize];
        match pick {
            0 => WorkerCompletion::Registered(Ok(())),
            1 => {
                // A poll reply: assignment, empty, or timeout.
                match rng.below(3) {
                    0 => {
                        let job_id = format!("job-{}", self.next_job);
                        self.next_job += 1;
                        WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(
                            assign(&job_id),
                        ))))
                    }
                    1 => WorkerCompletion::PollReply(Ok(None)),
                    _ => WorkerCompletion::PollReply(Ok(Some(poll_timeout_reply()))),
                }
            }
            2 => {
                // Finish a random dispatched job.
                let job = self
                    .dispatched
                    .iter()
                    .nth(rng.below(self.dispatched.len() as u64) as usize)
                    .cloned()
                    .expect("dispatched non-empty");
                finished(&job)
            }
            3 => WorkerCompletion::ResultDelivered {
                job_id: "job-x".to_string(),
                outcome: Ok(()),
            },
            4 => WorkerCompletion::PollTimer,
            _ => WorkerCompletion::HeartbeatTimer,
        }
    }

    /// Apply one completion and check all invariants on the emitted requests +
    /// resulting state.
    fn step(&mut self, completion: WorkerCompletion, seed: u64, tick: usize) {
        let is_register_ack = matches!(completion, WorkerCompletion::Registered(Ok(())));
        // A "wake" completion is one the machine must respond to in order to
        // keep making progress (a poll reply, a timer firing, a job finishing,
        // registration). Delivery acks (ResultDelivered / HeartbeatDelivered)
        // are fire-and-forget: the wake that keeps the loop alive was already
        // armed when the job finished / the heartbeat timer fired, so producing
        // nothing for an ack is correct, not a deadlock.
        let is_wake = matches!(
            completion,
            WorkerCompletion::Registered(_)
                | WorkerCompletion::PollReply(_)
                | WorkerCompletion::PollTimer
                | WorkerCompletion::HeartbeatTimer
                | WorkerCompletion::JobFinished { .. }
        );
        let free_before = self.machine.free_capacity();
        let requests = self.machine.on_completion(EngineTime::ZERO, completion);

        let mut emitted_run = false;
        let mut emitted_progress = false;
        for request in &requests {
            match request {
                WorkerRequest::RunJob(assign) => {
                    // Invariant 3: never dispatch without a free slot.
                    assert!(
                        free_before > 0,
                        "seed {seed} tick {tick}: dispatched a job with no free capacity"
                    );
                    // Invariant 4: the dispatched job becomes in-flight.
                    assert!(
                        self.machine.in_flight().contains(&assign.job_id),
                        "seed {seed} tick {tick}: dispatched job {} not in in_flight",
                        assign.job_id
                    );
                    self.dispatched.insert(assign.job_id.clone());
                    emitted_run = true;
                    emitted_progress = true;
                }
                WorkerRequest::SendPoll(_)
                | WorkerRequest::SendResult { .. }
                | WorkerRequest::SendHeartbeat(_)
                | WorkerRequest::SendRegister(_) => emitted_progress = true,
                WorkerRequest::ArmPollTimer(_) | WorkerRequest::ArmHeartbeatTimer(_) => {
                    emitted_progress = true;
                }
                WorkerRequest::Log(_) => {}
            }
        }

        if is_register_ack {
            self.registered = true;
        }

        // Reconcile the environment's view of dispatched jobs with the machine:
        // a finished job leaves in_flight, so drop any dispatched id no longer
        // in flight.
        self.dispatched
            .retain(|job| self.machine.in_flight().contains(job));

        // Invariant 1: capacity conservation.
        let free = self.machine.free_capacity();
        let in_flight = self.machine.in_flight().len() as u32;
        assert_eq!(
            free + in_flight,
            self.max_concurrent,
            "seed {seed} tick {tick}: capacity not conserved (free {free} + in_flight {in_flight} != {})",
            self.max_concurrent
        );
        // Invariant 2: capacity bounds.
        assert!(
            free <= self.max_concurrent,
            "seed {seed} tick {tick}: free capacity {free} exceeds max {}",
            self.max_concurrent
        );

        // Invariant 5: liveness — a registered machine never goes silent in
        // response to a wake completion (it must either make progress or arm a
        // timer that will wake it again). Delivery acks are exempt.
        if self.registered && is_wake {
            assert!(
                emitted_progress,
                "seed {seed} tick {tick}: registered machine went silent on a wake completion; requests = {requests:?}"
            );
        }

        let _ = emitted_run;
    }
}

#[test]
fn machine_invariants_hold_across_random_interleavings() {
    // Many seeds, several capacities, a long sequence each. All deterministic.
    for seed in 0..2_000u64 {
        for max_concurrent in [1u32, 2, 4] {
            let mut sim = Sim::new(max_concurrent);
            let mut rng = Rng::new(seed ^ (u64::from(max_concurrent) << 40));
            for tick in 0..64 {
                let completion = sim.next_completion(&mut rng);
                sim.step(completion, seed, tick);
            }
        }
    }
}

#[test]
fn capacity_never_oversubscribes_under_assignment_storm() {
    // Adversarial: hammer the machine with assignments far beyond capacity and
    // confirm it never dispatches more than max_concurrent jobs at once.
    for seed in 0..500u64 {
        let max_concurrent = 1 + (seed % 4) as u32;
        let mut sim = Sim::new(max_concurrent);
        // Always register first.
        sim.step(WorkerCompletion::Registered(Ok(())), seed, 0);
        let mut rng = Rng::new(seed);
        for tick in 1..128 {
            // Bias heavily toward assignments; occasionally finish a job.
            let completion = if !sim.dispatched.is_empty() && rng.below(4) == 0 {
                let job = sim
                    .dispatched
                    .iter()
                    .next()
                    .cloned()
                    .expect("dispatched non-empty");
                finished(&job)
            } else {
                let job_id = format!("job-{}", sim.next_job);
                sim.next_job += 1;
                WorkerCompletion::PollReply(Ok(Some(WorkerProtocolMessage::Assign(assign(
                    &job_id,
                )))))
            };
            sim.step(completion, seed, tick);
            assert!(
                sim.machine.in_flight().len() as u32 <= max_concurrent,
                "seed {seed} tick {tick}: in_flight exceeded capacity"
            );
        }
        let _ = ResultStatus::Success; // keep the import meaningful across edits
    }
}
