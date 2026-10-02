//! Accepting a plan, and growing one (engine-model.md, 5.2). An accepted plan
//! becomes an item for each step, keyed by the step's name, each with its step
//! in its record, and the goal's part of the record on the item that proposed
//! it. Steps added to an accepted plan are checked as a plan's are, against
//! the steps it has (a step is done only once the steps it added are, so they
//! may not come after it), and then against its envelope: growth within it
//! needs no one's acceptance, growth beyond it a person's. Whether a plan needs a
//! person's acceptance in the first place is the rules' call.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, List, Queue};

use crate::check::{Among, Found, Problem, Problems, check_plan, check_steps, count};
use crate::config::Config;
use crate::limits::Limits;
use crate::plan::{Envelope, Growth, Plan, Step, Work};
use crate::record::{Entry, Goal, Progress, Record};
use crate::write::{Key, Write};

/// Whether steps added to a plan are within its envelope.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Growing {
    /// Within it: the rules decide, as for any write.
    Within,
    /// Beyond it: a person accepts the growth first, whatever the rules say.
    Beyond,
}

/// The writes that make `plan` exist, into `out`, which has room for
/// [`max_out`](crate::max_out) of them: an item for each step, then the goal's
/// part of the record of the item that proposed it. Or what is wrong with it,
/// and nothing written.
pub fn accept(config: &Config, env: &Env<Limits>, plan: &Plan, out: &mut Queue<Write>) -> Result<(), Problems> {
    let mut found = Found::new();
    let estimate = check_plan(config, &env.limits, plan, &mut found);
    if !found.is_empty() {
        return Err(found.into_problems());
    }
    for step in &plan.steps {
        out.push(create(step));
    }
    out.push(Write::Goal(Goal {
        steps: joined(&[], None, &plan.steps),
        envelope: plan.envelope.clone(),
        budget: plan.budget,
        estimate,
        growth: Growth::NONE,
    }));
    Ok(())
}

/// The writes that add `steps` to `goal`'s plan, into `out`, which has room
/// for [`max_out`](crate::max_out) of them: an item for each step, then the
/// goal's part of its item's record. `by` is the step of the plan that adds
/// them, by its place among the goal's steps, which makes them its children;
/// none if the goal's session adds them. Whether the growth is within the
/// goal's envelope, or what is wrong with it, and nothing written.
pub fn grow(
    config: &Config,
    env: &Env<Limits>,
    goal: &Goal,
    by: Option<u32>,
    steps: &[Step],
    out: &mut Queue<Write>,
) -> Result<Growing, Problems> {
    let mut found = Found::new();
    if steps.is_empty() {
        found.add(Problem::NoSteps);
    }
    let added = check_steps(config, &env.limits, &goal.steps, by, steps, Among::Plan, &mut found);
    let estimate = goal.estimate.saturating_add(added);
    if estimate > goal.budget {
        found.add(Problem::OverBudget { estimate, budget: goal.budget });
    }
    if !found.is_empty() {
        return Err(found.into_problems());
    }
    let mut growth = goal.growth;
    let mut targets = true;
    for step in steps {
        growth = grown(growth, step);
        targets = targets && lands_within(&goal.envelope, step);
    }
    let growing = if targets && fits(&goal.envelope, growth) { Growing::Within } else { Growing::Beyond };
    for step in steps {
        out.push(create(step));
    }
    out.push(Write::Goal(Goal {
        steps: joined(&goal.steps, by, steps),
        envelope: goal.envelope.clone(),
        budget: goal.budget,
        estimate,
        growth,
    }));
    Ok(growing)
}

/// The item a step is made as, keyed by the step's name.
fn create(step: &Step) -> Write {
    Write::Create {
        key: Key::Step(copy_of(&step.name)),
        record: Box::new(Record { step: step.clone(), progress: Progress::NEW, goal: None }),
    }
}

/// The steps of `existing`, then `steps`, added by `by`.
fn joined(existing: &[Entry], by: Option<u32>, steps: &[Step]) -> Box<[Entry]> {
    let total = count(existing.len()).saturating_add(count(steps.len()));
    let mut entries = List::with_capacity(total);
    for entry in existing {
        let pushed = entries.push(entry.clone());
        assert!(pushed.is_ok(), "room for every step");
    }
    for step in steps {
        let entry = Entry { name: copy_of(&step.name), after: step.after.clone(), parent: by };
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

/// Whether `step` lands, if it is a change, where `envelope` lets changes
/// land.
fn lands_within(envelope: &Envelope, step: &Step) -> bool {
    match &step.work {
        Work::Change(spec) => {
            for target in &envelope.into {
                if target.repository == step.repository && *target.base == *spec.base {
                    return true;
                }
            }
            false
        }
        Work::Agent(_) | Work::Wait(_) | Work::Session(_) => true,
    }
}
