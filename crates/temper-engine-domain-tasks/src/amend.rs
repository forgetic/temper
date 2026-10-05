//! Changes checked whole, then committed with immutable history and a merged
//! executor control message. Authority comparisons belong to the root.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{
    Authority, Balance, Envelope, Key, Limits, Message, Numbers, Party, Phase, Refusal, Request, Spec, Stored,
    WakePolicy, Was,
};
use alloc::boxed::Box;
use skein_lib::{Env, List, Queue, ReplyTo};
/// Authenticated evidence supplied only by the root after authority checks.
/// Tasks verifies tree standing itself; role/escalation facts are external.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Authorization {
    Tree(Party),
    Person { person: u64, project: u32 },
    Escalation { holder: Party, task: u64, project: u32 },
}
impl Authorization {
    #[must_use]
    pub const fn party(self) -> Party {
        match self {
            Authorization::Tree(party) => party,
            Authorization::Person { person, .. } => Party::Person(person),
            Authorization::Escalation { holder, .. } => holder,
        }
    }
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Control {
    Cancel { reason: Box<[u8]> },
    Release { reason: Box<[u8]> },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct AuthorityChange {
    pub task: u64,
    pub before: Authority,
    /// The root checked narrowing/widening and all delegation ceilings.
    pub after: Authority,
    pub budget: u64,
    /// Root compared current run grants with the narrowed authority.
    pub stop_run: bool,
    /// Root-issued message candidate for this affected descendant.
    pub message: u64,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Amendment {
    pub message: u64,
    pub spec: Option<Spec>,
    pub policy: Option<WakePolicy>,
    pub dependencies: Option<Box<[u64]>>,
    pub tracked: Option<Option<u32>>,
    /// Includes every descendant the root's authority check narrows.
    pub authorities: Box<[AuthorityChange]>,
    pub balances: Box<[Balance]>,
    pub reason: Box<[u8]>,
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Change {
    Amended,
    Cancelled,
    Released,
    Moved { from: Party, to: Party },
}
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct History {
    pub task: u64,
    pub revision: u64,
    pub by: Party,
    pub reason: Box<[u8]>,
    pub change: Change,
}
pub(crate) fn below(d: &Domain, number: u64, ancestor: u64, bound: u32) -> bool {
    let mut at = Some(number);
    for _ in 0..bound {
        let Some(number) = at else {
            return false;
        };
        if number == ancestor {
            return true;
        }
        at = match record(d, number) {
            Some(task) => match task.requester {
                Party::Task(parent) => Some(parent),
                Party::Person(_) | Party::Deployment { .. } => None,
            },
            None => None,
        };
    }
    false
}
pub(crate) fn standing(d: &Domain, number: u64, authorization: Authorization, bound: u32) -> bool {
    let Some(task) = record(d, number) else {
        return false;
    };
    match authorization {
        Authorization::Tree(by) => {
            if task.requester == by {
                return true;
            }
            match by {
                Party::Task(ancestor) => ancestor != number && below(d, number, ancestor, bound),
                Party::Person(_) | Party::Deployment { .. } => false,
            }
        }
        Authorization::Person { project, .. } => project == task.project,
        Authorization::Escalation { task: target, project, .. } => target == number && project == task.project,
    }
}
pub(crate) fn mutable(phase: &Phase) -> bool {
    match phase {
        Phase::Waiting | Phase::Active(_) | Phase::Held { was: Was::Waiting | Was::Active(_), .. } => true,
        Phase::Closing(_) | Phase::Held { was: Was::Closing(_), .. } | Phase::Ended(_) => false,
    }
}
pub(crate) fn history(d: &mut Domain, number: u64, by: Party, reason: &[u8], change: Change, out: &mut Queue<Request>) {
    let task = task_mut(d, number).expect("history task live");
    task.record.revision = task.record.revision.checked_add(1).expect("revision admitted");
    out.push(Request::Save {
        record: Stored::History(History {
            task: number,
            revision: task.record.revision,
            by,
            reason: reason.into(),
            change,
        }),
    });
}
pub(crate) fn control(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    authorization: Authorization,
    action: Control,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    if !standing(d, number, authorization, env.limits.tasks) {
        return refused(to, Some(number), Refusal::Standing, out);
    }
    let (reason, change) = match &action {
        Control::Cancel { reason } => (reason, Change::Cancelled),
        Control::Release { reason } => (reason, Change::Released),
    };
    let reason_cap = match action {
        Control::Cancel { .. } => env.limits.message_bytes.min(env.limits.result_bytes),
        Control::Release { .. } => env.limits.message_bytes,
    };
    if reason.len() > usize::try_from(reason_cap).expect("u32 fits usize") {
        return refused(to, Some(number), Refusal::Reason, out);
    }
    let old = record(d, number).expect("entrance live");
    if old.revision == u64::MAX {
        return refused(to, Some(number), Refusal::Revision, out);
    }
    match &action {
        Control::Release { .. } => {
            match old.phase {
                Phase::Held { .. } => {}
                Phase::Waiting | Phase::Active(_) | Phase::Closing(_) | Phase::Ended(_) => {
                    return refused(to, Some(number), Refusal::Unheld, out);
                }
            }
            if crate::run::run_attempt(&old.phase).is_some() {
                return refused(to, Some(number), Refusal::State, out);
            }
        }
        Control::Cancel { .. } => {
            for (child, _) in &d.names {
                if below(d, *child, number, env.limits.tasks)
                    && record(d, *child).expect("live name").revision == u64::MAX
                {
                    return refused(to, Some(number), Refusal::Revision, out);
                }
            }
        }
    }
    let mut selected = List::with_capacity(env.limits.tasks);
    for (child, _) in &d.names {
        let include = match action {
            Control::Cancel { .. } => below(d, *child, number, env.limits.tasks),
            Control::Release { .. } => *child == number,
        };
        if include {
            selected.push(*child).expect("live subtree bounded");
        }
    }
    for child in selected.into_boxed() {
        history(d, child, authorization.party(), reason, change.clone(), out);
        publish(d, env, child, out);
    }
    match action {
        Control::Cancel { reason } => crate::closing::cancel(d, env, to, number, reason, out),
        Control::Release { .. } => crate::run::release(d, env, to, number, out),
    }
}
pub(crate) fn message(
    d: &mut Domain,
    env: &Env<Limits>,
    task: u64,
    number: u64,
    by: Party,
    reason: &[u8],
    out: &mut Queue<Request>,
) {
    let mut old = None;
    for (key, envelope) in &d.messages {
        if key.task == task && crate::inbox::is_amendment(&envelope.message) {
            old = Some((*key, envelope.at, envelope.hits));
        }
    }
    let (at, hits) = match old {
        Some((key, at, hits)) => {
            let removed = d.messages.remove(&key);
            assert!(removed.is_some(), "selected control slot exists");
            out.push(Request::Erase { key: Key::Message(key) });
            (at, hits.saturating_add(1))
        }
        None => (env.wall, 1),
    };
    let revision = record(d, task).expect("message task live").revision;
    crate::inbox::insert(
        d,
        env,
        Envelope {
            task,
            number,
            from: by,
            at,
            hits,
            eligible: true,
            message: Message::Amendment { revision, reason: reason.into() },
        },
        out,
    );
}
pub(crate) fn amend(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    authorization: Authorization,
    amendment: Amendment,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    if !standing(d, number, authorization, env.limits.tasks) {
        return refused(to, Some(number), Refusal::Standing, out);
    }
    if let Err(why) = check(d, &env.limits, number, &amendment) {
        return refused(to, Some(number), why, out);
    }
    let task = task_mut(d, number).expect("amendment validated");
    if let Some(spec) = amendment.spec {
        task.record.spec = spec;
    }
    if let Some(policy) = amendment.policy {
        task.record.policy = policy;
    }
    if let Some(dependencies) = amendment.dependencies {
        task.record.dependencies = dependencies;
    }
    if let Some(tracked) = amendment.tracked {
        task.record.tracked = tracked;
    }
    crate::funders::apply_balances(d, env, &amendment.balances, out);
    for change in &amendment.authorities {
        let task = task_mut(d, change.task).expect("authority snapshot validated");
        task.record.authority = change.after.clone();
        task.record.numbers.budget = change.budget;
        if change.stop_run
            && let Some(attempt) = crate::run::run_attempt(&task.record.phase)
        {
            task.record.narrowing = true;
            out.push(Request::Stop { task: change.task, attempt });
        }
        if change.task != number {
            history(d, change.task, authorization.party(), &amendment.reason, Change::Amended, out);
            message(d, env, change.task, change.message, authorization.party(), &amendment.reason, out);
        }
        publish(d, env, change.task, out);
    }
    history(d, number, authorization.party(), &amendment.reason, Change::Amended, out);
    message(d, env, number, amendment.message, authorization.party(), &amendment.reason, out);
    out.push(Request::Done { reply_to: to });
}
fn check(d: &Domain, l: &Limits, number: u64, amendment: &Amendment) -> Result<(), Refusal> {
    let task = record(d, number).expect("checked target live");
    if !mutable(&task.phase) {
        return Err(Refusal::State);
    }
    if task.revision == u64::MAX {
        return Err(Refusal::Revision);
    }
    if amendment.reason.len() > usize::try_from(l.message_bytes).expect("u32 fits usize") {
        return Err(Refusal::Reason);
    }
    if crate::run::run_attempt(&task.phase).is_some() && !crate::inbox::offer_room(d, number, true, 1) {
        return Err(Refusal::Busy);
    }
    if amendment.message == 0 || amendment.message <= task.last_message {
        return Err(Refusal::Message);
    }
    if let Some(spec) = &amendment.spec {
        if !crate::batch::valid_spec(l, spec) {
            return Err(Refusal::Spec);
        }
        for input in &spec.inputs {
            if !d.stubs.contains_key(input) {
                return Err(Refusal::Inputs);
            }
        }
    }
    if let Some(policy) = &amendment.policy
        && !crate::wake::valid(policy)
    {
        return Err(Refusal::Message);
    }
    if let Some(dependencies) = &amendment.dependencies {
        match task.phase {
            Phase::Waiting | Phase::Held { was: Was::Waiting, .. } => {}
            Phase::Active(_)
            | Phase::Closing(_)
            | Phase::Held { was: Was::Active(_) | Was::Closing(_), .. }
            | Phase::Ended(_) => return Err(Refusal::State),
        }
        for (at, dependency) in dependencies.iter().enumerate() {
            if !crate::batch::contains(&task.dependencies, *dependency) {
                return Err(Refusal::Dependencies);
            }
            for earlier in dependencies.iter().take(at) {
                if earlier == dependency {
                    return Err(Refusal::Dependencies);
                }
            }
        }
    }
    if amendment.authorities.len() > usize::try_from(l.tasks).expect("u32 fits usize")
        || !crate::funders::validate_balances(d, task.project, &amendment.balances, l.tasks.saturating_mul(2))
    {
        return Err(Refusal::Funding);
    }
    for (at, change) in amendment.authorities.iter().enumerate() {
        for earlier in amendment.authorities.iter().take(at) {
            if earlier.task == change.task {
                return Err(Refusal::Duplicate);
            }
        }
        let Some(other) = record(d, change.task) else {
            return Err(Refusal::Unknown);
        };
        if !below(d, change.task, number, l.tasks) {
            return Err(Refusal::Standing);
        }
        if !mutable(&other.phase) || other.authority != change.before {
            return Err(Refusal::State);
        }
        if crate::run::run_attempt(&other.phase).is_some() && !crate::inbox::offer_room(d, change.task, true, 1) {
            return Err(Refusal::Busy);
        }
        if other.revision == u64::MAX {
            return Err(Refusal::Revision);
        }
        if change.task != number && (change.message == 0 || change.message <= other.last_message) {
            return Err(Refusal::Message);
        }
        if !crate::batch::valid_authority(l, &change.after) || change.after.budget.spend != change.budget {
            return Err(Refusal::AuthorityShape);
        }
        let mut after = Numbers { budget: change.budget, ..other.numbers };
        for balance in &amendment.balances {
            if balance.funder == crate::Funder::Task(change.task) {
                after.reserved = balance.after.reserved;
            }
        }
        if crate::funders::available(after).is_none() {
            return Err(Refusal::Funding);
        }
    }
    check_reservations(d, amendment)
}
fn check_reservations(d: &Domain, amendment: &Amendment) -> Result<(), Refusal> {
    // Every reservation difference is exact and remains at its one funder.
    for balance in &amendment.balances {
        let mut expected = balance.before;
        match balance.funder {
            crate::Funder::Task(number) => {
                for change in &amendment.authorities {
                    if change.task == number {
                        expected.budget = change.budget;
                    }
                }
            }
            crate::Funder::Pool { .. } | crate::Funder::Period { .. } => {}
        }
        for change in &amendment.authorities {
            let other = record(d, change.task).expect("change checked");
            if other.funder == balance.funder {
                expected.reserved = expected
                    .reserved
                    .checked_sub(other.numbers.budget)
                    .ok_or(Refusal::Funding)?
                    .checked_add(change.budget)
                    .ok_or(Refusal::Funding)?;
            }
        }
        if expected != balance.after {
            return Err(Refusal::Funding);
        }
    }
    for change in &amendment.authorities {
        let other = record(d, change.task).expect("change checked");
        if change.budget != other.numbers.budget {
            let mut covered = false;
            for balance in &amendment.balances {
                if balance.funder == other.funder {
                    covered = true;
                }
            }
            if !covered {
                return Err(Refusal::Funding);
            }
        }
    }
    Ok(())
}
