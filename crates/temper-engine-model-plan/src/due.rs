//! What is due for an item (engine-model.md, 5.3), from its step and the facts
//! about it: nothing yet, and what it waits on; a run, and the parts of its
//! charter the plan decides; an engine action (4.5); done; or hold for a
//! person. Every step waits for its dependencies first. Then, by primitive:
//!
//! ```text
//! agent    gate: accepted? ─► run ─► (it reports) ─► its children done ─► done
//! wait     steps: done | decision: accepted ─► done, rejected ─► hold
//!          time: done once it has passed, after the last dependency done
//! session  goal's steps all done ─► done | gate: accepted? ─► woken ─► run
//! change   no branch ─► run: produce
//!          branch, no pull request ─► open it
//!          pull request merged ─► done | closed unmerged ─► hold
//!          open:  conflicts, base moved, CI failed ─► run: repair
//!                 CI pending or not reported ─► wait
//!                 changes asked for on the head ─► run: repair
//!                 review: a person's approval, or an agent's run and verdict
//!                 gates: approvals, a person's acceptance
//!                 mergeable not yet known ─► wait
//!                 clean ─► merge at exactly the head
//! ```
//!
//! A change repaired as many times as the limits allow is held for a person
//! when it needs another repair. Which sections a run's brief carries follows
//! from why it runs: a repair's say why it repairs (9).

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Queue, Time};

use crate::config::Config;
use crate::facts::{Ci, Decision, Facts, Mergeable, Pull, PullState};
use crate::limits::Limits;
use crate::plan::{
    AgentSpec, Budget, ChangeSpec, Charter, Commit, Gate, Grants, Resume, Review, SessionSpec, Step, WaitSpec, Work,
};
use crate::record::{Record, Reviewed, Verdict};
use crate::write::Write;

/// What is due for an item now.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Due {
    /// Nothing yet: the item waits on this.
    Nothing(Waits),
    /// A run, with these parts of its charter.
    Run(Run),
    /// An engine action: the writes it makes are in the caller's queue.
    Act(Action),
    /// The step is done: the writes in the caller's queue finish it, closing
    /// its item.
    Done,
    /// Hold the item for a person, and why.
    Hold(Hold),
}

/// What an item waits on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Waits {
    /// The steps it comes after.
    Dependencies,
    /// The steps it added.
    Children,
    /// A person's acceptance, which a gate asks for.
    Acceptance,
    /// A person's decision, which a wait waits for.
    Decision,
    /// This time to pass.
    Time(Time),
    /// CI on its pull request's head.
    Ci,
    /// A person's review of its pull request's head.
    Review,
    /// More people approving its pull request's head, which a gate asks for.
    Approvals,
    /// The forge to say whether its pull request merges cleanly.
    Mergeable,
    /// An inbox event its wake rule lets through.
    Wake,
}

/// An engine action that is due.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Action {
    /// Open the pull request of a pushed change.
    OpenPull,
    /// Merge a change whose gates hold, at exactly its head.
    Merge,
}

/// Why an item is held for a person.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// A person rejected it: on a wait for their decision, or on a gate of
    /// their acceptance.
    Rejected,
    /// Its change needs another repair, past the limit.
    Repairs,
    /// Its pull request was closed without being merged.
    PullClosed,
}

/// A run that is due: the parts of its charter the plan decides
/// (agent-model.md, 4.1). Its parent adds the rest (the item's workstream,
/// the models) and renders the brief.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Run {
    pub why: Why,
    /// The sections its brief carries.
    pub sections: Sections,
    /// What it may finish with: its outcome spec.
    pub finish: Finish,
    pub grants: Grants,
    pub budget: Budget,
    /// The guidance its brief carries, in the plan's words.
    pub instructions: Box<[u8]>,
    /// The template whose guidance its brief carries too, by its index in the
    /// configuration.
    pub template: Option<u32>,
    /// Whether it resumes the snapshot the item's last run parked, rather
    /// than starting fresh from the forge.
    pub resume: bool,
}

/// Why a run is due.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Why {
    /// An agent step's work.
    Work,
    /// Producing a change.
    Produce,
    /// Repairing a change, and why it needs it.
    Repair(Repair),
    /// Reviewing a change at exactly `head`.
    Review { head: Commit },
    /// A session's turn.
    Turn,
}

/// Why a change needs repairing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Repair {
    CiFailed,
    ChangesRequested,
    BaseMoved,
    Conflicts,
}

/// The sections a brief carries (engine-model.md, section 9).
#[expect(clippy::struct_excessive_bools, reason = "each section is chosen on its own: a set of flags")]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Sections {
    /// The item and its lineage.
    pub item: bool,
    /// The comments since the run's last turn.
    pub comments: bool,
    /// The outcomes of its dependencies.
    pub dependencies: bool,
    /// The CI failures on its pull request's head, with their output.
    pub ci: bool,
    /// Review comments.
    pub reviews: bool,
    /// How its pull request stands against its base: moved, or conflicting.
    pub pull: bool,
    /// Its earlier attempts, and why they failed.
    pub attempts: bool,
    /// Its plan's status.
    pub plan: bool,
    /// The index of the notes in its scope.
    pub notes: bool,
    /// The template it follows.
    pub template: bool,
}

/// What a run may finish with: its outcome spec, in the plan's terms. Any run
/// may escalate instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Finish {
    /// A report; or, if it `grows`, steps added to its plan.
    Report { grows: bool },
    /// A change, pushed once the repository's checks pass if `checks`.
    Change { checks: bool },
    /// A verdict on the head it reviews.
    Verdict,
    /// A session's turn: a reply, or tasks to make, and a plan proposed, or,
    /// if `supervising`, steps added to its goal's plan.
    Turn { supervising: bool },
}

/// What is due for the item whose record's plan part is `record`, from
/// `facts`. The writes an action or finishing makes go into `out`, which has
/// room for [`max_out`](crate::max_out) of them.
pub fn due(config: &Config, env: &Env<Limits>, record: &Record, facts: &Facts, out: &mut Queue<Write>) -> Due {
    if !facts.dependencies.all_done() {
        return Due::Nothing(Waits::Dependencies);
    }
    match &record.step.work {
        Work::Agent(spec) => agent(config, record, spec, facts, out),
        Work::Change(spec) => change(config, env, record, spec, facts, out),
        Work::Wait(spec) => wait(env, &record.step, *spec, facts, out),
        Work::Session(spec) => session(config, record, spec, facts, out),
    }
}

fn agent(config: &Config, record: &Record, spec: &AgentSpec, facts: &Facts, out: &mut Queue<Write>) -> Due {
    if record.progress.finished {
        if !facts.children.all_done() {
            return Due::Nothing(Waits::Children);
        }
        return done(out);
    }
    if let Some(due) = acceptance(&record.step, facts) {
        return due;
    }
    let sections = Sections { plan: spec.grows, ..sections(facts) };
    Due::Run(run(config, Why::Work, &spec.charter, Finish::Report { grows: spec.grows }, sections, false))
}

fn wait(env: &Env<Limits>, step: &Step, spec: WaitSpec, facts: &Facts, out: &mut Queue<Write>) -> Due {
    if let Some(due) = acceptance(step, facts) {
        return due;
    }
    match spec {
        WaitSpec::Steps => done(out),
        WaitSpec::Decision => match facts.decision {
            None => Due::Nothing(Waits::Decision),
            Some(Decision::Accepted) => done(out),
            Some(Decision::Rejected) => Due::Hold(Hold::Rejected),
        },
        WaitSpec::Time(span) => {
            let from = match facts.dependencies.last_done {
                Some(last) if facts.dependencies.total > 0 => last,
                Some(_) | None => facts.created,
            };
            let at = from.saturating_add(span);
            if env.now >= at {
                return done(out);
            }
            Due::Nothing(Waits::Time(at))
        }
    }
}

fn session(config: &Config, record: &Record, spec: &SessionSpec, facts: &Facts, out: &mut Queue<Write>) -> Due {
    let supervising = record.goal.is_some();
    if supervising && facts.children.total > 0 && facts.children.all_done() {
        return done(out);
    }
    if let Some(due) = acceptance(&record.step, facts) {
        return due;
    }
    if !facts.woken {
        return Due::Nothing(Waits::Wake);
    }
    let resume = facts.snapshot
        && match spec.resume {
            Resume::Default => !supervising,
            Resume::Always => true,
            Resume::Never => false,
        };
    let sections = Sections { plan: supervising, ..sections(facts) };
    Due::Run(run(config, Why::Turn, &spec.charter, Finish::Turn { supervising }, sections, resume))
}

fn change(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    spec: &ChangeSpec,
    facts: &Facts,
    out: &mut Queue<Write>,
) -> Due {
    let Some(pull) = facts.pull else {
        if facts.branch.is_none() {
            let finish = Finish::Change { checks: spec.checks };
            return Due::Run(run(config, Why::Produce, &spec.produce, finish, sections(facts), false));
        }
        out.push(Write::OpenPull { base: copy_of(&spec.base) });
        return Due::Act(Action::OpenPull);
    };
    match pull.state {
        PullState::Open => open(config, env, record, spec, facts, pull, out),
        PullState::Merged => {
            out.push(Write::Close);
            out.push(Write::DeleteBranch);
            Due::Done
        }
        PullState::Closed => Due::Hold(Hold::PullClosed),
    }
}

/// What is due for a change whose pull request is open.
fn open(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    spec: &ChangeSpec,
    facts: &Facts,
    pull: Pull,
    out: &mut Queue<Write>,
) -> Due {
    if pull.merge == Mergeable::Conflicts {
        return repair(config, env, record, spec, facts, Repair::Conflicts);
    }
    if pull.base_moved {
        return repair(config, env, record, spec, facts, Repair::BaseMoved);
    }
    match pull.ci {
        Ci::Failed => return repair(config, env, record, spec, facts, Repair::CiFailed),
        Ci::None | Ci::Pending => return Due::Nothing(Waits::Ci),
        Ci::Passed => {}
    }
    if pull.changes_requested {
        return repair(config, env, record, spec, facts, Repair::ChangesRequested);
    }
    match &spec.review {
        Review::Person => {
            if pull.approvals == 0 {
                return Due::Nothing(Waits::Review);
            }
        }
        Review::Agent(charter) => match record.progress.review {
            Some(Reviewed { head, verdict }) if head == pull.head => match verdict {
                Verdict::Approve => {}
                Verdict::Changes => return repair(config, env, record, spec, facts, Repair::ChangesRequested),
            },
            Some(_) | None => {
                let why = Why::Review { head: pull.head };
                let sections = Sections { reviews: true, ..sections(facts) };
                return Due::Run(run(config, why, charter, Finish::Verdict, sections, false));
            }
        },
    }
    for gate in &record.step.gates {
        match gate {
            Gate::Approvals(people) => {
                if pull.approvals < *people {
                    return Due::Nothing(Waits::Approvals);
                }
            }
            Gate::Accepted => {}
        }
    }
    if let Some(due) = acceptance(&record.step, facts) {
        return due;
    }
    match pull.merge {
        Mergeable::Unknown => Due::Nothing(Waits::Mergeable),
        Mergeable::Clean => {
            out.push(Write::Merge { head: pull.head });
            Due::Act(Action::Merge)
        }
        Mergeable::Conflicts => unreachable!("a change that conflicts is repaired first"),
    }
}

/// A run that repairs the change, its brief saying why, or a hold once it has
/// been repaired as many times as the limits allow.
fn repair(config: &Config, env: &Env<Limits>, record: &Record, spec: &ChangeSpec, facts: &Facts, why: Repair) -> Due {
    if record.progress.repairs >= env.limits.repairs {
        return Due::Hold(Hold::Repairs);
    }
    let base = sections(facts);
    let sections = match why {
        Repair::CiFailed => Sections { ci: true, ..base },
        Repair::ChangesRequested => Sections { reviews: true, ..base },
        Repair::BaseMoved | Repair::Conflicts => Sections { pull: true, ..base },
    };
    let finish = Finish::Change { checks: spec.checks };
    Due::Run(run(config, Why::Repair(why), &spec.produce, finish, sections, false))
}

/// What the step's gate of a person's acceptance asks for, if it has one and
/// it does not hold: a wait for the decision, or a hold if it was rejected.
fn acceptance(step: &Step, facts: &Facts) -> Option<Due> {
    let mut gated = false;
    for gate in &step.gates {
        match gate {
            Gate::Accepted => gated = true,
            Gate::Approvals(_) => {}
        }
    }
    if !gated {
        return None;
    }
    match facts.decision {
        None => Some(Due::Nothing(Waits::Acceptance)),
        Some(Decision::Accepted) => None,
        Some(Decision::Rejected) => Some(Due::Hold(Hold::Rejected)),
    }
}

/// The step is done: its item is closed.
fn done(out: &mut Queue<Write>) -> Due {
    out.push(Write::Close);
    Due::Done
}

/// The sections every brief carries: the item, the comments since the last
/// turn, earlier attempts, the notes' index, and the dependencies' outcomes
/// when there are any.
fn sections(facts: &Facts) -> Sections {
    Sections {
        item: true,
        comments: true,
        dependencies: facts.dependencies.total > 0,
        ci: false,
        reviews: false,
        pull: false,
        attempts: true,
        plan: false,
        notes: true,
        template: false,
    }
}

fn run(config: &Config, why: Why, charter: &Charter, finish: Finish, sections: Sections, resume: bool) -> Run {
    // A template the configuration no longer has gates nothing: the brief
    // goes without it.
    let template = match &charter.template {
        Some(name) => config.template(name),
        None => None,
    };
    Run {
        why,
        sections: Sections { template: template.is_some(), ..sections },
        finish,
        grants: charter.grants,
        budget: charter.budget,
        instructions: copy_of(&charter.instructions),
        template,
        resume,
    }
}
