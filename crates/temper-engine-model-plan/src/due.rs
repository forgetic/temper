//! What is due for an item (engine-model.md, 5.3), from its step and the facts
//! about it: nothing yet, and what it waits on; a run, and the parts of its
//! charter the plan decides; an engine action (4.5); done; or hold for a
//! person. An item a person closed is done. Every other step waits until all
//! its dependencies are made and done. Then, by primitive:
//!
//! ```text
//! agent    gate: accepted? ─► run ─► (it reports) ─► its children done ─► done
//! wait     steps: done | decision: accepted ─► done, rejected ─► hold
//!          time: done once it has passed, after the last dependency done
//! session  (it finished, or supervises a goal whose steps are all done)
//!            ─► its children done ─► done
//!          gate: accepted? ─► its first turn, a retry, or woken ─► run
//! change   no branch ─► run: produce
//!          branch, no pull request ─► open it
//!          pull request merged ─► done | closed unmerged ─► hold
//!          open:  conflicts, base moved ─► run: rebase
//!                 CI failed ─► run: repair
//!                 CI pending or not reported ─► wait
//!                 changes asked for on the head ─► run: repair
//!                 review: a person's approval, or an agent's run and verdict
//!                 gates: approvals, a person's acceptance
//!                 mergeable not yet known ─► wait
//!                 clean ─► merge at exactly the head
//! ```
//!
//! A change repaired for failures (CI, changes asked for) as many times as
//! the limits allow is held for a person when it needs another such repair;
//! one rebased onto a moved base as many times as they allow (a higher
//! limit: a busy base moves often), when it needs another rebase. A change
//! that waits on the forge (CI, a review, mergeability) longer than the
//! limits' stall, counted from its head's push or its last release, is held
//! too; its parent tells its goal's session first, if it has one. A person's
//! decision made before the step's last release counts for nothing.
//!
//! A run that is due is claimed: the progress write that goes with it, made
//! with the claim, records why it runs and when, so applying its outcome, and
//! a session's wake timer, need nothing kept in memory. Which sections a
//! run's brief carries follows from why it runs: a repair's say why it
//! repairs (9).

use alloc::boxed::Box;

use temper_lib::bytes::copy_of;
use temper_lib::{Env, Queue, Time};

use crate::check::count;
use crate::config::Config;
use crate::facts::{Ci, Decision, Facts, Mergeable, Pull, PullState};
use crate::limits::Limits;
use crate::plan::{
    AgentSpec, Budget, ChangeSpec, Charter, Commit, Gate, Grants, Resume, Review, SessionSpec, WaitSpec, Work,
};
use crate::record::{Progress, Record, Reviewed, Verdict};
use crate::write::Write;

/// What is due for an item now.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Due {
    /// Nothing yet: the item waits on `waits`. If `until` is given, it is
    /// asked again then, even if nothing it reads changes: a wait for a time,
    /// or the bound of a wait on the forge.
    Nothing { waits: Waits, until: Option<Time> },
    /// A run, with these parts of its charter. The progress write in the
    /// caller's queue goes with its claim.
    Run(Run),
    /// An engine action: the writes it makes are in the caller's queue.
    Act(Action),
    /// The step is done: the writes in the caller's queue finish it, closing
    /// its item, unless a person closed it already.
    Done,
    /// Hold the item for a person, and why.
    Hold(Hold),
}

/// What an item waits on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Waits {
    /// The steps it comes after: to be made, and done.
    Dependencies,
    /// The steps it added.
    Children,
    /// A person's acceptance, which a gate asks for.
    Acceptance,
    /// A person's decision, which a wait waits for.
    Decision,
    /// A time to pass.
    Time,
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

/// Why an item is held for a person. A person's release lifts the cause
/// ([`release`](crate::release)).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Hold {
    /// A person rejected it: on a wait for their decision, or on a gate of
    /// their acceptance; or rejected its proposals as many times as the
    /// limits allow.
    Rejected,
    /// Its change needs another repair for a failure, past the limit.
    Repairs,
    /// Its change needs another rebase, past the limit.
    Rebases,
    /// Its pull request was closed without being merged.
    PullClosed,
    /// Its run escalated: it asks for a decision it cannot make, of its
    /// goal's session first (section 6), or of a person.
    Escalated,
    /// Its change waited on the forge past the limits' stall: its parent
    /// tells its goal's session first, if it has one.
    Stalled,
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
    /// Not one of section 9's: a rebase's brief says why with it.
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
    /// A session's turn: a reply, tasks to make, or that it is finished; and
    /// a plan proposed, or, if `supervising`, steps added to its goal's plan
    /// or one of them released.
    Turn { supervising: bool },
}

/// What is due for the item whose record's plan part is `record`, from
/// `facts`. The writes a claim, an action or finishing makes go into `out`,
/// which has room for [`max_out`](crate::max_out) of them.
pub fn due(config: &Config, env: &Env<Limits>, record: &Record, facts: &Facts, out: &mut Queue<Write>) -> Due {
    if facts.closed {
        return Due::Done;
    }
    let step = &record.step;
    if facts.dependencies.total < count(step.after.len()) || !facts.dependencies.all_done() {
        return nothing(Waits::Dependencies);
    }
    match &step.work {
        Work::Agent(spec) => agent(config, env, record, spec, facts, out),
        Work::Change(spec) => change(config, env, record, spec, facts, out),
        Work::Wait(spec) => wait(env, record, *spec, facts, out),
        Work::Session(spec) => session(config, env, record, spec, facts, out),
    }
}

fn agent(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    spec: &AgentSpec,
    facts: &Facts,
    out: &mut Queue<Write>,
) -> Due {
    if record.progress.finished {
        if !facts.children.all_done() {
            return nothing(Waits::Children);
        }
        return done(out);
    }
    if let Some(due) = acceptance(record, facts) {
        return due;
    }
    let sections = Sections { plan: spec.grows, ..sections(facts) };
    let run = run(config, Why::Work, &spec.charter, Finish::Report { grows: spec.grows }, sections, false);
    claim(env, record, run, out)
}

fn wait(env: &Env<Limits>, record: &Record, spec: WaitSpec, facts: &Facts, out: &mut Queue<Write>) -> Due {
    if let Some(due) = acceptance(record, facts) {
        return due;
    }
    match spec {
        WaitSpec::Steps => done(out),
        WaitSpec::Decision => match decision(record, facts) {
            None => nothing(Waits::Decision),
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
            Due::Nothing { waits: Waits::Time, until: Some(at) }
        }
    }
}

fn session(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    spec: &SessionSpec,
    facts: &Facts,
    out: &mut Queue<Write>,
) -> Due {
    let supervising = record.goal.is_some();
    let goal_done = supervising && facts.children.total > 0 && facts.children.all_done();
    if record.progress.finished || goal_done {
        if !facts.children.all_done() {
            return nothing(Waits::Children);
        }
        return done(out);
    }
    if let Some(due) = acceptance(record, facts) {
        return due;
    }
    // Its first turn needs no wake, and nor does a turn claimed whose
    // outcome was never applied (its run failed, or the engine restarted):
    // the retry takes the same events. A person's release since its last
    // turn wakes it too: what held it (its runs failing) is lifted, and the
    // turn it never had is due.
    let retry = record.progress.running.is_some();
    let released = match record.progress.released {
        Some(released) => match record.progress.last_run {
            Some(last) => released >= last,
            None => false,
        },
        None => false,
    };
    if record.progress.last_run.is_some() && !retry && !released && !facts.woken {
        return nothing(Waits::Wake);
    }
    let resume = facts.snapshot
        && match spec.resume {
            Resume::Default => !supervising,
            Resume::Always => true,
            Resume::Never => false,
        };
    let sections = Sections { plan: supervising, ..sections(facts) };
    let run = run(config, Why::Turn, &spec.charter, Finish::Turn { supervising }, sections, resume);
    claim(env, record, run, out)
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
            let run = run(config, Why::Produce, &spec.produce, finish, sections(facts), false);
            return claim(env, record, run, out);
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
    let clean = match pull.merge {
        Mergeable::Conflicts => return repair(config, env, record, spec, facts, Repair::Conflicts, out),
        Mergeable::Unknown => false,
        Mergeable::Clean => true,
    };
    if pull.base_moved {
        return repair(config, env, record, spec, facts, Repair::BaseMoved, out);
    }
    // Waits on the forge count from the head's push, or the last release.
    let from = match record.progress.released {
        Some(released) => released.max(pull.pushed),
        None => pull.pushed,
    };
    let stall = from.saturating_add(env.limits.stall);
    match pull.ci {
        Ci::Failed => return repair(config, env, record, spec, facts, Repair::CiFailed, out),
        Ci::None | Ci::Pending => return forge(env, Waits::Ci, stall),
        Ci::Passed => {}
    }
    if pull.changes_requested {
        return repair(config, env, record, spec, facts, Repair::ChangesRequested, out);
    }
    match &spec.review {
        Review::Person => {
            if pull.approvals == 0 {
                return forge(env, Waits::Review, stall);
            }
        }
        Review::Agent(charter) => match record.progress.review {
            Some(Reviewed { head, verdict }) if head == pull.head => match verdict {
                Verdict::Approve => {}
                Verdict::Changes => {
                    return repair(config, env, record, spec, facts, Repair::ChangesRequested, out);
                }
            },
            Some(_) | None => {
                let why = Why::Review { head: pull.head };
                let sections = Sections { reviews: true, ..sections(facts) };
                return claim(env, record, run(config, why, charter, Finish::Verdict, sections, false), out);
            }
        },
    }
    for gate in &record.step.gates {
        match gate {
            Gate::Approvals(people) => {
                if pull.approvals < *people {
                    return forge(env, Waits::Approvals, stall);
                }
            }
            Gate::Accepted => {}
        }
    }
    if let Some(due) = acceptance(record, facts) {
        return due;
    }
    if !clean {
        return forge(env, Waits::Mergeable, stall);
    }
    out.push(Write::Merge { head: pull.head });
    Due::Act(Action::Merge)
}

/// A wait on the forge, until `stall`; past it, a hold.
fn forge(env: &Env<Limits>, waits: Waits, stall: Time) -> Due {
    if env.now >= stall {
        return Due::Hold(Hold::Stalled);
    }
    Due::Nothing { waits, until: Some(stall) }
}

/// A run that repairs or rebases the change, its brief saying why, or a hold
/// once it has been repaired, or rebased, as many times as the limits allow.
fn repair(
    config: &Config,
    env: &Env<Limits>,
    record: &Record,
    spec: &ChangeSpec,
    facts: &Facts,
    why: Repair,
    out: &mut Queue<Write>,
) -> Due {
    let progress = record.progress;
    let limits = &env.limits;
    let (made, most, hold) = match why {
        Repair::CiFailed | Repair::ChangesRequested => (progress.repairs, limits.repairs, Hold::Repairs),
        Repair::BaseMoved | Repair::Conflicts => (progress.rebases, limits.rebases, Hold::Rebases),
    };
    if made >= most {
        return Due::Hold(hold);
    }
    let base = sections(facts);
    let sections = match why {
        Repair::CiFailed => Sections { ci: true, ..base },
        Repair::ChangesRequested => Sections { reviews: true, ..base },
        Repair::BaseMoved | Repair::Conflicts => Sections { pull: true, ..base },
    };
    let finish = Finish::Change { checks: spec.checks };
    claim(env, record, run(config, Why::Repair(why), &spec.produce, finish, sections, false), out)
}

/// What the step's gate of a person's acceptance asks for, if it has one and
/// it does not hold: a wait for the decision, or a hold if it was rejected.
fn acceptance(record: &Record, facts: &Facts) -> Option<Due> {
    let mut gated = false;
    for gate in &record.step.gates {
        match gate {
            Gate::Accepted => gated = true,
            Gate::Approvals(_) => {}
        }
    }
    if !gated {
        return None;
    }
    match decision(record, facts) {
        None => Some(nothing(Waits::Acceptance)),
        Some(Decision::Accepted) => None,
        Some(Decision::Rejected) => Some(Due::Hold(Hold::Rejected)),
    }
}

/// A person's decision on the step, unless they made it before its last
/// release.
fn decision(record: &Record, facts: &Facts) -> Option<Decision> {
    let decided = facts.decision?;
    match record.progress.released {
        Some(released) if decided.at <= released => None,
        Some(_) | None => Some(decided.decision),
    }
}

fn nothing(waits: Waits) -> Due {
    Due::Nothing { waits, until: None }
}

/// The step is done: its item is closed.
fn done(out: &mut Queue<Write>) -> Due {
    out.push(Write::Close);
    Due::Done
}

/// A run claimed: its progress write records why it runs, and when.
fn claim(env: &Env<Limits>, record: &Record, run: Run, out: &mut Queue<Write>) -> Due {
    let progress = Progress {
        running: Some(run.why),
        last_run: Some(env.now),
        runs: record.progress.runs.saturating_add(1),
        ..record.progress
    };
    out.push(Write::Progress(progress));
    Due::Run(run)
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
