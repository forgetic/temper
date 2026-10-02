//! Checking a plan before it exists (engine-model.md, 5.2): its steps have
//! unique names and known repositories, branches and templates, come after
//! steps of the plan and never, through them, after themselves, and fit the
//! limits; its envelope lets changes land only where the deployment does; its
//! steps' budgets, added up, are within its own. What fails is feedback for
//! the run that wrote the plan: which step, and what is wrong with it.
//!
//! A primitive is a closed type here, so a plan of unknown primitives does not
//! reach the model: the protocol layer refuses it as it decodes the outcome.

use alloc::boxed::Box;

use temper_lib::{Env, List, Queue};

use crate::config::{Config, names};
use crate::limits::Limits;
use crate::plan::{Charter, Envelope, Gate, Plan, Review, Step, Work};

/// Something wrong with a plan, or with an outcome, for the run that wrote it
/// to fix. Steps are counted from zero, in the order the run gave them, and so
/// are a step's dependencies and gates and an envelope's targets.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Problem {
    /// The plan has no steps.
    NoSteps,
    /// The plan would hold more than `max` steps, those it has included.
    TooManySteps { max: u32 },
    /// The outcome creates more than `max` tasks.
    TooManyTasks { max: u32 },
    /// The step has no name.
    EmptyName { step: u32 },
    /// The step's name, or the branch its change lands into, is longer than
    /// `max` bytes.
    LongName { step: u32, max: u32 },
    /// An earlier step of the plan, or one it already has, has this step's
    /// name.
    NameTaken { step: u32 },
    /// The step's item would be in a repository the deployment does not have.
    UnknownRepository { step: u32 },
    /// The step's change would land into a branch the deployment does not let
    /// changes land into.
    UnknownBase { step: u32 },
    /// The step names a template the deployment does not have.
    UnknownTemplate { step: u32 },
    /// The step's instructions are longer than `max` bytes.
    LongInstructions { step: u32, max: u32 },
    /// The step gives a run more budget than a run may have.
    LargeBudget { step: u32 },
    /// The step comes after more than `max` steps.
    TooManyDependencies { step: u32, max: u32 },
    /// The step comes after a step the plan does not have. A task comes after
    /// none.
    UnknownDependency { step: u32, dependency: u32 },
    /// The step comes, through its dependencies, after itself.
    Cycle { step: u32 },
    /// The step adds more than `max` gates.
    TooManyGates { step: u32, max: u32 },
    /// The step's gate is not one its primitive has, or asks for nothing.
    Gate { step: u32, gate: u32 },
    /// The step's wake rule lets nothing through: it batches no events.
    Wake { step: u32 },
    /// The envelope lets changes land into more than `max` branches.
    TooManyTargets { max: u32 },
    /// The envelope lets changes land into a branch the deployment does not.
    UnknownTarget { target: u32 },
    /// The plan's steps are estimated to spend more than its budget.
    OverBudget { estimate: u64, budget: u64 },
    /// The step may not finish with an outcome of this kind.
    NotAllowed,
    /// Steps added by an item under no goal, which has no plan to add them to.
    NoGoal,
}

/// What is wrong: the first problems found, at most [`Problems::LISTED`] of
/// them in the order they were found, and how many more there were.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Problems {
    pub listed: Box<[Problem]>,
    pub more: u32,
}

impl Problems {
    /// Enough for the run to see what is wrong, few enough to keep what it is
    /// told small.
    pub const LISTED: u32 = 8;
}

/// Whether `plan` may exist, and if so the tokens its steps are estimated to
/// spend, from their runs' budgets.
pub fn check(config: &Config, env: &Env<Limits>, plan: &Plan) -> Result<u64, Problems> {
    let mut found = Found::new();
    let estimate = check_plan(config, &env.limits, plan, &mut found);
    if found.is_empty() {
        return Ok(estimate);
    }
    Err(found.into_problems())
}

pub(crate) fn check_plan(config: &Config, limits: &Limits, plan: &Plan, found: &mut Found) -> u64 {
    if plan.steps.is_empty() {
        found.add(Problem::NoSteps);
    }
    let estimate = check_steps(config, limits, &[], &plan.steps, Among::Plan, found);
    check_envelope(config, limits, &plan.envelope, found);
    if estimate > plan.budget {
        found.add(Problem::OverBudget { estimate, budget: plan.budget });
    }
    estimate
}

/// Whether steps join a plan or stand alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Among {
    /// They join a plan, after the steps it has, if any.
    Plan,
    /// They are tasks, each an item on its own, after nothing.
    Alone,
}

/// Checks `steps`, to join a plan whose steps are named `existing` or to stand
/// alone, and returns the tokens they are estimated to spend.
pub(crate) fn check_steps(
    config: &Config,
    limits: &Limits,
    existing: &[Box<[u8]>],
    steps: &[Step],
    among: Among,
    found: &mut Found,
) -> u64 {
    match among {
        Among::Plan => {
            if count(existing.len()).saturating_add(count(steps.len())) > limits.steps {
                found.add(Problem::TooManySteps { max: limits.steps });
                return 0;
            }
        }
        Among::Alone => {
            if count(steps.len()) > limits.tasks {
                found.add(Problem::TooManyTasks { max: limits.tasks });
                return 0;
            }
        }
    }
    let mut estimate: u64 = 0;
    for (index, step) in steps.iter().enumerate() {
        let at = count(index);
        check_step(config, limits, existing, steps, at, among, found);
        estimate = estimate.saturating_add(cost(step));
    }
    check_order(steps, found);
    estimate
}

/// The tokens a step's runs are estimated to spend: one of each of its
/// runs, at its budget.
pub(crate) fn cost(step: &Step) -> u64 {
    match &step.work {
        Work::Agent(spec) => spec.charter.budget.tokens,
        Work::Change(spec) => match &spec.review {
            Review::Person => spec.produce.budget.tokens,
            Review::Agent(review) => spec.produce.budget.tokens.saturating_add(review.budget.tokens),
        },
        Work::Wait(_) => 0,
        Work::Session(spec) => spec.charter.budget.tokens,
    }
}

fn check_step(
    config: &Config,
    limits: &Limits,
    existing: &[Box<[u8]>],
    steps: &[Step],
    at: u32,
    among: Among,
    found: &mut Found,
) {
    let step = steps.get(place(at)).expect("a step of the steps checked");
    if step.name.is_empty() {
        found.add(Problem::EmptyName { step: at });
    } else if count(step.name.len()) > limits.name_bytes {
        found.add(Problem::LongName { step: at, max: limits.name_bytes });
    } else if names(existing, &step.name) || first_named(steps, &step.name) != Some(at) {
        found.add(Problem::NameTaken { step: at });
    }
    if config.repo(step.repository).is_none() {
        found.add(Problem::UnknownRepository { step: at });
    }
    check_after(limits, existing, steps, at, among, found);
    check_gates(limits, step, at, found);
    if step.wake.batch.count == 0 {
        found.add(Problem::Wake { step: at });
    }
    match &step.work {
        Work::Agent(spec) => check_charter(config, limits, &spec.charter, at, found),
        Work::Change(spec) => {
            if count(spec.base.len()) > limits.name_bytes {
                found.add(Problem::LongName { step: at, max: limits.name_bytes });
            } else if config.repo(step.repository).is_some() && !config.is_base(step.repository, &spec.base) {
                found.add(Problem::UnknownBase { step: at });
            }
            check_charter(config, limits, &spec.produce, at, found);
            match &spec.review {
                Review::Person => {}
                Review::Agent(review) => check_charter(config, limits, review, at, found),
            }
        }
        Work::Wait(_) => {}
        Work::Session(spec) => check_charter(config, limits, &spec.charter, at, found),
    }
}

fn check_after(limits: &Limits, existing: &[Box<[u8]>], steps: &[Step], at: u32, among: Among, found: &mut Found) {
    let step = steps.get(place(at)).expect("a step of the steps checked");
    if count(step.after.len()) > limits.dependencies {
        found.add(Problem::TooManyDependencies { step: at, max: limits.dependencies });
        return;
    }
    for (dependency, name) in step.after.iter().enumerate() {
        let known = match among {
            Among::Plan => names(existing, name) || first_named(steps, name).is_some(),
            Among::Alone => false,
        };
        if !known {
            found.add(Problem::UnknownDependency { step: at, dependency: count(dependency) });
        }
    }
}

fn check_gates(limits: &Limits, step: &Step, at: u32, found: &mut Found) {
    if count(step.gates.len()) > limits.gates {
        found.add(Problem::TooManyGates { step: at, max: limits.gates });
        return;
    }
    let change = match &step.work {
        Work::Change(_) => true,
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => false,
    };
    for (gate, kind) in step.gates.iter().enumerate() {
        let fits = match kind {
            Gate::Approvals(people) => change && *people > 0,
            Gate::Accepted => true,
        };
        if !fits {
            found.add(Problem::Gate { step: at, gate: count(gate) });
        }
    }
}

fn check_charter(config: &Config, limits: &Limits, charter: &Charter, at: u32, found: &mut Found) {
    if count(charter.instructions.len()) > limits.instruction_bytes {
        found.add(Problem::LongInstructions { step: at, max: limits.instruction_bytes });
    }
    if let Some(template) = &charter.template
        && config.template(template).is_none()
    {
        found.add(Problem::UnknownTemplate { step: at });
    }
    if !charter.budget.within(limits.budget) {
        found.add(Problem::LargeBudget { step: at });
    }
}

fn check_envelope(config: &Config, limits: &Limits, envelope: &Envelope, found: &mut Found) {
    if count(envelope.into.len()) > limits.targets {
        found.add(Problem::TooManyTargets { max: limits.targets });
        return;
    }
    for (index, target) in envelope.into.iter().enumerate() {
        if !config.is_base(target.repository, &target.base) {
            found.add(Problem::UnknownTarget { target: count(index) });
        }
    }
}

/// Orders `steps` by their dependencies among themselves, as Kahn's algorithm
/// does: a step is ready once every step it comes after is ordered. A step
/// never ready comes after a cycle, or is on one; one on a cycle is reported.
/// Dependencies on steps the plan already has are met, and names nothing
/// has are reported elsewhere. Where names repeat, the first step of a name is
/// the one others come after.
fn check_order(steps: &[Step], found: &mut Found) {
    let total = count(steps.len());
    // How many of the steps each one comes after are not yet ordered.
    let mut waiting: List<u32> = List::with_capacity(total);
    let mut ready: Queue<u32> = Queue::with_capacity(total);
    for (index, step) in steps.iter().enumerate() {
        let mut among: u32 = 0;
        for name in &step.after {
            if first_named(steps, name).is_some() {
                among = among.saturating_add(1);
            }
        }
        if among == 0 {
            ready.push(count(index));
        }
        let pushed = waiting.push(among);
        assert!(pushed.is_ok(), "a count for each step");
    }
    let mut ordered: u32 = 0;
    for _ in 0..total {
        let Some(done) = ready.pop() else {
            break;
        };
        ordered = ordered.saturating_add(1);
        let name = &steps.get(place(done)).expect("a step of the steps ordered").name;
        if first_named(steps, name) != Some(done) {
            // Steps that come after this name come after an earlier step.
            continue;
        }
        for (index, step) in steps.iter().enumerate() {
            for after in &step.after {
                if **after != **name {
                    continue;
                }
                let left = waiting.get_mut(count(index)).expect("a count for each step");
                *left = left.checked_sub(1).expect("a step is released once for each time it names the step");
                if *left == 0 {
                    ready.push(count(index));
                }
            }
        }
    }
    if ordered == total {
        return;
    }
    // Every step never ordered comes after one never ordered, so following
    // such dependencies from any of them for as many steps as there are ends
    // on a cycle.
    let mut at = first_waiting(&waiting).expect("a step never ordered");
    for _ in 0..total {
        let step = steps.get(place(at)).expect("a step of the steps ordered");
        let mut next = None;
        for name in &step.after {
            if let Some(before) = first_named(steps, name)
                && waiting.get(before).copied().unwrap_or(0) > 0
            {
                next = Some(before);
                break;
            }
        }
        at = next.expect("a step never ordered comes after one never ordered");
    }
    found.add(Problem::Cycle { step: at });
}

/// The first step of `waiting` still waiting.
fn first_waiting(waiting: &List<u32>) -> Option<u32> {
    for (index, left) in waiting.iter().enumerate() {
        if *left > 0 {
            return Some(count(index));
        }
    }
    None
}

/// The first of `steps` named `name`.
pub(crate) fn first_named(steps: &[Step], name: &[u8]) -> Option<u32> {
    for (index, step) in steps.iter().enumerate() {
        if *step.name == *name {
            return Some(count(index));
        }
    }
    None
}

/// The problems found so far.
#[derive(Debug)]
pub(crate) struct Found {
    listed: List<Problem>,
    more: u32,
}

impl Found {
    pub(crate) fn new() -> Found {
        Found { listed: List::with_capacity(Problems::LISTED), more: 0 }
    }

    pub(crate) fn add(&mut self, problem: Problem) {
        match self.listed.push(problem) {
            Ok(()) => {}
            Err(_) => self.more = self.more.saturating_add(1),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.listed.is_empty()
    }

    pub(crate) fn into_problems(self) -> Problems {
        Problems { listed: self.listed.into_boxed(), more: self.more }
    }
}

/// A length or an index, as the `u32` the vocabulary counts in. Every one
/// counted is bounded by a limit, or refused before it is counted.
pub(crate) fn count(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// A `u32` index, to index a slice with.
pub(crate) fn place(index: u32) -> usize {
    usize::try_from(index).expect("a u32 fits in a usize")
}
