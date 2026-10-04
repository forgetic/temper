//! Accepting a plan, and growing one (engine-domain.md, 5.2). An accepted plan
//! becomes an item for each step, keyed by the step's name, each with its step
//! in its record, made dependencies first; then the goal's part of the record,
//! on the item that proposed it. Steps added to an accepted plan are checked
//! as a plan's are, against the steps it has (a step is done only once the
//! steps it added are, so they may not come after it), and then against its
//! envelope: growth within it needs no one's acceptance; growth beyond it a
//! person's, and once they accept it, it widens the envelope by what it
//! added. Whether a plan needs a person's acceptance in the first place is
//! the rules' call.
//!
//! Growth is repeat-safe: applied again after a restart that came between its
//! writes, it finds its steps already joined to the goal (the same names,
//! coming after the same steps, added by the same run of the same step) and
//! asks for the same writes as if they were new, so the goal's record and the
//! growing step's may land in either order. So is a plan, applied again once
//! its goal's record has landed.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{Env, List, Queue};

use crate::check::{
    Among, Checked, Found, Problem, Problems, check_plan, check_steps, cost, count, entry_named, place,
};
use crate::config::Config;
use crate::limits::Limits;
use crate::plan::{Envelope, Growth, Plan, Repository, Step, Target, Work};
use crate::record::{Entry, Goal, Progress, Record};
use crate::write::{Key, Write};

/// Steps added to a plan: whether they are within its envelope, and the
/// tokens they are estimated to spend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Grown {
    pub growing: Growing,
    pub estimate: u64,
}

/// Whether steps added to a plan are within its envelope.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Growing {
    /// Within it: the rules decide, as for any write.
    Within,
    /// Beyond it: a person accepts the growth first, whatever the rules say,
    /// and the goal written widens the envelope by it.
    Beyond,
}

/// The writes that make `plan` exist, into `out`, which has room for
/// [`max_out`](crate::max_out) of them: an item for each step, dependencies
/// first, then the goal's part of the record of the item that proposed it,
/// in its run counted `run`. The tokens its steps are estimated to spend, or
/// what is wrong with it, and nothing written.
pub fn accept(
    config: &Config,
    env: &Env<Limits>,
    plan: &Plan,
    run: u32,
    out: &mut Queue<Write>,
) -> Result<u64, Problems> {
    let mut found = Found::new();
    let checked = check_plan(config, &env.limits, plan, &mut found);
    if !found.is_empty() {
        return Err(found.into_problems());
    }
    create(&plan.steps, &checked, out);
    out.push(Write::Goal(Goal {
        steps: joined(&[], None, run, &plan.steps, &checked),
        envelope: plan.envelope.clone(),
        budget: plan.budget,
        estimate: checked.estimate,
        growth: Growth::NONE,
    }));
    Ok(checked.estimate)
}

/// The writes that add `steps` to `goal`'s plan, into `out`, which has room
/// for [`max_out`](crate::max_out) of them: an item for each step,
/// dependencies first, then the goal's part of its item's record. `by` is the
/// step of the plan that adds them, by its place among the goal's steps,
/// which makes them its children; none if the goal's session adds them; and
/// `run` counts the run of it whose outcome they are. Whether the growth is
/// within the goal's envelope, or what is wrong with it, and nothing written.
pub fn grow(
    config: &Config,
    env: &Env<Limits>,
    goal: &Goal,
    by: Option<u32>,
    run: u32,
    steps: &[Step],
    out: &mut Queue<Write>,
) -> Result<Grown, Problems> {
    if joined_already(goal, by, run, steps) {
        return Ok(rejoined(goal, steps, out));
    }
    let mut found = Found::new();
    if steps.is_empty() {
        found.add(Problem::NoSteps);
    }
    let checked = check_steps(config, &env.limits, &goal.steps, by, steps, Among::Plan, &mut found);
    let estimate = goal.estimate.saturating_add(checked.estimate);
    if estimate > goal.budget {
        found.add(Problem::OverBudget { estimate, budget: goal.budget });
    }
    if !found.is_empty() {
        return Err(found.into_problems());
    }
    let mut growth = goal.growth;
    let mut placed = true;
    for step in steps {
        growth = grown(growth, step);
        placed = placed && within_places(&goal.envelope, step);
    }
    let (growing, envelope) = if placed && fits(&goal.envelope, growth) {
        (Growing::Within, goal.envelope.clone())
    } else {
        (Growing::Beyond, widened(&goal.envelope, growth, steps))
    };
    create(steps, &checked, out);
    out.push(Write::Goal(Goal {
        steps: joined(&goal.steps, by, run, steps, &checked),
        envelope,
        budget: goal.budget,
        estimate,
        growth,
    }));
    Ok(Grown { growing, estimate: checked.estimate })
}

/// The writes of a plan applied again, once its goal's record has landed:
/// its run counted `run` made the goal's steps, and these are they.
pub(crate) fn reaccepted(goal: &Goal, run: u32, steps: &[Step], out: &mut Queue<Write>) -> Option<u64> {
    if !joined_already(goal, None, run, steps) || goal.steps.len() != steps.len() {
        return None;
    }
    Some(rejoined(goal, steps, out).estimate)
}

/// Whether every one of `steps` already joined `goal`'s plan, as the run
/// counted `run` of `by` added it: an outcome applied again. A run claimed
/// for nothing counts none.
fn joined_already(goal: &Goal, by: Option<u32>, run: u32, steps: &[Step]) -> bool {
    if steps.is_empty() || run == 0 {
        return false;
    }
    for step in steps {
        let Some(index) = entry_named(&goal.steps, &step.name) else {
            return false;
        };
        let Some(entry) = goal.steps.get(place(index)) else {
            return false;
        };
        if entry.parent != by || entry.run != run || entry.after != step.after {
            return false;
        }
    }
    true
}

/// The writes of a growth applied again: its items, in the order they
/// joined the goal, found by their keys, and the goal as it is.
fn rejoined(goal: &Goal, steps: &[Step], out: &mut Queue<Write>) -> Grown {
    let mut estimate: u64 = 0;
    for entry in &goal.steps {
        for step in steps {
            if *step.name == *entry.name {
                out.push(item(step));
                estimate = estimate.saturating_add(cost(step));
            }
        }
    }
    out.push(Write::Goal(goal.clone()));
    Grown { growing: Growing::Within, estimate }
}

/// An item for each of `steps`, in the order checked.
fn create(steps: &[Step], checked: &Checked, out: &mut Queue<Write>) {
    for index in &checked.order {
        let step = steps.get(place(*index)).expect("the order places the steps checked");
        out.push(item(step));
    }
}

/// The item a step is made as, keyed by the step's name.
fn item(step: &Step) -> Write {
    Write::Create {
        key: Key::Step(copy_of(&step.name)),
        record: Box::new(Record { step: step.clone(), progress: Progress::NEW, goal: None }),
    }
}

/// The steps of `existing`, then `steps` in the order checked, added by the
/// run counted `run` of `by`.
fn joined(existing: &[Entry], by: Option<u32>, run: u32, steps: &[Step], checked: &Checked) -> Box<[Entry]> {
    let total = count(existing.len()).saturating_add(count(steps.len()));
    let mut entries = List::with_capacity(total);
    for entry in existing {
        let pushed = entries.push(entry.clone());
        assert!(pushed.is_ok(), "room for every step");
    }
    for index in &checked.order {
        let step = steps.get(place(*index)).expect("the order places the steps checked");
        let entry = Entry { name: copy_of(&step.name), after: step.after.clone(), parent: by, run };
        let pushed = entries.push(entry);
        assert!(pushed.is_ok(), "room for every step");
    }
    entries.into_boxed()
}

/// `growth` with `step` added to it.
fn grown(growth: Growth, step: &Step) -> Growth {
    let Growth { agents, changes, waits, sessions } = growth;
    match &step.work {
        Work::Agent(_) => Growth { agents: agents.saturating_add(1), ..growth },
        Work::Change(_) => Growth { changes: changes.saturating_add(1), ..growth },
        Work::Wait(_) => Growth { waits: waits.saturating_add(1), ..growth },
        Work::Session(_) => Growth { sessions: sessions.saturating_add(1), ..growth },
    }
}

/// Whether `envelope` allows as many steps of each primitive as `growth`.
fn fits(envelope: &Envelope, growth: Growth) -> bool {
    growth.agents <= envelope.agents
        && growth.changes <= envelope.changes
        && growth.waits <= envelope.waits
        && growth.sessions <= envelope.sessions
}

/// Whether `step` is in a repository `envelope` names and, if it is a change,
/// lands where `envelope` lets changes land.
fn within_places(envelope: &Envelope, step: &Step) -> bool {
    if !envelope.repositories.contains(&step.repository) {
        return false;
    }
    match &step.work {
        Work::Change(spec) => targets(&envelope.into, step.repository, &spec.base),
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => true,
    }
}

/// Whether `into` has the branch `base` of `repository`.
fn targets(into: &[Target], repository: Repository, base: &[u8]) -> bool {
    for target in into {
        if target.repository == repository && *target.base == *base {
            return true;
        }
    }
    false
}

/// `envelope` widened to hold `growth`, and the repositories and branches of
/// `steps`.
fn widened(envelope: &Envelope, growth: Growth, steps: &[Step]) -> Envelope {
    let added = count(steps.len());
    let mut repositories = List::with_capacity(count(envelope.repositories.len()).saturating_add(added));
    for repository in &envelope.repositories {
        let pushed = repositories.push(*repository);
        assert!(pushed.is_ok(), "room for every repository");
    }
    let mut into = List::with_capacity(count(envelope.into.len()).saturating_add(added));
    for target in &envelope.into {
        let pushed = into.push(target.clone());
        assert!(pushed.is_ok(), "room for every branch");
    }
    for step in steps {
        if !repositories.as_slice().contains(&step.repository) {
            let pushed = repositories.push(step.repository);
            assert!(pushed.is_ok(), "room for every repository");
        }
        match &step.work {
            Work::Change(spec) => {
                if !targets(into.as_slice(), step.repository, &spec.base) {
                    let target = Target { repository: step.repository, base: copy_of(&spec.base) };
                    let pushed = into.push(target);
                    assert!(pushed.is_ok(), "room for every branch");
                }
            }
            Work::Agent(_) | Work::Wait(_) | Work::Session(_) => {}
        }
    }
    Envelope {
        agents: envelope.agents.max(growth.agents),
        changes: envelope.changes.max(growth.changes),
        waits: envelope.waits.max(growth.waits),
        sessions: envelope.sessions.max(growth.sessions),
        repositories: repositories.into_boxed(),
        into: into.into_boxed(),
    }
}
