//! What the world's runs and people do, drawn from its seed: the plans
//! sessions propose, the steps they and growing agent steps add, the tasks
//! they make, the outcomes runs finish with, and how people review, decide
//! and accept. The deployment the plan is configured with is here too.

use temper_engine_model_plan::{
    AgentSpec, Batch, Budget, ChangeSpec, Charter, Config, Envelope, Gate, Goal, Grants, Plan, Repo, Repository,
    Resume, Review, SessionSpec, Sources, Step, Target, Template, WaitSpec, Wake, Work,
};
use temper_lib::{Duration, Rng};
use temper_world::Span;

/// How often each thing happens, per mille, and how long things take.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Script {
    /// The fewest and the most steps a proposed plan has.
    pub steps: (u32, u32),
    /// A step is a change.
    pub changes: u32,
    /// A step is a wait.
    pub waits: u32,
    /// An agent step may grow its plan.
    pub grows: u32,
    /// A step comes after each step before it.
    pub dependencies: u32,
    /// A change is reviewed by an agent rather than a person.
    pub agent_reviews: u32,
    /// A step adds a gate.
    pub gates: u32,
    /// A proposed plan breaks a check, until the feedback is taken.
    pub invalid: u32,
    /// A supervising session's turn adds steps to its plan.
    pub growth: u32,
    /// Growth goes beyond the envelope.
    pub beyond: u32,
    /// A supervising session's turn makes a task.
    pub tasks: u32,
    /// A run escalates.
    pub escalations: u32,
    /// A run fails, and is tried again.
    pub failures: u32,
    /// An agent asks for changes on the head it reviews.
    pub verdicts: u32,
    /// How long a run takes.
    pub run: Span,
    /// CI fails on a head.
    pub ci_fails: u32,
    /// How long CI takes on a head.
    pub ci: Span,
    /// A head conflicts with its base.
    pub conflicts: u32,
    /// How long the forge takes to say whether a head merges cleanly.
    pub mergeable: Span,
    /// A person asks for changes on a head they review.
    pub changes_asked: u32,
    /// How long a person takes to review a head, or to decide.
    pub people: Span,
    /// A person rejects what they are asked to decide: a wait, a gate.
    pub rejects: u32,
    /// A person rejects a proposal: a plan, or growth beyond its envelope.
    pub proposals: u32,
    /// A person closes a pull request without merging it.
    pub closes: u32,
    /// A person pushes to a branch a pull request lands into while it is open.
    pub pushes: u32,
}

/// The deployment: two repositories; changes land into `main` or `feat` of
/// the first and `main` of the second. One template, `fix`.
#[must_use]
pub fn config() -> Config {
    Config {
        repositories: Box::new([
            Repo { bases: Box::new([bytes("main"), bytes("feat")]) },
            Repo { bases: Box::new([bytes("main")]) },
        ]),
        templates: Box::new([Template { name: bytes("fix"), guidance: bytes("fix what is said to be broken") }]),
    }
}

/// The branches changes land into, by repository.
pub const BASES: [(u32, &[u8]); 3] = [(0, b"main"), (0, b"feat"), (1, b"main")];

/// A branch no envelope this world draws lets changes land into: growth that
/// lands there goes beyond it.
const OUTSIDE: (u32, &[u8]) = (0, b"main");

fn bytes(text: &str) -> Box<[u8]> {
    Box::from(text.as_bytes())
}

/// What every run of this world may spend.
pub const BUDGET: Budget = Budget { tokens: 1_000, turns: 20, time: Duration::from_secs(3_600) };

const GRANTS: Grants = Grants { modify: true, shell: true, forge: true, subagents: false, note: false };

/// A session wakes on a person's message at once, and on its steps
/// finishing two at a time, or half an hour after the first.
pub const SESSION_WAKE: Wake = Wake {
    on: Sources { own: false, related: true, subscribed: false, messages: true },
    every: None,
    batch: Batch { count: 2, age: Some(Duration::from_secs(1_800)) },
};

fn charter(rng: &mut Rng) -> Charter {
    let template = if rng.chance(200) { Some(bytes("fix")) } else { None };
    Charter {
        instructions: bytes("do what the step says"),
        template,
        grants: GRANTS,
        budget: Budget { tokens: rng.between(100, BUDGET.tokens), ..BUDGET },
    }
}

/// The session a goal begins with.
#[must_use]
pub fn session(number: u64) -> Step {
    let charter = Charter {
        instructions: bytes("talk with the person, and plan the work"),
        template: None,
        grants: GRANTS,
        budget: BUDGET,
    };
    Step {
        name: format!("session-{number}").into_bytes().into_boxed_slice(),
        repository: Repository(0),
        work: Work::Session(SessionSpec { charter, resume: Resume::Default, wake: SESSION_WAKE }),
        after: Box::new([]),
        gates: Box::new([]),
    }
}

/// A plan a session proposes: steps of each primitive, each after some of
/// those before it, with room in its budget to grow to `steps`. An invalid
/// one comes after a step it lacks, or after itself.
pub fn plan(rng: &mut Rng, script: &Script, steps_most: u32, dependencies: u32, invalid: bool) -> Plan {
    let count = rng.between(u64::from(script.steps.0), u64::from(script.steps.1));
    let mut names: Vec<Box<[u8]>> = Vec::new();
    let mut steps = Vec::new();
    for index in 0..count {
        let name = format!("s{index}").into_bytes().into_boxed_slice();
        let mut after = Vec::new();
        for earlier in &names {
            if after.len() < usize::try_from(dependencies).expect("fits") && rng.chance(script.dependencies) {
                after.push(earlier.clone());
            }
        }
        let step = drawn(rng, script, name.clone(), after, true);
        names.push(name);
        steps.push(step);
    }
    if invalid && let Some(first) = steps.first_mut() {
        let wrong = if rng.chance(500) { first.name.clone() } else { bytes("nowhere") };
        first.after = vec![wrong].into_boxed_slice();
    }
    let budget = u64::from(steps_most) * 2 * BUDGET.tokens;
    Plan {
        steps: steps.into_boxed_slice(),
        envelope: Envelope {
            agents: 2,
            changes: 2,
            waits: 1,
            sessions: 0,
            repositories: Box::new([Repository(0)]),
            into: Box::new([Target { repository: Repository(0), base: bytes("feat") }]),
        },
        budget,
    }
}

/// A step named `name`, coming after `after`: an agent step, a change or (in
/// a plan) a wait.
fn drawn(rng: &mut Rng, script: &Script, name: Box<[u8]>, after: Vec<Box<[u8]>>, waits: bool) -> Step {
    let kind = rng.below(1000);
    let change = kind < u64::from(script.changes);
    let wait = waits && !change && kind < u64::from(script.changes + script.waits);
    let (repository, work) = if change {
        let (repository, base) = BASES[usize::try_from(rng.below(3)).expect("fits")];
        (repository, Work::Change(change_spec(rng, script, base)))
    } else if wait {
        let spec = match rng.below(3) {
            0 if !after.is_empty() => WaitSpec::Steps,
            0 | 1 => WaitSpec::Decision,
            _ => WaitSpec::Time(Duration::from_secs(rng.between(60, 7_200))),
        };
        (0, Work::Wait(spec))
    } else {
        (0, Work::Agent(AgentSpec { charter: charter(rng), grows: rng.chance(script.grows) }))
    };
    let mut gates = Vec::new();
    if rng.chance(script.gates) {
        match &work {
            Work::Change(_) if rng.chance(500) => gates.push(Gate::Approvals(2)),
            Work::Change(_) | Work::Agent(_) | Work::Wait(_) | Work::Session(_) => gates.push(Gate::Accepted),
        }
    }
    Step {
        name,
        repository: Repository(repository),
        work,
        after: after.into_boxed_slice(),
        gates: gates.into_boxed_slice(),
    }
}

fn change_spec(rng: &mut Rng, script: &Script, base: &[u8]) -> ChangeSpec {
    let review = if rng.chance(script.agent_reviews) { Review::Agent(charter(rng)) } else { Review::Person };
    ChangeSpec { base: Box::from(base), produce: charter(rng), checks: true, review }
}

/// Steps added to `goal`'s plan, named after `serial`, each after one of
/// `done` (steps of the plan already done) or after nothing: within its
/// envelope as far as its primitives go, or a change landing outside it. None
/// if the plan has no room for them.
pub fn growth(
    rng: &mut Rng,
    script: &Script,
    goal: &Goal,
    done: &[Box<[u8]>],
    steps: u32,
    serial: u64,
) -> Option<Box<[Step]>> {
    let count = rng.between(1, 2);
    if u64::try_from(goal.steps.len()).expect("fits") + count > u64::from(steps) {
        return None;
    }
    let beyond = rng.chance(script.beyond);
    let mut added = Vec::new();
    for index in 0..count {
        let name = format!("g{serial}-{index}").into_bytes().into_boxed_slice();
        let mut after = Vec::new();
        if !done.is_empty() && rng.chance(script.dependencies) {
            let count = u64::try_from(done.len()).expect("fits");
            after.push(done[usize::try_from(rng.below(count)).expect("fits")].clone());
        }
        let step = if beyond && index == 0 {
            let (repository, base) = OUTSIDE;
            Step {
                repository: Repository(repository),
                work: Work::Change(change_spec(rng, script, base)),
                ..drawn(rng, script, name, after, false)
            }
        } else if rng.chance(script.changes) {
            Step {
                repository: Repository(0),
                work: Work::Change(change_spec(rng, script, b"feat")),
                ..drawn(rng, script, name, after, false)
            }
        } else {
            let mut step = drawn(rng, script, name, after, false);
            step.work = Work::Agent(AgentSpec { charter: charter(rng), grows: false });
            step.repository = Repository(0);
            step.gates = Box::new([]);
            step
        };
        added.push(step);
    }
    Some(added.into_boxed_slice())
}

/// A task a session makes, named after `serial`: an agent step or a change
/// in the second repository.
pub fn task(rng: &mut Rng, script: &Script, serial: u64) -> Step {
    let name = format!("t{serial}").into_bytes().into_boxed_slice();
    let mut step = drawn(rng, script, name, Vec::new(), false);
    step.repository = Repository(1);
    step.work = match step.work {
        Work::Change(_) => Work::Change(change_spec(rng, script, b"main")),
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => {
            Work::Agent(AgentSpec { charter: charter(rng), grows: false })
        }
    };
    step
}
