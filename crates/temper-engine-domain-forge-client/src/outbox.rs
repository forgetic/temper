//! In-flight write lanes and uncertain-effect recovery (domain/forge.md,
//! section 6; domain/connectors.md, section 4.3).
//!
//! The client holds execution copies, not the top's durable outbox entries.
//! It never chooses effects or sends their news. `make` accepts a committed
//! entry; each lane moves Due → Busy → Due, Waiting, or settled. Recovery
//! searches by key or resulting state before retrying after the saved
//! lifetime. `Progress` gives the top a new durable position, and `Outcome`
//! reports settlement; the parent commits them before releasing calls.
use crate::api::{self, Answer, Commit, Error, Op, Read, Repository, Write};
use crate::calls::{self, Owner};
use crate::domain::{Alarm, Domain};
use crate::{
    Attempt, Condition, Entry, Fact, Limits, Made, Outcome, Position, Priority, Recovery, RecoveryClock, Request,
    bounds, recovery,
};
use skein_lib::{Duration, Env, Map, Queue, Time, Wall};

#[derive(Debug)]
pub(crate) struct Outbox {
    entries: Map<u64, Writing>,
    ready: bool,
    paused: bool,
    settling: bool,
    clock: RecoveryClock,
}
#[derive(Debug)]
struct Writing {
    entry: Entry,
    lane: Lane,
    clock: Time,
    state: State,
    uncertain: bool,
    restart_checked: bool,
    withdrawn: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
struct Lane {
    repository: Repository,
    number: Option<u64>,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum State {
    Due(Phase),
    Busy(Phase),
    Waiting(Phase),
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Phase {
    Clock,
    Check,
    Make,
    Find { page: u32, after: u64 },
    Compare { head: Commit, base: Commit },
    AfterUpdate,
    AfterMerge { merged: Commit },
}
impl Outbox {
    pub(crate) fn new(l: &Limits) -> Outbox {
        Outbox {
            entries: Map::with_capacity(l.entries),
            ready: false,
            paused: false,
            settling: false,
            clock: RecoveryClock::Monotonic,
        }
    }
    pub(crate) fn is_ready(&self, keep: &crate::keep::Keep) -> bool {
        if !self.ready || self.paused {
            return false;
        }
        for (&number, writing) in &self.entries {
            match writing.state {
                State::Due(_) => {
                    if eligible(self, keep, number, writing) {
                        return true;
                    }
                }
                State::Busy(_) | State::Waiting(_) => {}
            }
        }
        false
    }
}
pub(crate) fn worst_case(l: &Limits) -> Option<u64> {
    Map::<u64, Writing>::worst_case(l.entries)?.checked_add(u64::from(l.entries).checked_mul(u64::from(l.op_bytes))?)
}
fn valid(entry: &Entry, l: &Limits) -> bool {
    if entry.attempt.is_some() && entry.start.is_none() {
        return false;
    }
    let Some(held) = bounds::write(&entry.effect.write, l) else {
        return false;
    };
    let condition = match &entry.effect.condition {
        Condition::None | Condition::Update { .. } | Condition::Review { .. } => 0,
        Condition::Merge { base } => base.len(),
    };
    let Some(held) = held.checked_add(u64::try_from(condition).expect("usize fits u64")) else {
        return false;
    };
    if held > u64::from(l.op_bytes) {
        return false;
    }
    match &entry.effect.write {
        Write::Merge { .. } => match entry.effect.condition {
            Condition::Merge { .. } => true,
            Condition::None | Condition::Update { .. } | Condition::Review { .. } => false,
        },
        Write::Update { .. } => match entry.effect.condition {
            Condition::Update { .. } => true,
            Condition::None | Condition::Merge { .. } | Condition::Review { .. } => false,
        },
        Write::Review { key, .. } => match entry.effect.condition {
            Condition::Review { .. } => !key.is_empty(),
            Condition::None | Condition::Merge { .. } | Condition::Update { .. } => false,
        },
        Write::CreateIssue { key, .. } | Write::Post { key, .. } => {
            entry.effect.condition == Condition::None && !key.is_empty()
        }
        Write::OpenPull { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. } => entry.effect.condition == Condition::None,
    }
}
fn lane(entry: &Entry) -> Lane {
    let number = match entry.effect.write {
        Write::Post { number, .. }
        | Write::Review { number, .. }
        | Write::Edit { number, .. }
        | Write::SetReviewers { number, .. }
        | Write::Close { number }
        | Write::Reopen { number }
        | Write::Merge { number, .. }
        | Write::Update { number } => Some(number),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. } => None,
    };
    Lane { repository: entry.repository, number }
}
fn find(entry: &Entry) -> Phase {
    let position = entry.start.expect("an attempted write has a saved start");
    Phase::Find { page: position.review_page.max(1), after: position.comment }
}
pub(crate) fn make(d: &mut Domain, env: &Env<Limits>, entry: Entry, out: &mut Queue<Request>) {
    let error = if !d.outbox.ready {
        Some(Error::Busy)
    } else if !crate::identity::connected(d, entry.repository) {
        Some(Error::Forbidden)
    } else if !valid(&entry, &env.limits) {
        Some(Error::TooLarge)
    } else if d.outbox.entries.len() >= env.limits.entries {
        Some(Error::Busy)
    } else {
        None
    };
    if let Some(error) = error {
        d.fact(Fact::Refused);
        out.push(Request::Outcome { entry: entry.number, task: entry.task, outcome: Outcome::Failed(error) });
        return;
    }
    assert!(!d.outbox.entries.contains_key(&entry.number), "an outbox entry is handed off once");
    if let Some((number, _)) = d.outbox.entries.last() {
        assert!(*number < entry.number, "new entries arrive in commit order");
    }
    let number = entry.number;
    let lane = lane(&entry);
    let uncertain = entry.attempt.is_some();
    let phase = if uncertain { find(&entry) } else { Phase::Clock };
    d.outbox
        .entries
        .insert(
            number,
            Writing {
                entry,
                lane,
                clock: Time::ZERO,
                state: State::Due(phase),
                uncertain,
                restart_checked: !uncertain,
                withdrawn: false,
            },
        )
        .expect("outbox admission checked");
}
pub(crate) fn restored(d: &mut Domain, clock: RecoveryClock) {
    assert!(!d.outbox.ready, "restored once");
    d.outbox.ready = true;
    d.outbox.clock = clock;
}
pub(crate) fn pause(d: &mut Domain) {
    assert!(d.outbox.ready, "pause after restore");
    d.outbox.paused = true;
}
pub(crate) fn start_settle(d: &mut Domain) {
    assert!(d.outbox.ready && d.outbox.paused, "settle follows restored fresh reads");
    d.outbox.paused = false;
    d.outbox.settling = true;
}
pub(crate) fn settle_done(d: &mut Domain) -> bool {
    if !d.outbox.settling {
        return false;
    }
    for (_, entry) in &d.outbox.entries {
        if !entry.restart_checked {
            return false;
        }
    }
    d.outbox.settling = false;
    true
}
pub(crate) fn withdraw(d: &mut Domain, number: u64, out: &mut Queue<Request>) {
    let Some(writing) = d.outbox.entries.get(&number) else {
        return;
    };
    if writing.entry.attempt.is_some() {
        return;
    }
    match writing.state {
        State::Due(_) | State::Waiting(_) => {}
        State::Busy(_) => {
            if !calls::cancel_entry(&mut d.calls, number) {
                d.outbox.entries.get_mut(&number).expect("withdrawn entry remains").withdrawn = true;
                return;
            }
        }
    }
    finish(d, number, Outcome::Withdrawn, out);
}
pub(crate) fn pump(d: &mut Domain, env: &Env<Limits>, out: &mut Queue<Request>) -> bool {
    if !d.outbox.ready || d.outbox.paused || d.calls.is_full() {
        return false;
    }
    let mut selected = None;
    for (&number, writing) in &d.outbox.entries {
        let phase = match writing.state {
            State::Due(phase) => phase,
            State::Busy(_) | State::Waiting(_) => continue,
        };
        if eligible(&d.outbox, &d.keep, number, writing) {
            selected = Some((number, phase));
            break;
        }
    }
    let Some((number, phase)) = selected else {
        return false;
    };
    match phase {
        Phase::Find { .. } => {
            let w = d.outbox.entries.get(&number).expect("selected entry remains");
            if is_set(&w.entry.effect.write) {
                not_found(d, env, number, out);
                return true;
            }
        }
        Phase::Clock
        | Phase::Check
        | Phase::Make
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {}
    }
    let writing = d.outbox.entries.get_mut(&number).expect("selected entry remains");
    let op = operation(writing, phase);
    let repository = writing.entry.repository;
    let priority = match phase {
        Phase::Make => Priority::Write,
        Phase::Clock
        | Phase::Check
        | Phase::Find { .. }
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => Priority::Fresh,
    };
    writing.state = State::Busy(phase);
    calls::queue(&mut d.calls, Owner::Entry(number), repository, op, priority);
    false
}
fn operation(w: &Writing, phase: Phase) -> Op {
    let read = match phase {
        // An empty inclusive listing supplies Date without scanning history.
        Phase::Clock => Read::Items { since: Time::from_nanos(u64::MAX), page: 1, kind: None },
        Phase::Make => return Op::Write(w.entry.effect.write.clone()),
        Phase::Compare { head, base } => Read::Compare { before: head, after: base },
        Phase::Check | Phase::AfterUpdate | Phase::AfterMerge { .. } => {
            Read::Pull { number: w.lane.number.expect("conditional effect names an item") }
        }
        Phase::Find { page, after } => match &w.entry.effect.write {
            Write::CreateIssue { .. } => Read::Items {
                since: w.entry.start.expect("find has saved start").at,
                page,
                kind: Some(api::Kind::Issue),
            },
            Write::OpenPull { head, base, .. } => Read::PullFor { head: head.clone(), base: base.clone() },
            Write::Post { number, .. } => Read::Item { number: *number, after },
            Write::Review { number, .. } => Read::Reviews { number: *number, page },
            Write::Merge { number, .. } | Write::Update { number } => Read::Pull { number: *number },
            Write::CreateBranch { branch, .. } | Write::DeleteBranch { branch } => {
                Read::Branch { branch: branch.clone() }
            }
            Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. } => unreachable!("set recovery waits before entering make"),
        },
    };
    Op::Read(read)
}
/// Called at submission, before `Request::Call` is emitted. Progress joins
/// the decision, so the parent's commit gate makes both start fields
/// durable before the write can reach the forge.
pub(crate) fn sent(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let entry = &d.outbox.entries.get(&number).expect("call belongs to a live entry").entry;
    let position = crate::keep::position(&d.keep, entry);
    let w = d.outbox.entries.get_mut(&number).expect("call belongs to a live entry");
    let phase = match w.state {
        State::Busy(phase) => phase,
        State::Due(_) | State::Waiting(_) => unreachable!("queued entry is busy"),
    };
    match phase {
        Phase::Make => {
            if w.entry.start.is_none() {
                w.entry.start = Some(Position { at: w.clock, ..position });
            }
            let span = env.limits.lifetime.saturating_add(env.limits.clock_margin);
            let expires = Wall::from_nanos(env.wall.as_nanos().saturating_add(span.as_nanos()));
            w.entry.attempt =
                Some(Attempt { sent: env.now, deadline: env.now.saturating_add(span), wall: env.wall, expires });
            out.push(Request::Progress { entry: w.entry.clone() });
        }
        Phase::Clock
        | Phase::Check
        | Phase::Find { .. }
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {}
    }
}
pub(crate) fn due(d: &mut Domain, number: u64) {
    let w = d.outbox.entries.get_mut(&number).expect("entry alarm belongs to retained entry");
    match w.state {
        State::Waiting(phase) => w.state = State::Due(phase),
        State::Due(_) | State::Busy(_) => unreachable!("entry alarm fires while waiting"),
    }
}
fn check_phase(entry: &Entry) -> Phase {
    match entry.effect.condition {
        Condition::None => Phase::Make,
        Condition::Merge { .. } | Condition::Update { .. } | Condition::Review { .. } => Phase::Check,
    }
}
fn finished(d: &mut Domain, number: u64, made: Made, found: bool, out: &mut Queue<Request>) {
    finish(d, number, Outcome::Made { made, found }, out);
}
fn finish(d: &mut Domain, number: u64, outcome: Outcome, out: &mut Queue<Request>) {
    let w = d.outbox.entries.remove(&number).expect("settled entry lives");
    match outcome {
        Outcome::Made { made, .. } | Outcome::Raced { made, .. } => crate::keep::made(d, &w.entry, made, out),
        Outcome::Failed(_) | Outcome::Uncertain | Outcome::Held | Outcome::Withdrawn => {}
    }
    d.alarms.cancel(Alarm::Entry(number));
    out.push(Request::Outcome { entry: number, task: w.entry.task, outcome });
}
fn next(d: &mut Domain, number: u64, phase: Phase) {
    d.outbox.entries.get_mut(&number).expect("entry remains").state = State::Due(phase);
}
fn wait(d: &mut Domain, number: u64, phase: Phase, until: Time) {
    d.outbox.entries.get_mut(&number).expect("entry remains").state = State::Waiting(phase);
    d.alarms.arm(Alarm::Entry(number), until).expect("one alarm per entry fits");
}
fn remaining(entry: &Entry, clock: RecoveryClock, env: &Env<Limits>) -> Duration {
    let attempt = entry.attempt.expect("uncertain entry has an attempt");
    match clock {
        RecoveryClock::Monotonic => attempt.deadline.saturating_since(env.now),
        RecoveryClock::Wall => Duration::from_nanos(attempt.expires.as_nanos().saturating_sub(env.wall.as_nanos())),
    }
}
fn not_found(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    d.outbox.entries.get_mut(&number).expect("find owns entry").restart_checked = true;
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    let left = remaining(&w.entry, d.outbox.clock, env);
    if left == Duration::ZERO {
        if recovery(&w.entry.effect.write) == Recovery::Unrecoverable {
            finish(d, number, Outcome::Held, out);
        } else if w.entry.failures >= env.limits.write_attempts {
            finish(d, number, Outcome::Failed(Error::Timeout), out);
        } else {
            let phase = check_phase(&w.entry);
            d.outbox.entries.get_mut(&number).expect("entry remains").uncertain = false;
            next(d, number, phase);
        }
    } else {
        let phase = find(&w.entry);
        wait(d, number, phase, env.now.saturating_add(left));
    }
}
fn is_set(write: &Write) -> bool {
    match write {
        Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. } => true,
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. } => false,
    }
}
fn key(write: &Write) -> Option<&[u8]> {
    match write {
        Write::CreateIssue { key, .. } | Write::Post { key, .. } | Write::Review { key, .. } => Some(key),
        Write::OpenPull { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. } => None,
    }
}
fn same_key(value: Option<&[u8]>, expected: &[u8]) -> bool {
    match value {
        Some(value) => value == expected,
        None => false,
    }
}

pub(crate) fn answered(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    result: Result<Answer, Error>,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("call owner remains");
    if w.withdrawn {
        finish(d, number, Outcome::Withdrawn, out);
        return;
    }
    let phase = match w.state {
        State::Busy(phase) => phase,
        State::Due(_) | State::Waiting(_) => unreachable!("entry terminal while busy"),
    };
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            failed(d, env, number, phase, error, out);
            return;
        }
    };
    match answer {
        Answer::Items { items, more, now } => listed(d, env, number, phase, &items, Listing { more, now }, out),
        Answer::Item { comments, more, .. } => comments_found(d, env, number, phase, &comments, more, out),
        Answer::Reviews { reviews, more } => reviews_found(d, env, number, phase, &reviews, more, out),
        Answer::Pull(pull) => pulled(d, env, number, phase, pull, out),
        Answer::Compare { commits, .. } => compared(d, env, number, phase, commits.is_empty(), out),
        Answer::Commit(commit) => branch_found(d, env, number, commit, out),
        Answer::Created(item) => finished(d, number, Made::Created(item), false, out),
        Answer::Commented(comment) => finished(d, number, Made::Commented(comment), false, out),
        Answer::Reviewed(_) => next(d, number, find(&w.entry)),
        Answer::Merged(commit) => {
            d.outbox.entries.get_mut(&number).expect("merge entry remains").uncertain = true;
            next(d, number, Phase::AfterMerge { merged: commit });
        }
        Answer::Branch(created) => branch_created(d, number, created, out),
        Answer::Done => done(d, number, out),
        Answer::Statuses { .. }
        | Answer::Branches(_)
        | Answer::Remarks { .. }
        | Answer::PullFiles { .. }
        | Answer::Checks(_)
        | Answer::File { .. }
        | Answer::Job { .. }
        | Answer::Protection(_)
        | Answer::Settings(_)
        | Answer::Collaborators { .. }
        | Answer::Permission(_) => unreachable!("outbox operation has another answer shape"),
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Listing {
    more: bool,
    now: Time,
}
fn listed(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    items: &[api::Summary],
    info: Listing,
    out: &mut Queue<Request>,
) {
    let Listing { more, now } = info;
    match phase {
        Phase::Clock => {
            d.outbox.entries.get_mut(&number).expect("entry remains").clock = now;
            let phase = check_phase(&d.outbox.entries.get(&number).expect("entry remains").entry);
            next(d, number, phase);
        }
        Phase::Find { page, after } => {
            let expected = key(&d.outbox.entries.get(&number).expect("entry remains").entry.effect.write)
                .expect("issue creation has key");
            let mut found = None;
            for item in items {
                if crate::identity::writer(
                    d,
                    d.outbox.entries.get(&number).expect("entry remains").entry.repository,
                    item.author,
                ) && same_key(item.key.as_deref(), expected)
                {
                    found = Some(item.number);
                    break;
                }
            }
            match found {
                Some(item) => finished(d, number, Made::Created(item), true, out),
                None if more => next(d, number, Phase::Find { page: page.saturating_add(1), after }),
                None => not_found(d, env, number, out),
            }
        }
        Phase::Check | Phase::Make | Phase::Compare { .. } | Phase::AfterUpdate | Phase::AfterMerge { .. } => {
            unreachable!("items answer only clock or issue find")
        }
    }
}
fn comments_found(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    comments: &[api::Comment],
    more: bool,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    let expected = key(&w.entry.effect.write).expect("comment creation has key");
    let mut found = None;
    let mut last = match phase {
        Phase::Find { after, .. } => after,
        Phase::Clock
        | Phase::Check
        | Phase::Make
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {
            unreachable!("comments answer find")
        }
    };
    for comment in comments {
        last = last.max(comment.id);
        if original(comment.provenance)
            && crate::identity::writer(d, w.entry.repository, comment.author)
            && same_key(comment.key.as_deref(), expected)
        {
            found = Some(comment.id);
            break;
        }
    }
    match found {
        Some(comment) => finished(d, number, Made::Commented(comment), true, out),
        None if more => next(d, number, Phase::Find { page: 1, after: last }),
        None => not_found(d, env, number, out),
    }
}
fn reviews_found(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    reviews: &[api::Review],
    more: bool,
    out: &mut Queue<Request>,
) {
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    let start = w.entry.start.expect("a recovery search has its durable start");
    let expected = key(&w.entry.effect.write).expect("review creation has key");
    let mut found = None;
    for review in reviews {
        if review.id > start.review
            && original(review.provenance)
            && crate::identity::writer(d, w.entry.repository, review.author)
            && same_key(review.key.as_deref(), expected)
        {
            let head = match w.entry.effect.condition {
                Condition::Review { head } => head,
                Condition::None | Condition::Merge { .. } | Condition::Update { .. } => {
                    unreachable!("review condition admitted")
                }
            };
            if review.commit != head {
                finish(d, number, Outcome::Raced { made: Made::Reviewed(review.id), why: Error::Stale }, out);
                return;
            }
            found = Some(review.id);
            break;
        }
    }
    let page = match phase {
        Phase::Find { page, .. } => page,
        Phase::Clock
        | Phase::Check
        | Phase::Make
        | Phase::Compare { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {
            unreachable!("reviews answer find")
        }
    };
    match found {
        Some(review) => finished(d, number, Made::Reviewed(review), true, out),
        None if more => next(d, number, Phase::Find { page: page.saturating_add(1), after: 0 }),
        None => not_found(d, env, number, out),
    }
}
fn original(provenance: api::Provenance) -> bool {
    match provenance {
        api::Provenance::Original => true,
        api::Provenance::Revised | api::Provenance::Unknown => false,
    }
}
fn compared(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, empty: bool, out: &mut Queue<Request>) {
    let head = match phase {
        Phase::Compare { head, .. } => head,
        Phase::Clock
        | Phase::Check
        | Phase::Make
        | Phase::Find { .. }
        | Phase::AfterUpdate
        | Phase::AfterMerge { .. } => {
            unreachable!("comparison answers update recovery")
        }
    };
    if empty {
        finished(d, number, Made::Updated(head), true, out);
    } else {
        not_found(d, env, number, out);
    }
}
fn branch_found(d: &mut Domain, env: &Env<Limits>, number: u64, commit: Commit, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("find owns entry");
    match w.entry.effect.write {
        Write::CreateBranch { commit: expected, .. } => {
            if commit == expected {
                finished(d, number, Made::Branch(commit), true, out);
            } else {
                finish(d, number, Outcome::Failed(Error::Exists), out);
            }
        }
        Write::DeleteBranch { .. } => not_found(d, env, number, out),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Merge { .. }
        | Write::Update { .. }
        | Write::Status { .. } => unreachable!("branch read for branch effect"),
    }
}
fn branch_created(d: &mut Domain, number: u64, created: api::BranchCreation, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("creation owns entry");
    match created {
        api::BranchCreation::Created => {
            let commit = match w.entry.effect.write {
                Write::CreateBranch { commit, .. } => commit,
                Write::CreateIssue { .. }
                | Write::OpenPull { .. }
                | Write::Post { .. }
                | Write::Review { .. }
                | Write::Edit { .. }
                | Write::SetReviewers { .. }
                | Write::Close { .. }
                | Write::Reopen { .. }
                | Write::Merge { .. }
                | Write::Update { .. }
                | Write::Status { .. }
                | Write::DeleteBranch { .. } => unreachable!("branch creation terminal"),
            };
            finished(d, number, Made::Branch(commit), false, out);
        }
        api::BranchCreation::Exists => next(d, number, find(&w.entry)),
    }
}
fn done(d: &mut Domain, number: u64, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("terminal owns entry");
    match w.entry.effect.write {
        Write::Update { .. } => {
            d.outbox.entries.get_mut(&number).expect("update entry remains").uncertain = true;
            next(d, number, Phase::AfterUpdate);
        }
        Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Status { .. }
        | Write::DeleteBranch { .. } => finished(d, number, Made::Set, false, out),
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Merge { .. }
        | Write::CreateBranch { .. } => unreachable!("creation has its typed terminal"),
    }
}

fn pulled(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, pull: api::Pull, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("pull belongs to retained entry");
    match phase {
        Phase::Check => {
            if pull.state == api::State::Closed {
                finish(d, number, Outcome::Failed(Error::Closed), out);
                return;
            }
            let valid = match &w.entry.effect.condition {
                Condition::Merge { base } => {
                    let head = match w.entry.effect.write {
                        Write::Merge { head, .. } => head,
                        Write::CreateIssue { .. }
                        | Write::OpenPull { .. }
                        | Write::Post { .. }
                        | Write::Review { .. }
                        | Write::Edit { .. }
                        | Write::SetReviewers { .. }
                        | Write::Close { .. }
                        | Write::Reopen { .. }
                        | Write::Update { .. }
                        | Write::Status { .. }
                        | Write::CreateBranch { .. }
                        | Write::DeleteBranch { .. } => unreachable!("merge condition checked at admission"),
                    };
                    pull.base == *base && pull.commit == head
                }
                Condition::Update { head, base } => pull.commit == *head && pull.base_commit == Some(*base),
                Condition::Review { head } => pull.commit == *head,
                Condition::None => unreachable!("unconditional effects do not check pulls"),
            };
            if valid {
                next(d, number, Phase::Make);
            } else {
                finish(d, number, Outcome::Failed(Error::Stale), out);
            }
        }
        Phase::Find { .. } => match &w.entry.effect.write {
            Write::OpenPull { head, base, .. } => {
                if pull.head == *head && pull.base == *base {
                    if pull.state == api::State::Closed {
                        finish(d, number, Outcome::Raced { made: Made::Created(pull.number), why: Error::Closed }, out);
                    } else {
                        finished(d, number, Made::Created(pull.number), true, out);
                    }
                } else {
                    not_found(d, env, number, out);
                }
            }
            Write::Merge { head, .. } => {
                if let Some(merged) = pull.merged {
                    if pull.commit == *head {
                        merge_verified(d, number, pull, merged, true, out);
                    } else {
                        finish(d, number, Outcome::Failed(Error::Stale), out);
                    }
                } else {
                    not_found(d, env, number, out);
                }
            }
            Write::Update { .. } => {
                let base = match w.entry.effect.condition {
                    Condition::Update { base, .. } => base,
                    Condition::None | Condition::Merge { .. } | Condition::Review { .. } => {
                        unreachable!("update condition admitted")
                    }
                };
                next(d, number, Phase::Compare { head: pull.commit, base });
            }
            Write::CreateIssue { .. }
            | Write::Post { .. }
            | Write::Review { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Status { .. }
            | Write::CreateBranch { .. }
            | Write::DeleteBranch { .. } => unreachable!("pull find belongs to pull creation or transition"),
        },
        Phase::AfterMerge { merged } => merge_verified(d, number, pull, merged, false, out),
        Phase::AfterUpdate => {
            let base = match w.entry.effect.condition {
                Condition::Update { base, .. } => base,
                Condition::None | Condition::Merge { .. } | Condition::Review { .. } => {
                    unreachable!("update completion has update condition")
                }
            };
            next(d, number, Phase::Compare { head: pull.commit, base });
        }
        Phase::Clock | Phase::Make | Phase::Compare { .. } => {
            unreachable!("pull answers conditional checks and recovery")
        }
    }
}
fn merge_verified(d: &mut Domain, number: u64, pull: api::Pull, merged: Commit, found: bool, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get(&number).expect("merge entry remains");
    let base = match &w.entry.effect.condition {
        Condition::Merge { base } => base,
        Condition::None | Condition::Update { .. } | Condition::Review { .. } => {
            unreachable!("merge condition admitted")
        }
    };
    let head = match w.entry.effect.write {
        Write::Merge { head, .. } => head,
        Write::CreateIssue { .. }
        | Write::OpenPull { .. }
        | Write::Post { .. }
        | Write::Review { .. }
        | Write::Edit { .. }
        | Write::SetReviewers { .. }
        | Write::Close { .. }
        | Write::Reopen { .. }
        | Write::Update { .. }
        | Write::Status { .. }
        | Write::CreateBranch { .. }
        | Write::DeleteBranch { .. } => unreachable!("merge effect admitted"),
    };
    let made = Made::Merged(merged);
    if pull.base == *base && pull.commit == head && pull.merged == Some(merged) {
        finished(d, number, made, found, out);
    } else {
        finish(d, number, Outcome::Raced { made, why: Error::Stale }, out);
    }
}
fn failed(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, error: Error, out: &mut Queue<Request>) {
    if retain_uncertain(d, env, number, phase, error, out) {
        return;
    }
    match error {
        Error::RateLimited { .. } => unreachable!("request budget handles rate resets without completing owner"),
        Error::Timeout | Error::InvalidAnswer => match phase {
            Phase::Make => {
                let w = d.outbox.entries.get_mut(&number).expect("failed write remains");
                w.uncertain = true;
                w.entry.failures = w.entry.failures.saturating_add(1);
                let phase = find(&w.entry);
                let task = w.entry.task;
                out.push(Request::Progress { entry: w.entry.clone() });
                out.push(Request::Outcome { entry: number, task, outcome: Outcome::Uncertain });
                next(d, number, phase);
            }
            Phase::Clock
            | Phase::Check
            | Phase::Find { .. }
            | Phase::Compare { .. }
            | Phase::AfterUpdate
            | Phase::AfterMerge { .. } => {
                backoff(d, env, number, phase, out);
            }
        },
        Error::Unavailable => backoff(d, env, number, phase, out),
        Error::TooLarge => match phase {
            Phase::Clock | Phase::Check | Phase::Make => finish(d, number, Outcome::Failed(error), out),
            Phase::Find { .. } | Phase::Compare { .. } | Phase::AfterUpdate | Phase::AfterMerge { .. } => {
                backoff(d, env, number, phase, out);
            }
        },
        Error::Missing => {
            let w = d.outbox.entries.get(&number).expect("missing answer belongs to live entry");
            match phase {
                Phase::Find { .. } => match w.entry.effect.write {
                    Write::DeleteBranch { .. } => finished(d, number, Made::Set, true, out),
                    Write::CreateBranch { .. }
                    | Write::OpenPull { .. }
                    | Write::CreateIssue { .. }
                    | Write::Post { .. }
                    | Write::Review { .. }
                    | Write::Merge { .. }
                    | Write::Update { .. } => not_found(d, env, number, out),
                    Write::Edit { .. }
                    | Write::SetReviewers { .. }
                    | Write::Close { .. }
                    | Write::Reopen { .. }
                    | Write::Status { .. } => finish(d, number, Outcome::Failed(error), out),
                },
                Phase::Make => match w.entry.effect.write {
                    Write::DeleteBranch { .. } => finished(d, number, Made::Set, false, out),
                    Write::CreateIssue { .. }
                    | Write::OpenPull { .. }
                    | Write::Post { .. }
                    | Write::Review { .. }
                    | Write::Edit { .. }
                    | Write::SetReviewers { .. }
                    | Write::Close { .. }
                    | Write::Reopen { .. }
                    | Write::Merge { .. }
                    | Write::Update { .. }
                    | Write::Status { .. }
                    | Write::CreateBranch { .. } => finish(d, number, Outcome::Failed(error), out),
                },
                Phase::Clock | Phase::Check | Phase::Compare { .. } | Phase::AfterUpdate | Phase::AfterMerge { .. } => {
                    finish(d, number, Outcome::Failed(error), out);
                }
            }
        }
        Error::Exists => {
            let w = d.outbox.entries.get(&number).expect("exists belongs to live entry");
            if w.entry.attempt.is_some() {
                let phase = find(&w.entry);
                next(d, number, phase);
            } else {
                finish(d, number, Outcome::Failed(error), out);
            }
        }
        Error::Forbidden
        | Error::MissingJob
        | Error::Empty
        | Error::Full
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Refused
        | Error::Busy => finish(d, number, Outcome::Failed(error), out),
    }
}
fn backoff(d: &mut Domain, env: &Env<Limits>, number: u64, phase: Phase, out: &mut Queue<Request>) {
    let w = d.outbox.entries.get_mut(&number).expect("retry belongs to retained entry");
    w.entry.failures = w.entry.failures.saturating_add(1);
    let uncertain = w.uncertain;
    if w.entry.failures >= env.limits.write_attempts && !uncertain {
        finish(d, number, Outcome::Failed(Error::Unavailable), out);
        return;
    }
    let mut delay = env.limits.backoff;
    for _ in 1..w.entry.failures {
        delay = delay.saturating_mul(2).min(env.limits.backoff_max);
        if delay == env.limits.backoff_max {
            break;
        }
    }
    let jitter = d.rng.below(delay.as_nanos());
    let delay = delay.saturating_add(Duration::from_nanos(jitter)).min(env.limits.backoff_max);
    out.push(Request::Progress { entry: w.entry.clone() });
    wait(d, number, phase, env.now.saturating_add(delay));
}

fn eligible(o: &Outbox, keep: &crate::keep::Keep, number: u64, writing: &Writing) -> bool {
    for (&other, previous) in &o.entries {
        if other >= number {
            break;
        }
        if previous.lane == writing.lane {
            return false;
        }
    }
    match writing.state {
        State::Due(Phase::Make | Phase::Find { .. }) => crate::keep::echo_room(keep, &writing.entry),
        State::Due(_) => true,
        State::Busy(_) | State::Waiting(_) => false,
    }
}
pub(crate) fn echo(d: &Domain, repository: Repository, key: &[u8]) -> bool {
    for (_, writing) in &d.outbox.entries {
        if writing.entry.repository != repository {
            continue;
        }
        match &writing.entry.effect.write {
            Write::CreateIssue { key: own, .. } | Write::Post { key: own, .. } | Write::Review { key: own, .. } => {
                if **own == *key {
                    return true;
                }
            }
            Write::OpenPull { .. }
            | Write::Edit { .. }
            | Write::SetReviewers { .. }
            | Write::Close { .. }
            | Write::Reopen { .. }
            | Write::Merge { .. }
            | Write::Update { .. }
            | Write::Status { .. }
            | Write::CreateBranch { .. }
            | Write::DeleteBranch { .. } => {}
        }
    }
    false
}

fn retain_uncertain(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    phase: Phase,
    error: Error,
    out: &mut Queue<Request>,
) -> bool {
    let transient = match error {
        Error::Timeout | Error::InvalidAnswer | Error::Unavailable | Error::TooLarge | Error::RateLimited { .. } => {
            true
        }
        Error::Forbidden
        | Error::Missing
        | Error::MissingJob
        | Error::Empty
        | Error::Full
        | Error::Exists
        | Error::NothingToMerge
        | Error::Closed
        | Error::Stale
        | Error::Conflict
        | Error::Protected
        | Error::Refused
        | Error::Busy => false,
    };
    if !transient {
        // A missing creation is the answer to its recovery lookup,
        // rather than a failed lookup to back off. Let `failed` apply the
        // saved deadline and either wait, retry safely, or hold for a person.
        if error == Error::Missing
            && let Phase::Find { .. } = phase
        {
            let w = d.outbox.entries.get(&number).expect("find owns entry");
            match w.entry.effect.write {
                Write::CreateBranch { .. }
                | Write::OpenPull { .. }
                | Write::CreateIssue { .. }
                | Write::Post { .. }
                | Write::Review { .. }
                | Write::Merge { .. }
                | Write::Update { .. } => return false,
                Write::Edit { .. }
                | Write::SetReviewers { .. }
                | Write::Close { .. }
                | Write::Reopen { .. }
                | Write::Status { .. }
                | Write::DeleteBranch { .. } => {}
            }
        }
        match phase {
            Phase::AfterMerge { merged } => {
                finish(d, number, Outcome::Raced { made: Made::Merged(merged), why: error }, out);
                return true;
            }
            Phase::Clock
            | Phase::Check
            | Phase::Make
            | Phase::Find { .. }
            | Phase::Compare { .. }
            | Phase::AfterUpdate => {}
        }
        if d.outbox.entries.get(&number).expect("failed entry remains").uncertain {
            backoff(d, env, number, phase, out);
            return true;
        }
    }
    false
}
