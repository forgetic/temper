//! What an outcome writes (engine-model.md, 4.4), once it is recorded on its
//! item and the item, and what the outcome touches, are read afresh. For each
//! kind of outcome the plan says the writes, in its own terms and keyed from
//! the outcome; or that the outcome is stale, because the item moved on while
//! the run worked; or that it is invalid, because it breaks the step's spec,
//! with feedback the run can act on.
//!
//! ```text
//! change      a change step's: on a branch still at its head; a repair, or a rebase, is counted
//! verdict     an agent's review of a change, on its pull request's head still
//! report      an agent step's, once: it finishes the step
//! plan        a chatting session's: accepted, it makes the plan's items, and the session its goal
//! steps       a growing agent step's (it finishes the step), or a supervising session's:
//!             added to the goal's plan; beyond its envelope, a person accepts them first
//! tasks       a session's: items on their own; a supervising session's join its goal's plan,
//!             within its envelope and budget, as steps of no dependency
//! reply       a session's: the outcome's own comment is the reply
//! finished    a session's: its last turn, which finishes it
//! release     a supervising session's: one of its goal's held steps, released
//! escalation  any run's: the item is held, for its goal's session or a person
//! ```
//!
//! Every outcome applied clears the claim from the step's progress, and that
//! write comes last: it is the step's part of the commit point. What the
//! outcome says in words (a report, a reply, an escalation, a verdict's
//! reasons) is in the comment its parent posts before applying it, so the
//! plan writes no words of its own. Whether a write needs a person's
//! acceptance beyond the envelope's is the rules' call.
//!
//! A person's answers to what the plan asked of them have entry points of
//! their own: [`release`], when they release a held item, and [`rejected`],
//! when they reject one of its proposals.

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Queue};

use crate::accept::{Growing, accept, grow, reaccepted};
use crate::check::{Among, Found, Problem, Problems, check_steps, count, entry_named};
use crate::config::Config;
use crate::due::{Hold, Repair, Why};
use crate::facts::{Facts, PullState, Relations};
use crate::limits::Limits;
use crate::plan::{Commit, Plan, Review, Step, Work};
use crate::record::{Goal, Progress, Record, Reviewed, Verdict};
use crate::write::{Key, Write};

/// A run's outcome, in the plan's terms.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// A change, pushed: the item's branch holds `head`.
    Change {
        head: Commit,
    },
    /// A verdict on a change's exact head.
    Verdict {
        head: Commit,
        verdict: Verdict,
    },
    Report,
    /// A plan proposed, whose goal is the item.
    Plan(Plan),
    /// Steps added to the goal's plan.
    Steps(Box<[Step]>),
    /// Tasks to make, each carrying a step.
    Tasks(Box<[Step]>),
    /// A session's reply.
    Reply,
    /// A session's last turn: it is finished.
    Finished,
    /// A supervising session releases the step of its goal named `step`.
    Release {
        step: Box<[u8]>,
    },
    /// The run asks for a decision it cannot make.
    Escalation,
}

/// What applying an outcome comes to.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Applied {
    /// Make the writes in the caller's queue, in order, then go on as `then`
    /// says. `estimate` is the tokens the steps they make are estimated to
    /// spend, for the rules' bounds on spending.
    Writes { accept: Accept, then: Then, estimate: u64 },
    /// The item moved on while the run worked: nothing of it is applied,
    /// save the step's progress in the caller's queue, if any: a repair or
    /// a rebase whose change went stale counts all the same, so a change
    /// whose branch moves under every run is held once they are spent.
    Stale(Stale),
    /// It breaks the step's spec: feedback for the run.
    Invalid(Problems),
}

/// Who accepts the writes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Accept {
    /// The rules decide, as for any write.
    Rules,
    /// A person accepts them first, whatever the rules say: growth beyond the
    /// goal's envelope. If they reject them, [`rejected`] says what follows.
    Person,
}

/// What the item does once the writes are made.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Then {
    /// It waits for what is due next.
    Wait,
    /// It is held for a person.
    Hold(Hold),
}

/// How the item moved on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stale {
    /// Its branch, or its pull request's head, is no longer the outcome's.
    Moved,
    /// Its change has landed.
    Landed,
    /// Its pull request was closed.
    Closed,
    /// Its step has finished already.
    Finished,
}

/// What `outcome` writes, for the item whose record's plan part is `record`,
/// from `facts` read afresh, into `out`, which has room for
/// [`max_out`](crate::max_out) writes. `goal` is the goal's part of the
/// record of the goal the item is under, or of its own if it is one.
pub fn apply(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    goal: Option<&Goal>,
    facts: &Facts,
    outcome: &Outcome,
    out: &mut Queue<Write>,
) -> Applied {
    let step = &record.step;
    let cleared = Progress { running: None, ..record.progress };
    let supervising = record.goal.is_some();
    match outcome {
        Outcome::Change { head } => match &step.work {
            Work::Change(_) => changed(record.progress, facts, *head, out),
            Work::Agent(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Verdict { head, verdict } => match &step.work {
            Work::Change(spec) => match &spec.review {
                Review::Agent(_) => reviewed(cleared, facts, *head, *verdict, out),
                Review::Person => not_allowed(),
            },
            Work::Agent(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Report => match &step.work {
            Work::Agent(_) => {
                if cleared.finished {
                    return Applied::Stale(Stale::Finished);
                }
                written(Progress { finished: true, ..cleared }, Accept::Rules, 0, out)
            }
            Work::Change(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Plan(plan) => match &step.work {
            Work::Session(_) if !supervising => proposed(config, env, record.progress.runs, cleared, plan, out),
            // The plan applied again, once the goal's record has landed.
            Work::Session(_) => match reaccepted_plan(record, plan, out) {
                Some(estimate) => written(cleared, Accept::Rules, estimate, out),
                None => not_allowed(),
            },
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Steps(steps) => match &step.work {
            // A growth applied again after its step finished joins nothing
            // twice: the goal finds its steps already joined.
            Work::Agent(spec) if spec.grows => match goal {
                Some(goal) => {
                    let by = entry_named(&goal.steps, &step.name);
                    let finished = Progress { finished: true, ..cleared };
                    grown(config, env, goal, (by, cleared.runs), steps, finished, out)
                }
                None => invalid(Problem::NoGoal),
            },
            Work::Session(_) if supervising => match goal {
                Some(goal) => grown(config, env, goal, (None, cleared.runs), steps, cleared, out),
                None => invalid(Problem::NoGoal),
            },
            Work::Agent(_) | Work::Session(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Tasks(tasks) => match &step.work {
            Work::Session(_) => match goal {
                Some(goal) if supervising => supervised(config, env, goal, tasks, cleared, out),
                Some(_) | None => tasked(config, env, tasks, facts.children, cleared, out),
            },
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Reply => match &step.work {
            Work::Session(_) => written(cleared, Accept::Rules, 0, out),
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Finished => match &step.work {
            Work::Session(_) => written(Progress { finished: true, ..cleared }, Accept::Rules, 0, out),
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Release { step: name } => match &step.work {
            Work::Session(_) if supervising => match goal {
                Some(goal) if entry_named(&goal.steps, name).is_some() => {
                    out.push(Write::Release { step: copy_of(name) });
                    written(cleared, Accept::Rules, 0, out)
                }
                Some(_) | None => invalid(Problem::UnknownStep),
            },
            Work::Session(_) | Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Escalation => {
            out.push(Write::Progress(cleared));
            Applied::Writes { accept: Accept::Rules, then: Then::Hold(Hold::Escalated), estimate: 0 }
        }
    }
}

/// What a person's release of a held item writes, into `out`: the release,
/// in its step's progress, which lifts whatever held it. The repairs and
/// rebases and rejected proposals are counted afresh; a decision made before
/// it counts for nothing, so a rejected step waits for a new one; its waits
/// on the forge count from it; and a pull request closed unmerged is opened
/// again.
pub fn release(env: &Env<Limits>, record: &Record, facts: &Facts, out: &mut Queue<Write>) {
    if let Some(pull) = facts.pull {
        match pull.state {
            PullState::Closed => out.push(Write::ReopenPull),
            PullState::Open | PullState::Merged => {}
        }
    }
    out.push(Write::Progress(Progress {
        running: None,
        repairs: 0,
        rebases: 0,
        rejections: 0,
        released: Some(env.now),
        ..record.progress
    }));
}

/// What a person's rejection of one of the item's proposals (a plan, or
/// growth beyond its goal's envelope) writes, into `out`: the rejection,
/// counted in its step's progress. The item runs again, the rejection in its
/// brief's comments; past as many rejections as the limits allow, it is
/// held.
pub fn rejected(env: &Env<Limits>, record: &Record, out: &mut Queue<Write>) -> Then {
    let rejections = record.progress.rejections.saturating_add(1);
    out.push(Write::Progress(Progress { running: None, rejections, ..record.progress }));
    if rejections >= env.limits.rejections {
        return Then::Hold(Hold::Rejected);
    }
    Then::Wait
}

/// A change pushed to the item's branch, by the run claimed in `progress`. A
/// repair for a failure counts against the limit of repairs, a rebase
/// (conflicts, or a base that moved) against the limit of rebases.
fn changed(progress: Progress, facts: &Facts, head: Commit, out: &mut Queue<Write>) -> Applied {
    if let Some(pull) = facts.pull {
        match pull.state {
            PullState::Open => {}
            PullState::Merged => return Applied::Stale(Stale::Landed),
            PullState::Closed => return Applied::Stale(Stale::Closed),
        }
    }
    let cleared = Progress { running: None, ..progress };
    let Progress { repairs, rebases, .. } = cleared;
    let counted = match repair_of(progress) {
        Some(Repair::CiFailed | Repair::ChangesRequested) => Progress { repairs: repairs.saturating_add(1), ..cleared },
        Some(Repair::BaseMoved | Repair::Conflicts) => Progress { rebases: rebases.saturating_add(1), ..cleared },
        None => cleared,
    };
    if facts.branch != Some(head) {
        // Its push went nowhere, the branch moved under it: a repair or a
        // rebase counts all the same.
        if counted != cleared {
            out.push(Write::Progress(counted));
        }
        return Applied::Stale(Stale::Moved);
    }
    written(counted, Accept::Rules, 0, out)
}

/// An agent's verdict on the change's pull request, at `head`.
fn reviewed(cleared: Progress, facts: &Facts, head: Commit, verdict: Verdict, out: &mut Queue<Write>) -> Applied {
    let Some(pull) = facts.pull else {
        return Applied::Stale(Stale::Moved);
    };
    match pull.state {
        PullState::Open => {}
        PullState::Merged => return Applied::Stale(Stale::Landed),
        PullState::Closed => return Applied::Stale(Stale::Closed),
    }
    if pull.head != head {
        return Applied::Stale(Stale::Moved);
    }
    written(Progress { review: Some(Reviewed { head, verdict }), ..cleared }, Accept::Rules, 0, out)
}

/// A plan proposed by a chatting session, in its run counted `run`: the
/// writes that make it, once it is accepted as the rules say.
fn proposed(
    config: &Config,
    env: &Env<Limits>,
    run: u32,
    cleared: Progress,
    plan: &Plan,
    out: &mut Queue<Write>,
) -> Applied {
    match accept(config, env, plan, run, out) {
        Ok(estimate) => written(cleared, Accept::Rules, estimate, out),
        Err(problems) => Applied::Invalid(problems),
    }
}

/// Steps added to `goal`'s plan by the run counted `run` of `by`, then the
/// step's progress.
fn grown(
    config: &Config,
    env: &Env<Limits>,
    goal: &Goal,
    (by, run): (Option<u32>, u32),
    steps: &[Step],
    progress: Progress,
    out: &mut Queue<Write>,
) -> Applied {
    match grow(config, env, goal, by, run, steps, out) {
        Ok(grown) => {
            let accept = match grown.growing {
                Growing::Within => Accept::Rules,
                Growing::Beyond => Accept::Person,
            };
            written(progress, accept, grown.estimate, out)
        }
        Err(problems) => Applied::Invalid(problems),
    }
}

/// Tasks a supervising session makes: they join its goal's plan as steps of
/// no dependency, so they count against its envelope and budget.
fn supervised(
    config: &Config,
    env: &Env<Limits>,
    goal: &Goal,
    tasks: &[Step],
    cleared: Progress,
    out: &mut Queue<Write>,
) -> Applied {
    let mut found = Found::new();
    if count(tasks.len()) > env.limits.tasks {
        found.add(Problem::TooManyTasks { max: env.limits.tasks });
    }
    for (index, task) in tasks.iter().enumerate() {
        if !task.after.is_empty() {
            found.add(Problem::UnknownDependency { step: count(index), dependency: 0 });
        }
    }
    if !found.is_empty() {
        return Applied::Invalid(found.into_problems());
    }
    grown(config, env, goal, (None, cleared.runs), tasks, cleared, out)
}

/// The writes of the plan a supervising session proposed, applied again
/// once its goal's record landed, if they are its goal's.
fn reaccepted_plan(record: &Record, plan: &Plan, out: &mut Queue<Write>) -> Option<u64> {
    let goal = record.goal.as_ref()?;
    reaccepted(goal, record.progress.runs, &plan.steps, out)
}

/// Tasks a chatting session makes: items on their own, keyed by their place
/// among the outcome's tasks.
fn tasked(
    config: &Config,
    env: &Env<Limits>,
    tasks: &[Step],
    children: Relations,
    cleared: Progress,
    out: &mut Queue<Write>,
) -> Applied {
    let mut found = Found::new();
    if tasks.is_empty() {
        found.add(Problem::NoSteps);
    }
    // The session is done only once its tasks are, so it keeps them, as
    // many as a plan's steps that are not done.
    let live = children.total.saturating_sub(children.done).saturating_add(count(tasks.len()));
    if live > env.limits.steps {
        found.add(Problem::TooManyChildren { max: env.limits.steps });
    }
    let checked = check_steps(config, &env.limits, &[], None, tasks, Among::Alone, &mut found);
    if !found.is_empty() {
        return Applied::Invalid(found.into_problems());
    }
    for (index, task) in tasks.iter().enumerate() {
        out.push(Write::Create {
            key: Key::Task(count(index)),
            record: Box::new(Record { step: task.clone(), progress: Progress::NEW, goal: None }),
        });
    }
    written(cleared, Accept::Rules, checked.estimate, out)
}

/// The step's progress, written last, and the writes accepted by `accept`.
fn written(progress: Progress, accept: Accept, estimate: u64, out: &mut Queue<Write>) -> Applied {
    out.push(Write::Progress(progress));
    Applied::Writes { accept, then: Then::Wait, estimate }
}

fn not_allowed() -> Applied {
    invalid(Problem::NotAllowed)
}

fn invalid(problem: Problem) -> Applied {
    Applied::Invalid(Problems { listed: Box::new([problem]), more: 0 })
}

/// The repair the run whose outcome this is was claimed for.
fn repair_of(progress: Progress) -> Option<Repair> {
    match progress.running {
        Some(Why::Repair(repair)) => Some(repair),
        Some(Why::Work | Why::Produce | Why::Review { .. } | Why::Turn) | None => None,
    }
}
