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
    if !representable(d, number, delta) {
        return refused(to, Some(number), Refusal::Funding, out);
    }
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
/// Validate owned authentic before snapshots before any mutation. Root checks
/// current authority; tasks checks concrete finite balances and arithmetic.
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
                let Some(ledger) = d.funding.get(&balance.funder) else {
                    return false;
                };
                if other != project || ledger.closed || ledger.numbers != balance.before {
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
                d.funding.get_mut(&balance.funder).expect("authentic balance validated").numbers = balance.after;
                save_funding(d, balance.funder, out);
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
/// Actual task sources cannot end while any incoming allocation remains live.
pub(crate) fn funded_live(d: &Domain, funder: u64) -> bool {
    for (number, _) in &d.names {
        if *number != funder && record(d, *number).expect("live name").funder == Funder::Task(funder) {
            return true;
        }
    }
    false
}

pub(crate) fn can_reserve(d: &Domain, creator: crate::Party, batch: &[crate::New]) -> bool {
    for new in batch {
        if new.numbers != (Numbers { budget: new.authority.budget.spend, spent: 0, spent_below: 0, reserved: 0 }) {
            return false;
        }
        match new.funder {
            Funder::Task(number) => {
                let Some(funder) = record(d, number) else {
                    return false;
                };
                let ancestor = match creator {
                    crate::Party::Task(requester) => crate::amend::below(d, requester, number, d.names.len()),
                    crate::Party::Person(_) | crate::Party::Deployment { .. } => false,
                };
                if !ancestor || funder.project != new.project || !crate::amend::mutable(&funder.phase) {
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
                let Some(ledger) = d.funding.get(&new.funder) else {
                    return false;
                };
                if project != new.project || ledger.closed {
                    return false;
                }
                let mut after = ledger.numbers;
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
            // Ordinary external reservations are owned here, in the Make commit.
            Funder::Pool { .. } | Funder::Period { .. } => {
                let ledger = d.funding.get_mut(&new.funder).expect("finite source admitted");
                ledger.numbers.reserved =
                    ledger.numbers.reserved.checked_add(new.numbers.budget).expect("reservation admitted");
                save_funding(d, new.funder, out);
            }
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
        // Post once against the actual original source in the task-end commit.
        Funder::Pool { .. } | Funder::Period { .. } => {
            let ledger = d.funding.get_mut(&funder).expect("actual source preserved");
            ledger.numbers.reserved = ledger.numbers.reserved.checked_sub(budget).expect("allotment reserved");
            ledger.numbers.spent_below =
                ledger.numbers.spent_below.checked_add(spent).expect("priced aggregate fits unit");
            save_funding(d, funder, out);
        }
    }
}

pub(crate) fn links(d: &Domain, bound: u32) -> bool {
    for (funder, ledger) in &d.funding {
        let mut reserved = 0_u64;
        for (_, other) in &d.funding {
            if other.parent == Some(*funder) && !other.closed {
                let Some(sum) = reserved.checked_add(other.numbers.budget) else {
                    return false;
                };
                reserved = sum;
            }
        }
        for (number, _) in &d.names {
            let task = record(d, *number).expect("live name");
            if task.funder == *funder {
                if ledger.closed {
                    return false;
                }
                let Some(sum) = reserved.checked_add(task.numbers.budget) else {
                    return false;
                };
                reserved = sum;
            }
        }
        if reserved != ledger.numbers.reserved {
            return false;
        }
        if let Some(parent) = ledger.parent
            && !d.funding.contains_key(&parent)
        {
            return false;
        }
    }
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
                    if project != task.project || !d.funding.contains_key(&node.funder) {
                        return false;
                    }
                    None
                }
            };
        }
    }
    true
}

/// Live external allotment. A pool's actual parent is immutable across resets
/// and requester moves (domain/tasks.md, 2). Source retirement remains the
/// later 02e depth increment; this slice refuses new sources at its live bound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FundingRecord {
    /// Durable actual source identity (domain/tasks.md, 2).
    pub funder: Funder,
    /// A pool's original project period; periods have no parent (domain/tasks.md, 2).
    pub parent: Option<Funder>,
    /// Finite authentic counters owned and updated by tasks (domain/tasks.md, 2).
    pub numbers: Numbers,
    /// Reserved for later bounded source retirement; currently always false
    /// (domain/tasks.md, 2).
    pub closed: bool,
}

fn save_funding(domain: &Domain, funder: Funder, out: &mut Queue<Request>) {
    let ledger = *domain.funding.get(&funder).expect("funding admitted");
    out.push(Request::Save { record: Stored::Ledger(ledger) });
    out.push(Request::Save { record: Stored::Funding { funder, numbers: ledger.numbers } });
}

pub(crate) fn open(domain: &mut Domain, to: ReplyTo, project: u32, period: u64, budget: u64, out: &mut Queue<Request>) {
    if !domain.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    let funder = Funder::Period { project, period };
    if domain.funding.contains_key(&funder) {
        return refused(to, None, Refusal::Duplicate, out);
    }
    // Monotonic identities prevent reintroducing a reset's old period.
    for (other, _) in &domain.funding {
        match *other {
            Funder::Period { project: p, period: old } => {
                if p == project && old >= period {
                    return refused(to, None, Refusal::Funding, out);
                }
            }
            Funder::Task(_) | Funder::Pool { .. } => {}
        }
    }
    if domain.funding.len() == domain.funding.capacity() {
        return refused(to, None, Refusal::Busy, out);
    }
    let record = FundingRecord {
        funder,
        parent: None,
        numbers: Numbers { budget, spent: 0, spent_below: 0, reserved: 0 },
        closed: false,
    };
    assert!(domain.funding.insert(funder, record) == Ok(None), "period admitted");
    save_funding(domain, funder, out);
    out.push(Request::Done { reply_to: to });
}

pub(crate) fn carve(
    domain: &mut Domain,
    to: ReplyTo,
    project: u32,
    person: u64,
    period: u64,
    budget: u64,
    out: &mut Queue<Request>,
) {
    if !domain.ready() {
        return refused(to, None, Refusal::NotReady, out);
    }
    let funder = Funder::Pool { project, person, period };
    let parent = Funder::Period { project, period };
    if domain.funding.contains_key(&funder) {
        return refused(to, None, Refusal::Duplicate, out);
    }
    let Some(old) = domain.funding.get(&parent) else {
        return refused(to, None, Refusal::Funding, out);
    };
    let mut after = old.numbers;
    let Some(reserved) = after.reserved.checked_add(budget) else {
        return refused(to, None, Refusal::Funding, out);
    };
    after.reserved = reserved;
    if old.closed || available(after).is_none() {
        return refused(to, None, Refusal::Funding, out);
    }
    if domain.funding.len() == domain.funding.capacity() {
        return refused(to, None, Refusal::Busy, out);
    }
    let record = FundingRecord {
        funder,
        parent: Some(parent),
        numbers: Numbers { budget, spent: 0, spent_below: 0, reserved: 0 },
        closed: false,
    };
    assert!(domain.funding.insert(funder, record) == Ok(None), "pool admitted");
    domain.funding.get_mut(&parent).expect("period admitted").numbers = after;
    save_funding(domain, parent, out);
    save_funding(domain, funder, out);
    out.push(Request::Done { reply_to: to });
}

pub(crate) fn restore_funding(domain: &mut Domain, ledger: FundingRecord) -> bool {
    if domain.funding.contains_key(&ledger.funder)
        || domain.funding.len() == domain.funding.capacity()
        || total(ledger.numbers).is_none()
    {
        return false;
    }
    let valid = match ledger.funder {
        Funder::Task(_) => false,
        Funder::Period { .. } => ledger.parent.is_none() && !ledger.closed,
        Funder::Pool { project, period, .. } => {
            ledger.parent == Some(Funder::Period { project, period })
                && (!ledger.closed || ledger.numbers.reserved == 0)
        }
    };
    if !valid {
        return false;
    }
    assert!(domain.funding.insert(ledger.funder, ledger) == Ok(None), "restored ledger admitted");
    true
}

// The final period will eventually receive every still-open aggregate under
// it. Counting each live node's own/closed spend once bounds every intermediate
// task and pool posting as well. No accepted overrun can overflow settlement.
fn original_period(domain: &Domain, number: u64) -> Option<Funder> {
    let mut funder = record(domain, number)?.funder;
    for _ in 0..domain.names.len().checked_add(2)? {
        match funder {
            Funder::Task(number) => funder = record(domain, number)?.funder,
            Funder::Pool { .. } => funder = domain.funding.get(&funder)?.parent?,
            Funder::Period { .. } => return Some(funder),
        }
    }
    None
}

pub(crate) fn representable(domain: &Domain, number: u64, delta: u64) -> bool {
    let Some(period) = original_period(domain, number) else {
        return false;
    };
    let Some(ledger) = domain.funding.get(&period) else {
        return false;
    };
    let Some(base) = total(ledger.numbers) else {
        return false;
    };
    let Some(mut eventual) = base.checked_add(delta) else {
        return false;
    };
    for (_, pool) in &domain.funding {
        if pool.parent == Some(period) && !pool.closed {
            let Some(spent) = total(pool.numbers) else {
                return false;
            };
            let Some(sum) = eventual.checked_add(spent) else {
                return false;
            };
            eventual = sum;
        }
    }
    for (number, _) in &domain.names {
        if original_period(domain, *number) == Some(period) {
            let Some(spent) = total(record(domain, *number).expect("live name").numbers) else {
                return false;
            };
            let Some(sum) = eventual.checked_add(spent) else {
                return false;
            };
            eventual = sum;
        }
    }
    true
}
