//! The translations between the child domains' vocabularies, and between theirs
//! and the boundary's (programming-model.md, 4.5): siblings share no types,
//! so the hub's items meet the forge's, the brief's and the fleet's names
//! here, the plan's decisions the hub's and the rules' terms, and the
//! fleet's and the views' tokens the boundary's items, through small total
//! functions, each an exhaustive match, so that a variant added on either
//! side breaks the build in one place.
//!
//! A run is named by its item (seams: "Names"): the fleet's and the views'
//! token for it packs the item, its repository in the high 16 bits and its
//! number in the low 48, so that a worker's hello after a restart names the
//! same run as the claim read. An item whose names do not fit is never run:
//! it is not taken in.

use alloc::boxed::Box;

use skein_lib::bytes::copy_of;
use skein_lib::{List, Token, Writer};
use temper_engine_domain_brief as brief;
use temper_engine_domain_fleet as fleet;
use temper_engine_domain_views as views;
use temper_legacy_engine_domain_forge::{self as forge, api};
use temper_legacy_engine_domain_plan as plan;
use temper_legacy_engine_domain_rules as rules;
use temper_legacy_engine_domain_work as work;

use crate::boundary::{Answer, Chunk, Failure, Hello, Item, Outcome, Phase, Posted, Trace};
use crate::items::Seen;

/// The bits of a run's token that hold the item's number.
const NUMBER_BITS: u32 = 48;

/// The token of the run of `item`, if its names fit one.
pub(crate) fn run(item: Item) -> Option<Token> {
    let repository = u64::from(item.repository);
    if repository >> (u64::BITS - NUMBER_BITS) != 0 || item.number >> NUMBER_BITS != 0 {
        return None;
    }
    Some(Token::new((repository << NUMBER_BITS) | item.number))
}

/// The item whose run `run` names.
pub(crate) fn item(run: Token) -> Item {
    let raw = run.raw();
    let repository = u32::try_from(raw >> NUMBER_BITS).expect("a run's token holds a repository in 16 bits");
    let mask = (1_u64 << NUMBER_BITS).saturating_sub(1);
    Item { repository, number: raw & mask }
}

/// The token of the run of `item`, which is taken in, so it fits.
pub(crate) fn run_of(item: Item) -> Token {
    run(item).expect("an item taken in names a run")
}

pub(crate) const fn forge_item(item: Item) -> forge::Item {
    forge::Item { repository: item.repository, number: item.number }
}

pub(crate) const fn item_of(item: forge::Item) -> Item {
    Item { repository: item.repository, number: item.number }
}

pub(crate) const fn brief_item(item: Item) -> brief::Item {
    brief::Item { repository: item.repository, number: item.number }
}

pub(crate) const fn from_brief(item: brief::Item) -> Item {
    Item { repository: item.repository, number: item.number }
}

/// The workstream of an item's runs (seams: "Workstream"), which names the
/// checkout they share: its repository and its number, big-endian.
pub(crate) fn workstream(item: Item) -> Box<[u8]> {
    concat(&[&item.repository.to_be_bytes(), &item.number.to_be_bytes()])
}

/// The bytes of a workstream key.
pub(crate) const WORKSTREAM_BYTES: u32 = 12;

/// The fleet's hello for a worker's: the runs it names by items that fit a
/// run's token, which are the only ones the engine assigns.
pub(crate) fn hello(hello: Hello) -> fleet::Hello {
    let Hello { slots, workstreams, hosting } = hello;
    let mut hosted = List::with_capacity(u32::try_from(hosting.len()).unwrap_or(0));
    for one in &hosting {
        let Some(run) = run(one.item) else { continue };
        let hosted_run = fleet::Hosted { run, attempt: Token::new(one.attempt), phase: one.phase };
        if hosted.push(hosted_run).is_err() {
            break;
        }
    }
    fleet::Hello { graces: None, slots, workstreams, hosting: hosted.into_boxed() }
}

/// What the fleet acts on of a run's answer.
pub(crate) const fn answer(answer: &Answer) -> fleet::Answer {
    match answer {
        Answer::Busy => fleet::Answer::Busy,
        Answer::Invalid => fleet::Answer::Invalid,
        Answer::Ended { .. } => fleet::Answer::Ended,
        Answer::Parked { .. } => fleet::Answer::Parked,
        Answer::Failed { .. } => fleet::Answer::Failed,
    }
}

/// The hub's failure class for a run's failure.
pub(crate) const fn class(failure: Failure) -> work::Class {
    match failure {
        Failure::Transient => work::Class::Transient,
        Failure::Permanent => work::Class::Permanent,
        Failure::Run => work::Class::Run,
        Failure::Agent => work::Class::Agent,
    }
}

/// The hub's code for a plan's reason to hold an item, which means the same
/// after a restart. Zero is the top level's: the item's record has no step.
pub(crate) const fn hold(hold: plan::Hold) -> u32 {
    match hold {
        plan::Hold::Rejected => 1,
        plan::Hold::Repairs => 2,
        plan::Hold::Rebases => 3,
        plan::Hold::PullClosed => 4,
        plan::Hold::Escalated => 5,
        plan::Hold::Stalled => 6,
    }
}

/// The hold of no step: the item's record does not say what it carries.
pub(crate) const NO_STEP: u32 = 0;

/// The holds of the run due, which the rules want a person to accept first,
/// or refuse: the top level's own, past the plan's.
pub(crate) const RUN_ACCEPTANCE: u32 = 7;
pub(crate) const RUN_REFUSED: u32 = 8;

/// What the hub does once an outcome's writes are made.
pub(crate) const fn then(then: plan::Then) -> work::Then {
    match then {
        plan::Then::Wait => work::Then::Wait,
        plan::Then::Hold(why) => work::Then::Hold { reason: hold(why) },
    }
}

/// An item's phase as people see it, from its record's.
pub(crate) const fn phase(phase: work::Phase) -> Phase {
    match phase {
        work::Phase::Waiting | work::Phase::Parked | work::Phase::Retrying(_) => Phase::Waiting,
        work::Phase::Claimed => Phase::Claimed,
        work::Phase::Applying { .. } => Phase::Applying,
        work::Phase::Held { .. } => Phase::Held,
        work::Phase::Done => Phase::Done,
    }
}

/// A watcher's stream, its runs and items named by items.
pub(crate) fn chunks(chunks: Box<[views::Chunk]>) -> Box<[Chunk]> {
    let mut stream = List::with_capacity(u32::try_from(chunks.len()).unwrap_or(0));
    for chunk in chunks {
        let chunk = match chunk {
            views::Chunk::Snapshot { at, content } => Chunk::Snapshot { at, content },
            views::Chunk::Report { run, attempt, kind, at, content } => {
                Chunk::Report { item: item(run), attempt: attempt.raw(), kind, at, content }
            }
            views::Chunk::Phase { item: of, phase, at } => {
                // The views carry back the codes the top level gave them.
                let Some(phase) = Phase::of(phase) else { continue };
                Chunk::Phase { item: item(of), phase, at }
            }
        };
        if stream.push(chunk).is_err() {
            break;
        }
    }
    stream.into_boxed()
}

/// Traces as the store keeps them, their runs named by items.
pub(crate) fn traces(records: Box<[views::Record]>) -> Box<[Trace]> {
    let mut traces = List::with_capacity(u32::try_from(records.len()).unwrap_or(0));
    for record in records {
        let views::Record { run, attempt, kind, at, size, content } = record;
        if traces.push(Trace { item: item(run), attempt: attempt.raw(), kind, at, size, content }).is_err() {
            break;
        }
    }
    traces.into_boxed()
}

/// The plan's terms for an outcome posted, for the item whose step is
/// `record`: a change's head is what its run pushed, a verdict's the head
/// its run was given to review.
pub(crate) fn outcome(posted: &Posted, record: &plan::Record) -> plan::Outcome {
    match &posted.outcome {
        Outcome::Change { message: _ } => plan::Outcome::Change { head: plan::Commit(posted.head.unwrap_or([0; 32])) },
        Outcome::Verdict { verdict, text: _ } => plan::Outcome::Verdict { head: reviewed(record), verdict: *verdict },
        Outcome::Report { text: _ } => plan::Outcome::Report,
        Outcome::Plan { plan: proposed, text: _ } => plan::Outcome::Plan(plan::Plan::clone(proposed)),
        Outcome::Steps { steps, text: _ } => plan::Outcome::Steps(steps.clone()),
        Outcome::Tasks { tasks, text: _ } => plan::Outcome::Tasks(tasks.clone()),
        Outcome::Reply { text: _ } => plan::Outcome::Reply,
        Outcome::Finished { text: _ } => plan::Outcome::Finished,
        Outcome::Release { step, text: _ } => plan::Outcome::Release { step: copy_of(step) },
        Outcome::Escalation { text: _ } => plan::Outcome::Escalation,
    }
}

/// The head a review run was given, as its claim recorded it.
fn reviewed(record: &plan::Record) -> plan::Commit {
    match record.progress.running {
        Some(why) => match why {
            plan::Why::Review { head } => head,
            plan::Why::Work | plan::Why::Produce | plan::Why::Repair(_) | plan::Why::Turn => plan::Commit([0; 32]),
        },
        None => plan::Commit([0; 32]),
    }
}

/// A change's pull request as the plan reads it, from the working set: its
/// head, when it was first seen there and where the base was then, and the
/// verdicts on it.
pub(crate) fn pull(level: forge::Level, reviews: Option<&[forge::Reviewed]>, seen: Option<Seen>) -> plan::Pull {
    let state = if level.open {
        plan::PullState::Open
    } else if level.merged.is_some() {
        plan::PullState::Merged
    } else {
        plan::PullState::Closed
    };
    let mut approvals: u32 = 0;
    let mut changes_requested = false;
    for review in reviews.unwrap_or(&[]) {
        match review.verdict {
            api::Verdict::Approve => approvals = approvals.saturating_add(1),
            api::Verdict::RequestChanges => changes_requested = true,
            api::Verdict::Comment => {}
        }
    }
    let (pushed, base_moved) = match seen {
        Some(seen) if seen.head == level.commit => (seen.at, seen.base != level.base),
        Some(_) | None => (skein_lib::Time::ZERO, false),
    };
    plan::Pull {
        head: plan::Commit(level.commit),
        pushed,
        state,
        ci: plan_ci(level.ci),
        approvals,
        changes_requested,
        merge: if level.mergeable { plan::Mergeable::Clean } else { plan::Mergeable::Conflicts },
        base_moved,
    }
}

pub(crate) const fn plan_ci(ci: forge::Ci) -> plan::Ci {
    match ci {
        forge::Ci::None => plan::Ci::None,
        forge::Ci::Pending => plan::Ci::Pending,
        forge::Ci::Passed => plan::Ci::Passed,
        forge::Ci::Failed => plan::Ci::Failed,
    }
}

pub(crate) const fn rules_ci(ci: plan::Ci) -> rules::Ci {
    match ci {
        plan::Ci::None => rules::Ci::None,
        plan::Ci::Pending => rules::Ci::Pending,
        plan::Ci::Passed => rules::Ci::Passed,
        plan::Ci::Failed => rules::Ci::Failed,
    }
}

pub(crate) const fn permission(permission: api::Permission) -> rules::Permission {
    match permission {
        api::Permission::None => rules::Permission::None,
        api::Permission::Read => rules::Permission::Read,
        api::Permission::Write => rules::Permission::Write,
        api::Permission::Admin => rules::Permission::Admin,
    }
}

pub(crate) const fn repository(repository: u32) -> rules::Repository {
    rules::Repository::Deployment(repository)
}

/// The rules' gates for a step's.
pub(crate) fn gates(gates: &[plan::Gate]) -> List<rules::Gate> {
    let mut found = List::with_capacity(u32::try_from(gates.len()).unwrap_or(0));
    for gate in gates {
        let gate = match gate {
            plan::Gate::Approvals(count) => rules::Gate::Approvals(*count),
            plan::Gate::Accepted => rules::Gate::Accepted,
        };
        if found.push(gate).is_err() {
            break;
        }
    }
    found
}

/// The rules' grants for a run of an item of `repository`: it reads it, and
/// pushes to `branch` if it may modify.
pub(crate) fn grants(grants: plan::Grants, repository: u32, branch: &[u8]) -> Box<[rules::Grant]> {
    let read = rules::Grant::Read { repository: self::repository(repository) };
    if grants.modify {
        let push = rules::Grant::Push { repository: self::repository(repository), branch: copy_of(branch) };
        return Box::new([read, push]);
    }
    Box::new([read])
}

/// A review's stance, for the rules; `None` for one that only comments.
pub(crate) const fn stance(verdict: api::Verdict) -> Option<rules::Stance> {
    match verdict {
        api::Verdict::Approve => Some(rules::Stance::Approve),
        api::Verdict::RequestChanges => Some(rules::Stance::RequestChanges),
        api::Verdict::Comment => None,
    }
}

/// The bytes of `number` in decimal.
pub(crate) fn decimal(number: u64) -> Box<[u8]> {
    let mut digits = [b'0'; 20];
    let mut rest = number;
    let mut start = digits.len();
    for slot in digits.iter_mut().rev() {
        let digit = u8::try_from(rest.checked_rem(10).unwrap_or(0)).unwrap_or(0);
        *slot = b'0'.saturating_add(digit);
        rest = rest.checked_div(10).unwrap_or(0);
        start = start.saturating_sub(1);
        if rest == 0 {
            break;
        }
    }
    copy_of(digits.get(start..).unwrap_or(&[]))
}

/// `parts`, one after another.
pub(crate) fn concat(parts: &[&[u8]]) -> Box<[u8]> {
    let mut len: usize = 0;
    for part in parts {
        len = len.saturating_add(part.len());
    }
    let mut writer = Writer::new(len);
    for part in parts {
        writer.put(part).expect("the writer has room for every part");
    }
    writer.finish()
}

/// The branch of an item's change, or of its saved work: `prefix`, then its
/// number.
pub(crate) fn branch(prefix: &[u8], item: Item) -> Box<[u8]> {
    concat(&[prefix, &decimal(item.number)])
}

/// The charter a step's runs work under, a change's reviews apart: an
/// agent's, a session's, or what produces and repairs a change. A wait has
/// none.
pub(crate) const fn charter_of(work: &plan::Work) -> Option<&plan::Charter> {
    match work {
        plan::Work::Agent(spec) => Some(&spec.charter),
        plan::Work::Session(spec) => Some(&spec.charter),
        plan::Work::Change(spec) => Some(&spec.produce),
        plan::Work::Wait(_) => None,
    }
}

/// The grants of the run claimed for the step `record` carries, as the
/// record says why it runs: what a run adopted after a restart was given.
pub(crate) fn grants_of(record: &plan::Record) -> Option<plan::Grants> {
    let review = match record.progress.running {
        Some(why) => match why {
            plan::Why::Review { .. } => true,
            plan::Why::Work | plan::Why::Produce | plan::Why::Repair(_) | plan::Why::Turn => false,
        },
        None => false,
    };
    match &record.step.work {
        plan::Work::Change(spec) if review => match &spec.review {
            plan::Review::Agent(charter) => Some(charter.grants),
            plan::Review::Person => None,
        },
        plan::Work::Agent(_) | plan::Work::Change(_) | plan::Work::Session(_) | plan::Work::Wait(_) => {
            Some(charter_of(&record.step.work)?.grants)
        }
    }
}

/// The pull request a forge's answer carries, if it is one.
pub(crate) fn pull_answered(answer: api::Answer) -> Option<api::Pull> {
    match answer {
        api::Answer::Pull(pull) => Some(pull),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Comment(_)
        | api::Answer::Reviews { .. }
        | api::Answer::Statuses { .. }
        | api::Answer::Remarks { .. }
        | api::Answer::Permission(_)
        | api::Answer::Commit(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented { .. }
        | api::Answer::Edited { .. }
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done => None,
    }
}

/// The permission a forge's answer carries, if it is one.
pub(crate) fn permission_answered(answer: api::Answer) -> Option<api::Permission> {
    match answer {
        api::Answer::Permission(permission) => Some(permission),
        api::Answer::Items { .. }
        | api::Answer::Item { .. }
        | api::Answer::Comment(_)
        | api::Answer::Pull(_)
        | api::Answer::Reviews { .. }
        | api::Answer::Statuses { .. }
        | api::Answer::Remarks { .. }
        | api::Answer::Commit(_)
        | api::Answer::Pages { .. }
        | api::Answer::Page(_)
        | api::Answer::Created(_)
        | api::Answer::Commented { .. }
        | api::Answer::Edited { .. }
        | api::Answer::Reviewed(_)
        | api::Answer::Merged(_)
        | api::Answer::Revision(_)
        | api::Answer::Done => None,
    }
}
