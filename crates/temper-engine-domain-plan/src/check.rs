//! Checking a plan before it exists (engine-domain.md, 5.2): its steps have
//! unique names and known repositories, branches and templates, come after
//! steps of the plan and never, through them, after themselves, and fit the
//! limits; its envelope lets steps be only where the deployment has
//! repositories, and changes land only where it lets them; its steps'
//! budgets, added up, are within its own. What fails is feedback for the run
//! that wrote the plan: which step, and what is wrong with it.
//!
//! A step is done only once the steps it added are, so the order a plan's
//! steps are checked in is over its whole graph: a step waits on the steps it
//! comes after and on the steps it added. Names are resolved once, into
//! places, and the order is Kahn's, which also gives the order a plan's items
//! are made in, dependencies first.
//!
//! A goal's record is forge data, which a person may edit: what it says is
//! checked as it is read, and what does not hold is a [`Problem::Goal`],
//! never a panic.
//!
//! A primitive is a closed type here, so a plan of unknown primitives does not
//! reach the domain: the protocol layer refuses it as it decodes the outcome.

use alloc::boxed::Box;

use skein_lib::{Env, List, Queue};

use crate::config::Config;
use crate::limits::Limits;
use crate::plan::{Charter, Envelope, Gate, Plan, Review, Step, Work};
use crate::record::{Entry, Goal};

/// Something wrong with a plan, or with an outcome, for the run that wrote it
/// to fix. Steps are counted from zero, in the order the run gave them, and so
/// are a step's dependencies and gates and an envelope's targets and
/// repositories.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Problem {
    /// The plan has no steps.
    NoSteps,
    /// The plan would hold more than `max` steps, those it has included.
    TooManySteps { max: u32 },
    /// The outcome creates more than `max` tasks.
    TooManyTasks { max: u32 },
    /// The item would have more than `max` items it made not done yet, the
    /// tasks the outcome creates among them.
    TooManyChildren { max: u32 },
    /// The step has no name.
    EmptyName { step: u32 },
    /// The step's name is longer than `max` bytes.
    LongName { step: u32, max: u32 },
    /// An earlier step of the plan, or one it already has, has this step's
    /// name.
    NameTaken { step: u32 },
    /// The step's item would be in a repository the deployment does not have.
    UnknownRepository { step: u32 },
    /// The branch the step's change lands into is longer than `max` bytes.
    LongBase { step: u32, max: u32 },
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
    /// The step comes, through its dependencies and the steps they added,
    /// after itself.
    Cycle { step: u32 },
    /// The step adds more than `max` gates.
    TooManyGates { step: u32, max: u32 },
    /// The step's gate is not one its primitive has, or asks for nothing.
    Gate { step: u32, gate: u32 },
    /// The session's wake rule lets nothing through: it names no source and
    /// has no timer, or it batches no events, or more than the limits let a
    /// batch count.
    Wake { step: u32 },
    /// The task is an agent step that grows, and a task is in no plan.
    GrowingTask { step: u32 },
    /// The envelope names more than `max` branches, or more than `max`
    /// repositories.
    TooManyTargets { max: u32 },
    /// The envelope lets changes land into a branch the deployment does not.
    UnknownTarget { target: u32 },
    /// The envelope lets steps be in a repository the deployment does not
    /// have.
    UnknownArea { repository: u32 },
    /// The plan's steps are estimated to spend more than its budget.
    OverBudget { estimate: u64, budget: u64 },
    /// The step may not finish with an outcome of this kind.
    NotAllowed,
    /// Steps added by an item under no goal, which has no plan to add them to.
    NoGoal,
    /// A session released a step its goal does not have.
    UnknownStep,
    /// The goal's record is not one the plan wrote: its steps come, through
    /// one another, after themselves, or one comes after more steps than a
    /// step may, or was added by a step that joined after it. Its parent holds
    /// the goal's item for a person.
    Goal,
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
    let checked = check_plan(config, &env.limits, plan, &mut found);
    if found.is_empty() {
        return Ok(checked.estimate);
    }
    Err(found.into_problems())
}

/// Whether `goal`, read from its item's record, is one the plan could have
/// written: its parent runs this as it reads the record, and holds the goal's
/// item for a person if it is not.
pub fn check_goal(env: &Env<Limits>, goal: &Goal) -> Result<(), Problems> {
    let limits = &env.limits;
    let mut found = Found::new();
    if count(goal.steps.len()) > limits.steps || !holds_together(limits, &goal.steps) {
        found.add(Problem::Goal);
    } else {
        let _order: List<u32> = check_order(limits, &goal.steps, None, &[], &mut found);
    }
    if found.is_empty() {
        return Ok(());
    }
    Err(found.into_problems())
}

/// Whether `step`, read from its item's record, is one the plan could have
/// written: its parent runs this as it reads the record, as it runs
/// [`check_goal`] on a goal's part, and holds the item for a person if it is
/// not. The steps it comes after are its goal's, which it is not read with:
/// their names are checked within the limits only.
pub fn check_record(config: &Config, env: &Env<Limits>, step: &Step) -> Result<(), Problems> {
    let mut found = Found::new();
    check_step(config, &env.limits, &[], core::slice::from_ref(step), 0, Among::Record, &mut found);
    if found.is_empty() {
        return Ok(());
    }
    Err(found.into_problems())
}

pub(crate) fn check_plan(config: &Config, limits: &Limits, plan: &Plan, found: &mut Found) -> Checked {
    if plan.steps.is_empty() {
        found.add(Problem::NoSteps);
    }
    let checked = check_steps(config, limits, &[], None, &plan.steps, Among::Plan, found);
    check_envelope(config, limits, &plan.envelope, found);
    if checked.estimate > plan.budget {
        found.add(Problem::OverBudget { estimate: checked.estimate, budget: plan.budget });
    }
    checked
}

/// What a check found besides problems: the tokens the steps checked are
/// estimated to spend, and their places among them in an order where each
/// comes after the steps it comes after.
#[derive(Debug)]
pub(crate) struct Checked {
    pub(crate) estimate: u64,
    pub(crate) order: List<u32>,
}

/// Whether steps join a plan or stand alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Among {
    /// They join a plan, after the steps it has, if any.
    Plan,
    /// They are tasks, each an item on its own, after nothing.
    Alone,
    /// It is a step read from its item's record, without its goal's steps:
    /// what it comes after is named within the limits.
    Record,
}

/// Checks `steps`, to join a plan that has the steps `existing` (added by
/// its step `by`, if one adds them) or to stand alone. A plan's steps that do
/// not hold together are a [`Problem::Goal`], and no step is checked.
pub(crate) fn check_steps(
    config: &Config,
    limits: &Limits,
    existing: &[Entry],
    by: Option<u32>,
    steps: &[Step],
    among: Among,
    found: &mut Found,
) -> Checked {
    let none = Checked { estimate: 0, order: List::with_capacity(0) };
    match among {
        Among::Plan => {
            if count(existing.len()).saturating_add(count(steps.len())) > limits.steps {
                found.add(Problem::TooManySteps { max: limits.steps });
                return none;
            }
            if !holds_together(limits, existing) {
                found.add(Problem::Goal);
                return none;
            }
        }
        Among::Alone | Among::Record => {
            if count(steps.len()) > limits.tasks {
                found.add(Problem::TooManyTasks { max: limits.tasks });
                return none;
            }
        }
    }
    let mut estimate: u64 = 0;
    for (index, step) in steps.iter().enumerate() {
        let at = count(index);
        check_step(config, limits, existing, steps, at, among, found);
        estimate = estimate.saturating_add(cost(step));
    }
    let order = check_order(limits, existing, by, steps, found);
    Checked { estimate, order }
}

/// Whether the steps a goal has hold together as the plan wrote them: each
/// comes after no more steps than a step may, and was added, if a step added
/// it, by one that joined before it.
fn holds_together(limits: &Limits, existing: &[Entry]) -> bool {
    for (index, entry) in existing.iter().enumerate() {
        if count(entry.after.len()) > limits.dependencies {
            return false;
        }
        if let Some(parent) = entry.parent
            && parent >= count(index)
        {
            return false;
        }
    }
    true
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
    existing: &[Entry],
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
    } else if entry_named(existing, &step.name).is_some() || first_named(steps, &step.name) != Some(at) {
        found.add(Problem::NameTaken { step: at });
    }
    if config.repo(step.repository).is_none() {
        found.add(Problem::UnknownRepository { step: at });
    }
    check_after(limits, existing, steps, at, among, found);
    check_gates(limits, step, at, found);
    match &step.work {
        Work::Agent(spec) => {
            check_charter(config, limits, &spec.charter, at, found);
            match among {
                Among::Plan | Among::Record => {}
                Among::Alone => {
                    if spec.grows {
                        found.add(Problem::GrowingTask { step: at });
                    }
                }
            }
        }
        Work::Change(spec) => {
            if count(spec.base.len()) > limits.name_bytes {
                found.add(Problem::LongBase { step: at, max: limits.name_bytes });
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
        Work::Session(spec) => {
            check_charter(config, limits, &spec.charter, at, found);
            let wake = spec.wake;
            let sources = wake.on.own || wake.on.related || wake.on.subscribed || wake.on.messages;
            let batch = wake.batch.count;
            if (!sources && wake.every.is_none()) || batch == 0 || batch > limits.events {
                found.add(Problem::Wake { step: at });
            }
        }
    }
}

fn check_after(limits: &Limits, existing: &[Entry], steps: &[Step], at: u32, among: Among, found: &mut Found) {
    let step = steps.get(place(at)).expect("a step of the steps checked");
    if count(step.after.len()) > limits.dependencies {
        found.add(Problem::TooManyDependencies { step: at, max: limits.dependencies });
        return;
    }
    for (dependency, name) in step.after.iter().enumerate() {
        let known = match among {
            Among::Plan => resolve(existing, steps, name).is_some(),
            Among::Alone => false,
            Among::Record => !name.is_empty() && count(name.len()) <= limits.name_bytes,
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
    if count(envelope.into.len()) > limits.targets || count(envelope.repositories.len()) > limits.targets {
        found.add(Problem::TooManyTargets { max: limits.targets });
        return;
    }
    for (index, target) in envelope.into.iter().enumerate() {
        if !config.is_base(target.repository, &target.base) {
            found.add(Problem::UnknownTarget { target: count(index) });
        }
    }
    for (index, repository) in envelope.repositories.iter().enumerate() {
        if config.repo(*repository).is_none() {
            found.add(Problem::UnknownArea { repository: count(index) });
        }
    }
}

/// Orders the steps of the plan, those it has (`existing`) and those
/// checked, as Kahn's algorithm does: a step is ready once every step it
/// comes after is ordered, and every step it added. Steps checked are added
/// by `by`, one of those it has, if a step adds them. Returns the places of
/// the steps checked, in the order found.
///
/// A step never ready comes after a cycle, or is on one. Every cycle the
/// steps checked make goes through one of them, which is reported; a cycle
/// among the steps the plan has is a [`Problem::Goal`]. Names nothing has are
/// reported elsewhere, and dependencies past the limit are not ordered; where
/// names repeat, the first step of a name is the one others come after.
fn check_order(limits: &Limits, existing: &[Entry], by: Option<u32>, steps: &[Step], found: &mut Found) -> List<u32> {
    let known = count(existing.len());
    let total = known.saturating_add(count(steps.len()));
    let graph = resolved(limits, existing, steps);
    // How many times each step waits on a step not yet ordered: once for
    // each step it comes after, once for each step it added.
    let mut waiting: List<u32> = List::with_capacity(total);
    for node in 0..total {
        let pushed = waiting.push(graph.after(node));
        assert!(pushed.is_ok(), "a count for each step");
    }
    for node in 0..total {
        if let Some(parent) = parent_of(existing, by, node)
            && let Some(left) = waiting.get_mut(parent)
        {
            *left = left.saturating_add(1);
        }
    }
    let mut ready: Queue<u32> = Queue::with_capacity(total);
    for node in 0..total {
        if waiting.get(node).copied() == Some(0) {
            ready.push(node);
        }
    }
    let mut order: List<u32> = List::with_capacity(count(steps.len()));
    let mut ordered: u32 = 0;
    for _ in 0..total {
        let Some(done) = ready.pop() else {
            break;
        };
        ordered = ordered.saturating_add(1);
        if let Some(step) = done.checked_sub(known) {
            let pushed = order.push(step);
            assert!(pushed.is_ok(), "a place for each step checked");
        }
        for node in 0..total {
            let times = graph.times(node, done);
            if times > 0 {
                release(&mut waiting, &mut ready, node, times);
            }
        }
        if let Some(parent) = parent_of(existing, by, done) {
            release(&mut waiting, &mut ready, parent, 1);
        }
    }
    if ordered < total {
        found.add(cycle(&graph, existing, by, &waiting, known, total));
    }
    order
}

/// Step `node` waits `times` less; it is ready once it waits on nothing.
fn release(waiting: &mut List<u32>, ready: &mut Queue<u32>, node: u32, times: u32) {
    let Some(left) = waiting.get_mut(node) else {
        return;
    };
    if *left == 0 {
        return;
    }
    *left = left.saturating_sub(times);
    if *left == 0 {
        ready.push(node);
    }
}

/// A cycle's problem. Every step never ordered waits on one never ordered,
/// so following such steps from any of them for as many moves as there are
/// steps ends on a cycle, and following it on reaches a step checked, if it
/// has one.
fn cycle(graph: &Graph, existing: &[Entry], by: Option<u32>, waiting: &List<u32>, known: u32, total: u32) -> Problem {
    let mut at = None;
    for (index, left) in waiting.iter().enumerate() {
        if *left > 0 {
            at = Some(count(index));
            break;
        }
    }
    let Some(mut at) = at else {
        return Problem::Goal;
    };
    for moved in 0..total.saturating_mul(2) {
        if moved >= total && at >= known {
            break;
        }
        let mut next = None;
        for other in 0..total {
            let waits = graph.times(at, other) > 0 || parent_of(existing, by, other) == Some(at);
            if waits && waiting.get(other).copied().unwrap_or(0) > 0 {
                next = Some(other);
                break;
            }
        }
        let Some(other) = next else {
            return Problem::Goal;
        };
        at = other;
    }
    match at.checked_sub(known) {
        Some(step) => Problem::Cycle { step },
        None => Problem::Goal,
    }
}

/// Who added step `node` of a plan: the goal keeps it for the steps it has;
/// `by` added the steps checked.
fn parent_of(existing: &[Entry], by: Option<u32>, node: u32) -> Option<u32> {
    match existing.get(place(node)) {
        Some(entry) => entry.parent,
        None => by,
    }
}

/// A plan's dependencies, resolved: for each step, the places of the steps
/// it comes after that the plan has, at most as many as a step may name.
#[derive(Debug)]
struct Graph {
    /// Where each step's places start in `places`, and where the last ends.
    starts: List<u32>,
    places: List<u32>,
}

impl Graph {
    /// How many steps `node` comes after.
    fn after(&self, node: u32) -> u32 {
        let start = self.starts.get(node).copied().unwrap_or(0);
        let end = self.starts.get(node.saturating_add(1)).copied().unwrap_or(start);
        end.saturating_sub(start)
    }

    /// How many times `node` comes after `other`.
    fn times(&self, node: u32, other: u32) -> u32 {
        let start = self.starts.get(node).copied().unwrap_or(0);
        let end = self.starts.get(node.saturating_add(1)).copied().unwrap_or(start);
        let mut times: u32 = 0;
        for index in start..end {
            if self.places.get(index).copied() == Some(other) {
                times = times.saturating_add(1);
            }
        }
        times
    }
}

/// Resolves the names each step comes after, once.
fn resolved(limits: &Limits, existing: &[Entry], steps: &[Step]) -> Graph {
    let known = count(existing.len());
    let total = known.saturating_add(count(steps.len()));
    let mut starts = List::with_capacity(total.saturating_add(1));
    let mut places = List::with_capacity(total.saturating_mul(limits.dependencies));
    for node in 0..total {
        let pushed = starts.push(places.len());
        assert!(pushed.is_ok(), "a start for each step");
        let after: &[Box<[u8]>] = match existing.get(place(node)) {
            Some(entry) => &entry.after,
            None => match steps.get(place(node.saturating_sub(known))) {
                Some(step) => &step.after,
                None => &[],
            },
        };
        for name in after.iter().take(place(limits.dependencies)) {
            if let Some(other) = resolve(existing, steps, name) {
                let pushed = places.push(other);
                assert!(pushed.is_ok(), "no more places than steps may name");
            }
        }
    }
    let pushed = starts.push(places.len());
    assert!(pushed.is_ok(), "an end for the last step");
    Graph { starts, places }
}

/// The step named `name`: the first the plan has of that name, else the
/// first checked, counted after those the plan has.
fn resolve(existing: &[Entry], steps: &[Step], name: &[u8]) -> Option<u32> {
    if let Some(index) = entry_named(existing, name) {
        return Some(index);
    }
    let checked = first_named(steps, name)?;
    count(existing.len()).checked_add(checked)
}

/// The first of `entries` named `name`.
pub(crate) fn entry_named(entries: &[Entry], name: &[u8]) -> Option<u32> {
    for (index, entry) in entries.iter().enumerate() {
        if *entry.name == *name {
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
