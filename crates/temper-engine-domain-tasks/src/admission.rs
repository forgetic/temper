//! Charged admissions preflight the complete turn or terminal before mutation.
//! Durable exact receipts survive task ending and never charge replay twice.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{Accepted, End, Hold, Key, Limits, MessageKey, Refusal, Request, Stored};
use skein_lib::{Env, Queue, ReplyTo};
/// Root-issued task/attempt identity, plus a turn number or terminal fence.
/// Tasks retains these within `Limits::admissions` (domain/tasks.md, 5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AdmissionKey {
    /// One charged turn in its attempt.
    Turn { task: u64, attempt: u64, turn: u32 },
    /// The attempt's unique charged terminal.
    Activation { task: u64, attempt: u64 },
}
/// Exact child-owned replay evidence. Root loads this durable row at restart;
/// its result is bounded by `Limits::result_bytes` before cloning (domain/tasks.md, 5).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Admission {
    /// Exact priced turn and immutable read fence accepted in one decision.
    Turn { task: u64, attempt: u64, turn: u32, read: Option<u64>, cumulative: u64 },
    /// Exact bounded original terminal, before lifecycle normalization.
    Activation { task: u64, attempt: u64, end: End, cumulative: u64 },
}
impl Admission {
    /// The durable replay identity (domain/tasks.md, 5).
    #[must_use]
    pub const fn key(&self) -> AdmissionKey {
        match self {
            Admission::Turn { task, attempt, turn, .. } => {
                AdmissionKey::Turn { task: *task, attempt: *attempt, turn: *turn }
            }
            Admission::Activation { task, attempt, .. } => AdmissionKey::Activation { task: *task, attempt: *attempt },
        }
    }
}
fn check_charge(d: &Domain, number: u64, cumulative: u64) -> Result<u64, Refusal> {
    let old = record(d, number).expect("admission recipient live");
    let delta = cumulative.checked_sub(old.run_spent).ok_or(Refusal::Turn)?;
    let spent = old.numbers.spent.checked_add(delta).ok_or(Refusal::Funding)?;
    let _total = spent.checked_add(old.numbers.spent_below).ok_or(Refusal::Funding)?;
    if !crate::funders::representable(d, number, delta) {
        return Err(Refusal::Funding);
    }
    if d.admissions.len() == d.admissions.capacity() {
        return Err(Refusal::Busy);
    }
    Ok(spent)
}
fn post(
    d: &mut Domain,
    env: &Env<Limits>,
    number: u64,
    cumulative: u64,
    spent: u64,
    receipt: Admission,
    out: &mut Queue<Request>,
) {
    let task = task_mut(d, number).expect("admitted recipient live");
    task.record.run_spent = cumulative;
    task.record.numbers.spent = spent;
    let overrun = crate::funders::available(task.record.numbers).is_none();
    publish(d, env, number, out);
    assert!(d.admissions.insert(receipt.key(), receipt.clone()) == Ok(None), "receipt room preflighted");
    out.push(Request::Save { record: Stored::Admission(receipt) });
    if overrun {
        crate::run::hold(d, env, number, Hold::Budget, out);
    }
}
#[expect(clippy::too_many_arguments, reason = "one complete admission event")]
pub(crate) fn turn(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    attempt: u64,
    turn: u32,
    read: Option<u64>,
    cumulative: u64,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, Some(number), Refusal::NotReady, out);
    }
    let receipt = Admission::Turn { task: number, attempt, turn, read, cumulative };
    if let Some(old) = d.admissions.get(&receipt.key()) {
        if *old != receipt {
            return refused(to, Some(number), Refusal::KeyConflict, out);
        }
        return out.push(Request::TurnAcknowledged {
            reply_to: to,
            task: number,
            attempt,
            turn,
            accepted: Accepted::Already,
        });
    }
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let old = record(d, number).expect("entrance checked");
    if attempt == 0 || old.attempt != attempt {
        return refused(to, Some(number), Refusal::Attempt, out);
    }
    if crate::run::run_attempt(&old.phase) != Some(attempt) || old.turn.checked_add(1) != Some(turn) {
        return refused(to, Some(number), Refusal::Turn, out);
    }
    if let Some(read) = read
        && old.last_read != Some(read)
        && match d.offers.get(&MessageKey { task: number, number: read }) {
            Some(offer) => offer.attempt != attempt,
            None => true,
        }
    {
        return refused(to, Some(number), Refusal::Read, out);
    }
    let spent = match check_charge(d, number, cumulative) {
        Ok(spent) => spent,
        Err(why) => return refused(to, Some(number), why, out),
    };
    crate::inbox::turn(d, env, to, number, attempt, turn, read, out);
    post(d, env, number, cumulative, spent, receipt, out);
}
#[expect(clippy::too_many_arguments, reason = "one complete admission event")]
pub(crate) fn activation(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    attempt: u64,
    end: End,
    cumulative: u64,
    out: &mut Queue<Request>,
) {
    if !d.ready() {
        return refused(to, Some(number), Refusal::NotReady, out);
    }
    let key = AdmissionKey::Activation { task: number, attempt };
    if let Some(old) = d.admissions.get(&key) {
        let exact = match old {
            Admission::Activation { end: previous, cumulative: whole, .. } => *previous == end && *whole == cumulative,
            Admission::Turn { .. } => false,
        };
        if !exact {
            return refused(to, Some(number), Refusal::KeyConflict, out);
        }
        return out.push(Request::Acknowledged { reply_to: to, task: number, attempt, accepted: Accepted::Already });
    }
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let old = record(d, number).expect("entrance checked");
    if attempt == 0 || crate::run::run_attempt(&old.phase) != Some(attempt) || old.last_answer == Some(attempt) {
        return refused(to, Some(number), Refusal::Attempt, out);
    }
    match &end {
        End::Finished { result, cancel_delegates } => {
            if crate::run::result_bytes(result) > usize::try_from(env.limits.result_bytes).expect("u32 fits usize") {
                return refused(to, Some(number), Refusal::Contract, out);
            }
            let closing = match old.phase {
                crate::Phase::Closing(_) | crate::Phase::Held { was: crate::Was::Closing(_), .. } => true,
                crate::Phase::Waiting
                | crate::Phase::Active(_)
                | crate::Phase::Held { was: crate::Was::Waiting | crate::Was::Active(_), .. }
                | crate::Phase::Ended(_) => false,
            };
            if !closing
                && !old.narrowing
                && crate::run::valid_result(&old.contract, result, &env.limits)
                && !cancel_delegates
                && !old.delegates.is_empty()
            {
                return refused(to, Some(number), Refusal::LiveDelegates, out);
            }
        }
        End::Parked | End::Failed(_) | End::Refused => {}
    }
    let spent = match check_charge(d, number, cumulative) {
        Ok(spent) => spent,
        Err(why) => return refused(to, Some(number), why, out),
    };
    let receipt = Admission::Activation { task: number, attempt, end: end.clone(), cumulative };
    crate::run::activation(d, env, to, number, attempt, end, out);
    post(d, env, number, cumulative, spent, receipt, out);
}
pub(crate) fn forget(d: &mut Domain, to: ReplyTo, key: AdmissionKey, out: &mut Queue<Request>) {
    if !d.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    if d.admissions.remove(&key).is_some() {
        out.push(Request::Erase { key: Key::Admission(key) });
    }
    out.push(Request::Done { reply_to: to });
}
pub(crate) fn restore(d: &mut Domain, env: &Env<Limits>, receipt: Admission) -> bool {
    if d.admissions.len() == d.admissions.capacity() || d.admissions.contains_key(&receipt.key()) {
        return false;
    }
    let valid = match &receipt {
        Admission::Turn { task, attempt, turn, .. } => *task != 0 && *attempt != 0 && *turn != 0,
        Admission::Activation { task, attempt, end, .. } => {
            *task != 0
                && *attempt != 0
                && match end {
                    End::Finished { result, .. } => {
                        crate::run::result_bytes(result)
                            <= usize::try_from(env.limits.result_bytes).expect("u32 fits usize")
                    }
                    End::Parked | End::Failed(_) | End::Refused => true,
                }
        }
    };
    if !valid {
        return false;
    }
    assert!(d.admissions.insert(receipt.key(), receipt) == Ok(None), "restore receipt admitted");
    true
}
