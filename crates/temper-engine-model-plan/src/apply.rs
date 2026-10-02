//! What an outcome writes (engine-model.md, 4.4), once it is recorded on its
//! item and the item, and what the outcome touches, are read afresh. For each
//! kind of outcome the plan says the writes, in its own terms and keyed from
//! the outcome; or that the outcome is stale, because the item moved on while
//! the run worked; or that it is invalid, because it breaks the step's spec,
//! with feedback the run can act on.
//!
//! ```text
//! change      a change step's: on a branch still at its head; a repair for a failure is counted
//! verdict     an agent's review of a change, on its pull request's head still
//! report      an agent step's, once: it finishes the step
//! plan        a chatting session's: accepted, it makes the plan's items, and the session its goal
//! steps       a growing agent step's (it finishes the step), or a supervising session's:
//!             added to the goal's plan; beyond its envelope, a person accepts them first
//! tasks       a session's: items on their own, each carrying a step
//! reply       a session's: the outcome's own comment is the reply
//! escalation  any run's: the item is held, for its goal's session or a person
//! ```
//!
//! What the outcome says in words (a report, a reply, an escalation, a
//! verdict's reasons) is in the comment its parent posts before applying it,
//! so the plan writes no words of its own. Whether a write needs a person's
//! acceptance beyond the envelope's is the rules' call.

use alloc::boxed::Box;

use temper_lib::{Env, Queue};

use crate::accept::{Growing, accept, grow};
use crate::check::{Among, Found, Problem, Problems, check_steps, count, entry_named};
use crate::config::Config;
use crate::due::{Hold, Repair};
use crate::facts::{Facts, PullState};
use crate::limits::Limits;
use crate::plan::{Commit, Plan, Review, Step, Work};
use crate::record::{Goal, Progress, Record, Reviewed, Verdict};
use crate::write::{Key, Write};

/// A run's outcome, in the plan's terms.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Outcome {
    /// A change, pushed: the item's branch holds `head`. `repair` is the
    /// repair its run was due for, if it was one.
    Change {
        head: Commit,
        repair: Option<Repair>,
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
    /// Tasks to make: items on their own, each carrying a step.
    Tasks(Box<[Step]>),
    /// A session's reply.
    Reply,
    /// The run asks for a decision it cannot make.
    Escalation,
}

/// What applying an outcome comes to.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Applied {
    /// Make the writes in the caller's queue, in order, then go on as `then`
    /// says.
    Writes { accept: Accept, then: Then },
    /// The item moved on while the run worked: nothing of it is applied.
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
    /// goal's envelope.
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
    let progress = record.progress;
    match outcome {
        Outcome::Change { head, repair } => match &step.work {
            Work::Change(_) => changed(progress, facts, *head, *repair, out),
            Work::Agent(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Verdict { head, verdict } => match &step.work {
            Work::Change(spec) => match &spec.review {
                Review::Agent(_) => reviewed(progress, facts, *head, *verdict, out),
                Review::Person => not_allowed(),
            },
            Work::Agent(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Report => match &step.work {
            Work::Agent(_) => {
                if progress.finished {
                    return Applied::Stale(Stale::Finished);
                }
                out.push(Write::Progress(Progress { finished: true, ..progress }));
                writes(Accept::Rules)
            }
            Work::Change(_) | Work::Wait(_) | Work::Session(_) => not_allowed(),
        },
        Outcome::Plan(plan) => match &step.work {
            Work::Session(_) if record.goal.is_none() => proposed(config, env, plan, out),
            Work::Session(_) | Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Steps(steps) => match &step.work {
            Work::Agent(spec) if spec.grows => {
                if progress.finished {
                    return Applied::Stale(Stale::Finished);
                }
                let Some(goal) = goal else {
                    return invalid(Problem::NoGoal);
                };
                let by = entry_named(&goal.steps, &step.name);
                match grow(config, env, goal, by, steps, out) {
                    Ok(growing) => {
                        out.push(Write::Progress(Progress { finished: true, ..progress }));
                        writes(accepted_by(growing))
                    }
                    Err(problems) => Applied::Invalid(problems),
                }
            }
            Work::Session(_) if record.goal.is_some() => match goal {
                Some(goal) => grown(config, env, goal, steps, out),
                None => invalid(Problem::NoGoal),
            },
            Work::Agent(_) | Work::Session(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Tasks(tasks) => match &step.work {
            Work::Session(_) => tasked(config, env, tasks, out),
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Reply => match &step.work {
            Work::Session(_) => writes(Accept::Rules),
            Work::Agent(_) | Work::Change(_) | Work::Wait(_) => not_allowed(),
        },
        Outcome::Escalation => Applied::Writes { accept: Accept::Rules, then: Then::Hold(Hold::Escalated) },
    }
}

/// A change pushed to the item's branch. A repair for a failure counts
/// against the limit of repairs; a rebase onto a base that moved does not,
/// since how often that happens is bounded by the changes landing there.
fn changed(progress: Progress, facts: &Facts, head: Commit, repair: Option<Repair>, out: &mut Queue<Write>) -> Applied {
    if let Some(pull) = facts.pull {
        match pull.state {
            PullState::Open => {}
            PullState::Merged => return Applied::Stale(Stale::Landed),
            PullState::Closed => return Applied::Stale(Stale::Closed),
        }
    }
    if facts.branch != Some(head) {
        return Applied::Stale(Stale::Moved);
    }
    match repair {
        Some(Repair::CiFailed | Repair::ChangesRequested | Repair::Conflicts) => {
            out.push(Write::Progress(Progress { repairs: progress.repairs.saturating_add(1), ..progress }));
        }
        Some(Repair::BaseMoved) | None => {}
    }
    writes(Accept::Rules)
}

/// An agent's verdict on the change's pull request, at `head`.
fn reviewed(progress: Progress, facts: &Facts, head: Commit, verdict: Verdict, out: &mut Queue<Write>) -> Applied {
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
    out.push(Write::Progress(Progress { review: Some(Reviewed { head, verdict }), ..progress }));
    writes(Accept::Rules)
}

/// A plan proposed by a chatting session: the writes that make it, once it is
/// accepted as the rules say.
fn proposed(config: &Config, env: &Env<Limits>, plan: &Plan, out: &mut Queue<Write>) -> Applied {
    match accept(config, env, plan, out) {
        Ok(()) => writes(Accept::Rules),
        Err(problems) => Applied::Invalid(problems),
    }
}

/// Steps added to `goal`'s plan by its session.
fn grown(config: &Config, env: &Env<Limits>, goal: &Goal, steps: &[Step], out: &mut Queue<Write>) -> Applied {
    match grow(config, env, goal, None, steps, out) {
        Ok(growing) => writes(accepted_by(growing)),
        Err(problems) => Applied::Invalid(problems),
    }
}

/// Who accepts growth: within the envelope, the rules alone.
fn accepted_by(growing: Growing) -> Accept {
    match growing {
        Growing::Within => Accept::Rules,
        Growing::Beyond => Accept::Person,
    }
}

/// Tasks a session makes: items on their own, keyed by their place among the
/// outcome's tasks.
fn tasked(config: &Config, env: &Env<Limits>, tasks: &[Step], out: &mut Queue<Write>) -> Applied {
    let mut found = Found::new();
    if tasks.is_empty() {
        found.add(Problem::NoSteps);
    }
    let _estimate: u64 = check_steps(config, &env.limits, &[], None, tasks, Among::Alone, &mut found);
    if !found.is_empty() {
        return Applied::Invalid(found.into_problems());
    }
    for (index, task) in tasks.iter().enumerate() {
        out.push(Write::Create {
            key: Key::Task(count(index)),
            record: Box::new(Record { step: task.clone(), progress: Progress::NEW, goal: None }),
        });
    }
    writes(Accept::Rules)
}

fn writes(accept: Accept) -> Applied {
    Applied::Writes { accept, then: Then::Wait }
}

fn not_allowed() -> Applied {
    invalid(Problem::NotAllowed)
}

fn invalid(problem: Problem) -> Applied {
    Applied::Invalid(Problems { listed: Box::new([problem]), more: 0 })
}
