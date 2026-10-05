//! Concrete accounting carriers. Authority remains the root's pure policy;
//! tasks validates exact arithmetic and closes durable generations once.
use crate::domain::{Domain, entrance, publish, record, refused, task_mut};
use crate::{Funder, Hold, Limits, Numbers, Refusal, Request, Stored};
use skein_lib::{Env, Queue, ReplyTo};
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Balance {
    pub funder: Funder,
    /// Root's authoritative current snapshot; task funders are cross-checked.
    pub before: Numbers,
    pub after: Numbers,
}
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Closure {
    pub task: u64,
    pub generation: u64,
    pub funder: Funder,
    pub budget: u64,
    pub spent: u64,
}
pub(crate) fn total(numbers: Numbers) -> Option<u64> {
    numbers.spent.checked_add(numbers.spent_below)
}
pub(crate) fn remaining(numbers: Numbers) -> Option<u64> {
    numbers.budget.checked_sub(total(numbers)?)
}
pub(crate) fn available(numbers: Numbers) -> Option<u64> {
    remaining(numbers)?.checked_sub(numbers.reserved)
}
pub(crate) fn charge(
    d: &mut Domain,
    env: &Env<Limits>,
    to: ReplyTo,
    number: u64,
    attempt: u64,
    cumulative: u64,
    out: &mut Queue<Request>,
) {
    let to = match entrance(d, to, number) {
        Ok(to) => to,
        Err((to, why)) => return refused(to, Some(number), why, out),
    };
    let old = record(d, number).expect("entrance names task");
    if attempt == 0
        || attempt != old.attempt
        || (crate::run::run_attempt(&old.phase) != Some(attempt) && old.last_answer != Some(attempt))
    {
        return refused(to, Some(number), Refusal::Attempt, out);
    }
    // A replay must supply the same cumulative whole; lower values cannot
    // erase a charged turn, and a late answer cannot add after settlement.
    if cumulative < old.run_spent || (old.last_answer == Some(attempt) && cumulative != old.run_spent) {
        return refused(to, Some(number), Refusal::Turn, out);
    }
    let delta = cumulative.checked_sub(old.run_spent).expect("ordered cumulative values");
    let Some(spent) = old.numbers.spent.checked_add(delta) else {
        return refused(to, Some(number), Refusal::Funding, out);
    };
    let task = task_mut(d, number).expect("entrance names task");
    if spent.checked_add(task.record.numbers.spent_below).is_none() {
        return refused(to, Some(number), Refusal::Funding, out);
    }
    task.record.run_spent = cumulative;
    task.record.numbers.spent = spent;
    let overrun = available(task.record.numbers).is_none();
    publish(d, env, number, out);
    if overrun {
        crate::run::hold(d, env, number, Hold::Budget, out);
    }
    out.push(Request::Done { reply_to: to });
}
/// Validate all caller-supplied snapshots before any mutation. External pool
/// and period authenticity is the root's contract; no unlimited synthetic pool.
pub(crate) fn validate_balances(d: &Domain, project: u32, balances: &[Balance], bound: u32) -> bool {
    if balances.len() > usize::try_from(bound).expect("u32 fits usize") {
        return false;
    }
    for (at, balance) in balances.iter().enumerate() {
        for earlier in balances.iter().take(at) {
            if earlier.funder == balance.funder {
                return false;
            }
        }
        match balance.funder {
            Funder::Task(number) => {
                let Some(task) = record(d, number) else {
                    return false;
                };
                if task.project != project
                    || task.numbers != balance.before
                    || (balance.after.reserved > balance.before.reserved && !crate::amend::mutable(&task.phase))
                {
                    return false;
                }
            }
            Funder::Pool { project: other, .. } | Funder::Period { project: other, .. } => {
                if other != project {
                    return false;
                }
            }
        }
        if available(balance.after).is_none() {
            return false;
        }
    }
    true
}
pub(crate) fn apply_balances(d: &mut Domain, env: &Env<Limits>, balances: &[Balance], out: &mut Queue<Request>) {
    for balance in balances {
        match balance.funder {
            Funder::Task(number) => {
                task_mut(d, number).expect("balance validated").record.numbers = balance.after;
                publish(d, env, number, out);
            }
            Funder::Pool { .. } | Funder::Period { .. } => {
                out.push(Request::Save { record: Stored::Funding { funder: balance.funder, numbers: balance.after } });
            }
        }
    }
}
pub(crate) fn closure(number: u64, generation: u64, funder: Funder, numbers: Numbers, out: &mut Queue<Request>) {
    out.push(Request::Save {
        record: Stored::Closure(Closure {
            task: number,
            generation,
            funder,
            budget: numbers.budget,
            spent: total(numbers).expect("accounting total checked"),
        }),
    });
}
/// Existing batches carry snapshots only. 02c operations require complete,
/// coherent actual funding snapshots; 02e integrates ordinary carving.
pub(crate) fn funded_live(d: &Domain, funder: u64) -> bool {
    for (number, _) in &d.names {
        if *number != funder && record(d, *number).expect("live name").funder == Funder::Task(funder) {
            return true;
        }
    }
    false
}

pub(crate) fn can_reserve(d: &Domain, batch: &[crate::New]) -> bool {
    for new in batch {
        if new.numbers != (Numbers { budget: new.authority.budget.spend, spent: 0, spent_below: 0, reserved: 0 }) {
            return false;
        }
        match new.funder {
            Funder::Task(number) => {
                let Some(funder) = record(d, number) else {
                    return false;
                };
                if funder.project != new.project || !crate::amend::mutable(&funder.phase) {
                    return false;
                }
                let mut after = funder.numbers;
                for sibling in batch {
                    if sibling.funder == new.funder {
                        let Some(reserved) = after.reserved.checked_add(sibling.numbers.budget) else {
                            return false;
                        };
                        after.reserved = reserved;
                    }
                }
                if available(after).is_none() {
                    return false;
                }
            }
            Funder::Pool { project, .. } | Funder::Period { project, .. } => {
                if project != new.project {
                    return false;
                }
            }
        }
    }
    true
}
pub(crate) fn reserve(d: &mut Domain, env: &Env<Limits>, batch: &[crate::New], out: &mut Queue<Request>) {
    for new in batch {
        match new.funder {
            Funder::Task(number) => {
                let task = task_mut(d, number).expect("actual funder admitted");
                task.record.numbers.reserved =
                    task.record.numbers.reserved.checked_add(new.numbers.budget).expect("reservation admitted");
                publish(d, env, number, out);
            }
            // The root reserved authentic external funding before Make; its
            // original pool/period stays named on the task and its closure.
            Funder::Pool { .. } | Funder::Period { .. } => {}
        }
    }
}
pub(crate) fn end(d: &mut Domain, env: &Env<Limits>, number: u64, out: &mut Queue<Request>) {
    let task = record(d, number).expect("ending task live");
    let funder = task.funder;
    let budget = task.numbers.budget;
    let spent = total(task.numbers).expect("total checked on charge and restore");
    closure(number, task.allotment, funder, task.numbers, out);
    match funder {
        Funder::Task(parent) => {
            let parent_number = parent;
            let parent = task_mut(d, parent_number).expect("actual funder remains live until its allocations close");
            parent.record.numbers.reserved =
                parent.record.numbers.reserved.checked_sub(budget).expect("allotment reserved on admission");
            parent.record.numbers.spent_below = parent
                .record
                .numbers
                .spent_below
                .checked_add(spent)
                .expect("root ensures priced aggregates fit accounting unit");
            publish(d, env, parent_number, out);
        }
        // The root applies this unique closure to the original external
        // balance before committing the task end. No period reset loses it.
        Funder::Pool { .. } | Funder::Period { .. } => {}
    }
}

pub(crate) fn links(d: &Domain, bound: u32) -> bool {
    for (number, _) in &d.names {
        let task = record(d, *number).expect("live name");
        let mut reserved = 0_u64;
        for (child, _) in &d.names {
            let child = record(d, *child).expect("live name");
            if child.funder == Funder::Task(*number) {
                let Some(total) = reserved.checked_add(child.numbers.budget) else {
                    return false;
                };
                reserved = total;
            }
        }
        if reserved != task.numbers.reserved {
            return false;
        }
        let mut at = Some(*number);
        for hop in 0..bound {
            let Some(current) = at else {
                break;
            };
            let Some(node) = record(d, current) else {
                return false;
            };
            if node.project != task.project {
                return false;
            }
            at = match node.funder {
                Funder::Task(parent) => {
                    if parent == *number || hop.saturating_add(1) == bound {
                        return false;
                    }
                    Some(parent)
                }
                Funder::Pool { project, .. } | Funder::Period { project, .. } => {
                    if project != task.project {
                        return false;
                    }
                    None
                }
            };
        }
    }
    true
}
