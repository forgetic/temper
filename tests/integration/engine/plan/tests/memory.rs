//! Memory stays within the worst case (programming-style.md, 6.4), measured by
//! a counting allocator: every entry point of the plan, called with inputs of
//! exactly its limits, holds no more of its own than `worst_case` says; what
//! it hands out (writes, runs, problems) is its receiver's.

use temper_engine_model_plan::{
    AgentSpec, Batch, Budget, ChangeSpec, Charter, Ci, Config, Entry, Envelope, Facts, Gate, Goal, Grants, Growth,
    Inbound, Limits, Mergeable, Outcome, Plan, Progress, Pull, PullState, Record, Relations, Repo, Repository, Resume,
    Review, SessionSpec, Source, Sources, Step, Target, Template, Wake, Work, Write, accept, apply, check, due, grow,
    max_out, wake, worst_case,
};
use temper_engine_model_plan_tests::translate::commit;
use temper_lib::{Duration, Env, List, Queue, Time};
use temper_world::heap::{self, Meter};

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(600) };

const LIMITS: Limits = Limits {
    steps: 16,
    name_bytes: 24,
    dependencies: 4,
    gates: 2,
    targets: 2,
    instruction_bytes: 512,
    tasks: 4,
    events: 32,
    repairs: 3,
    rebases: 6,
    rejections: 3,
    stall: Duration::from_secs(600),
    budget: BUDGET,
};

const WAKE: Wake = Wake {
    on: Sources { own: true, related: true, subscribed: true, messages: true },
    every: Some(Duration::from_secs(60)),
    batch: Batch { count: 4, age: Some(Duration::from_secs(60)) },
};

fn config(limits: &Limits) -> Config {
    Config {
        repositories: Box::new([Repo { bases: Box::new([name(limits, 0)]) }]),
        templates: Box::new([Template { name: name(limits, 1), guidance: Box::from([b'g'; 64].as_slice()) }]),
    }
}

/// A name of exactly the limit, ending in `index`.
fn name(limits: &Limits, index: u32) -> Box<[u8]> {
    let mut bytes = vec![b'n'; usize::try_from(limits.name_bytes).expect("fits")];
    let tail = index.to_be_bytes();
    let at = bytes.len() - tail.len();
    bytes[at..].copy_from_slice(&tail);
    bytes.into_boxed_slice()
}

fn charter(limits: &Limits) -> Charter {
    Charter {
        instructions: vec![b'i'; usize::try_from(limits.instruction_bytes).expect("fits")].into_boxed_slice(),
        template: Some(name(limits, 1)),
        grants: Grants { modify: true, shell: true, forge: true, subagents: true, note: true },
        budget: Budget { tokens: 10, ..limits.budget },
    }
}

/// Step `index` of a plan of exactly the limits: a change reviewed by an
/// agent, with as many gates as a step may add, after as many of the steps
/// before it as a step may come after.
fn step(limits: &Limits, index: u32) -> Step {
    let mut after = Vec::new();
    for before in index.saturating_sub(limits.dependencies)..index {
        after.push(name(limits, 100 + before));
    }
    let mut gates = vec![Gate::Approvals(1); usize::try_from(limits.gates).expect("fits")];
    if let Some(last) = gates.last_mut() {
        *last = Gate::Accepted;
    }
    Step {
        name: name(limits, 100 + index),
        repository: Repository(0),
        work: Work::Change(ChangeSpec {
            base: name(limits, 0),
            produce: charter(limits),
            checks: true,
            review: Review::Agent(charter(limits)),
        }),
        after: after.into_boxed_slice(),
        gates: gates.into_boxed_slice(),
    }
}

fn steps(limits: &Limits, from: u32, to: u32) -> Box<[Step]> {
    let mut steps = Vec::new();
    for index in from..to {
        steps.push(step(limits, index));
    }
    steps.into_boxed_slice()
}

fn envelope(limits: &Limits) -> Envelope {
    let mut into = Vec::new();
    for _ in 0..limits.targets {
        into.push(Target { repository: Repository(0), base: name(limits, 0) });
    }
    Envelope {
        agents: 0,
        changes: limits.steps,
        waits: 0,
        sessions: 0,
        repositories: Box::new([Repository(0)]),
        into: into.into_boxed_slice(),
    }
}

fn plan(limits: &Limits) -> Plan {
    Plan { steps: steps(limits, 0, limits.steps), envelope: envelope(limits), budget: u64::MAX }
}

/// The goal of a plan of `count` steps, the last of them added by the first.
fn goal(limits: &Limits, count: u32) -> Goal {
    let mut entries = Vec::new();
    for index in 0..count {
        let step = step(limits, index);
        let parent = if index > 0 && index + 1 == count { Some(0) } else { None };
        entries.push(Entry { name: step.name, after: step.after, parent, run: 1 });
    }
    Goal {
        steps: entries.into_boxed_slice(),
        envelope: envelope(limits),
        budget: u64::MAX,
        estimate: 0,
        growth: Growth::NONE,
    }
}

fn facts() -> Facts {
    Facts {
        created: Time::ZERO,
        dependencies: Relations::NONE,
        children: Relations::NONE,
        branch: None,
        gone: false,
        pull: None,
        decision: None,
        closed: false,
        snapshot: false,
        woken: true,
    }
}

/// The plan under `limits`: its configuration and environment, and the queue
/// its writes go into, all made before any call is measured.
struct Measured {
    config: Config,
    env: Env<Limits>,
    out: Queue<Write>,
    bound: u64,
}

impl Measured {
    fn new(limits: Limits) -> Measured {
        let bound = worst_case(&limits).expect("the test limits fit");
        Measured {
            config: config(&limits),
            env: Env { now: Time::ZERO, limits },
            out: Queue::with_capacity(max_out(&limits)),
            bound,
        }
    }

    /// Measures `call`, given the plan's configuration, environment and
    /// queue: what it hands out (its result, its writes) is dropped, and what
    /// it held of its own checked against the worst case. Returns its peak,
    /// all of it.
    fn measure<T>(&mut self, what: &str, call: impl FnOnce(&Config, &Env<Limits>, &mut Queue<Write>) -> T) -> u64 {
        let meter = Meter::new();
        meter.start();
        let result = call(&self.config, &self.env, &mut self.out);
        let measured = meter.end();
        drop(result);
        while self.out.pop().is_some() {}
        meter.check(measured, self.bound, (what, self.env.limits));
        measured.peak
    }
}

fn record(step: Step) -> Record {
    Record { step, progress: Progress::NEW, goal: None }
}

fn session(limits: &Limits) -> Step {
    Step {
        work: Work::Session(SessionSpec { charter: charter(limits), resume: Resume::Default, wake: WAKE }),
        ..step(limits, 0)
    }
}

fn paths(limits: Limits) {
    let mut plan_ = Measured::new(limits);

    // Checking and accepting a plan of exactly the limits.
    let full = plan(&limits);
    plan_.measure("check", |config, env, _| {
        let checked = check(config, env, &full);
        assert!(checked.is_ok(), "{checked:?}");
        checked
    });
    plan_.measure("accept", |config, env, out| {
        let accepted = accept(config, env, &full, 0, out);
        assert_eq!(out.len(), limits.steps + 1, "an item for each step, and the goal");
        accepted
    });

    // A plan of nameless steps, which come after steps it lacks: as many
    // problems as are listed, and more counted, under the larger limits.
    let mut broken = plan(&limits);
    for step in &mut broken.steps {
        step.name = Box::new([]);
    }
    plan_.measure("check, refused", |config, env, _| {
        let refused = check(config, env, &broken);
        assert!(refused.is_err(), "{refused:?}");
        refused
    });

    // Growing a plan to its limit, by its session.
    let started = goal(&limits, 1);
    let added = steps(&limits, 1, limits.steps);
    plan_.measure("grow", |config, env, out| {
        let grown = grow(config, env, &started, None, 1, &added, out);
        assert!(grown.is_ok(), "{grown:?}");
        grown
    });

    // A growing step's outcome, and a session's plan and tasks.
    let mut grower = step(&limits, 0);
    grower.name = name(&limits, 99);
    grower.work = Work::Agent(AgentSpec { charter: charter(&limits), grows: true });
    let grower = record(grower);
    let outcome = Outcome::Steps(steps(&limits, 1, limits.steps));
    plan_.measure("apply, steps", |config, env, out| {
        apply(config, env, &grower, Some(&started), &facts(), &outcome, out)
    });
    let chatting = record(session(&limits));
    let proposed = Outcome::Plan(plan(&limits));
    plan_.measure("apply, plan", |config, env, out| apply(config, env, &chatting, None, &facts(), &proposed, out));
    let mut tasks = Vec::new();
    for index in 0..limits.tasks {
        tasks.push(Step { after: Box::new([]), ..step(&limits, index) });
    }
    let tasks = Outcome::Tasks(tasks.into_boxed_slice());
    plan_.measure("apply, tasks", |config, env, out| apply(config, env, &chatting, None, &facts(), &tasks, out));

    // What is due: a run, an action, a review, a session's turn.
    let change = record(step(&limits, 0));
    plan_.measure("due, a run", |config, env, out| due(config, env, &change, &facts(), out));
    let pushed = Facts { branch: Some(commit(1)), ..facts() };
    plan_.measure("due, an action", |config, env, out| due(config, env, &change, &pushed, out));
    let pull = Pull {
        head: commit(1),
        pushed: Time::ZERO,
        state: PullState::Open,
        ci: Ci::Passed,
        approvals: 0,
        changes_requested: false,
        merge: Mergeable::Clean,
        base_moved: false,
    };
    let reviewing = Facts { pull: Some(pull), ..pushed };
    plan_.measure("due, a review", |config, env, out| due(config, env, &change, &reviewing, out));
    plan_.measure("due, a turn", |config, env, out| due(config, env, &chatting, &facts(), out));

    // A wake decision over a full inbox.
    let inbox = vec![Inbound { source: Source::Own, at: Time::ZERO }; usize::try_from(limits.events).expect("fits")];
    plan_.measure("wake", |_, env, _| wake(env, &WAKE, &inbox, Time::ZERO));
}

#[test]
fn every_entry_point_stays_within_the_worst_case() {
    paths(LIMITS);
    paths(Limits { steps: 4, dependencies: 1, gates: 1, targets: 1, tasks: 1, ..LIMITS });
    paths(Limits { steps: 64, dependencies: 8, tasks: 8, instruction_bytes: 4096, ..LIMITS });
}

/// A check of a plan of exactly the limits reaches the scratch the worst case
/// counts: a count and a place in the order for every step.
#[test]
fn a_full_plans_check_reaches_its_scratch() {
    for limits in [LIMITS, Limits { steps: 64, dependencies: 8, ..LIMITS }] {
        let mut plan_ = Measured::new(limits);
        let full = plan(&limits);
        let peak = plan_.measure("check", |config, env, _| check(config, env, &full));
        let scratch = List::<u32>::worst_case(limits.steps).expect("fits")
            + Queue::<u32>::worst_case(limits.steps).expect("fits");
        assert!(peak >= scratch, "{limits:?}: {peak} bytes at the peak, less than the scratch's {scratch}");
    }
}
