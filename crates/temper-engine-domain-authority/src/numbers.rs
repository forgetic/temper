//! Carved funding and exact spend (domain/authority.md, section 7).
//!
//! These functions own no counters: the tasks child keeps its own numbers,
//! and the root translates their values. An arithmetic or accounting
//! refusal returns `None` without changing any caller's state. Batch slices
//! have already been admitted under the caller's task limit. The caller
//! checks actual funder links and commits a task's settlement once; these
//! snapshots cannot recognize a duplicate settlement.

/// The four numbers against one current allotment. An overrun may put spend
/// above the budget; it is still counted and leaves no available budget.
/// Caller-owned allotment snapshot; value queries return replacements without keeping a ledger or
/// recognizing duplicate settlements. (domain/authority.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Numbers {
    /// Full amount reserved for this current allotment. (domain/authority.md, section 7).
    pub budget: u64,
    /// Actual spend charged directly to this allotment, including overruns. (domain/authority.md,
    /// section 7).
    pub spent: u64,
    /// Settled spend from allotments funded below it, counted once by the caller.
    /// (domain/authority.md, section 7).
    pub spent_below: u64,
    /// Full budgets of directly funded allotments still open. (domain/authority.md, section 7).
    pub reserved: u64,
}

/// Reservations name their actual funder, including its original period.
/// Starting another period never changes this name or clears its counters.
/// Spend charged even when it exceeded what was available.
/// Successful actual-spend charge, including any amount beyond the available budget.
/// (domain/authority.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Charged {
    /// Replacement snapshot with the actual amount added to direct spend. (domain/authority.md,
    /// section 7).
    pub numbers: Numbers,
    /// Amount of this charge exceeding the pre-charge available budget. (domain/authority.md,
    /// section 7).
    pub overrun: u64,
}

/// What is available, never below zero. Sequential subtraction also handles
/// overruns whose sum with live reservations would exceed an integer.
/// Pure snapshot query: subtract direct spend, settled descendant spend and reservations, flooring
/// available funding at zero without emitting output. (domain/authority.md, section 7).
#[must_use]
pub fn left(numbers: Numbers) -> u64 {
    numbers.budget.saturating_sub(numbers.spent).saturating_sub(numbers.spent_below).saturating_sub(numbers.reserved)
}

/// Reserve every budget in one checked batch, or refuse all of it.
/// Pure checked batch reservation over caller-admitted `budgets`; returns replacement funder
/// numbers or `None` on arithmetic/accounting refusal, with no partial mutation.
/// (domain/authority.md, section 7).
#[must_use]
pub fn carve(funder: Numbers, budgets: &[u64]) -> Option<Numbers> {
    spend(funder)?;
    let mut requested = 0_u64;
    for budget in budgets {
        requested = requested.checked_add(*budget)?;
    }
    if requested > left(funder) {
        return None;
    }
    Some(Numbers { reserved: funder.reserved.checked_add(requested)?, ..funder })
}

/// Close an allotment: remove its full reservation and count all its spend
/// below its funder. Every allotment it funded must have settled first.
/// The caller verifies its recorded reservation; aggregate numbers alone
/// cannot identify a task or prevent duplicate settlement.
/// Pure checked settlement returning replacement funder numbers or `None`; `ended` must have no open
/// reservations and the caller must commit the end exactly once.
/// (domain/authority.md, section 7).
#[must_use]
pub fn settle(funder: Numbers, ended: Numbers) -> Option<Numbers> {
    if ended.reserved != 0 {
        return None;
    }
    let reserved = funder.reserved.checked_sub(ended.budget)?;
    let spent_below = funder.spent_below.checked_add(spend(ended)?)?;
    let result = Numbers { spent_below, reserved, ..funder };
    spend(result)?;
    Some(result)
}

/// Charge the actual spend. Budget exhaustion is an overrun, not a refusal;
/// a total that cannot be represented is an arithmetic refusal.
/// Pure actual-spend charge returning replacement numbers and overrun; `None` means unrepresentable
/// accounting, not exhausted budget. Caller commits the charge with the accepted record.
/// (domain/authority.md, section 7).
#[must_use]
pub fn charge(task: Numbers, amount: u64) -> Option<Charged> {
    let available = left(task);
    let numbers = Numbers { spent: task.spent.checked_add(amount)?, ..task };
    spend(numbers)?;
    Some(Charged { numbers, overrun: amount.saturating_sub(available) })
}

fn spend(numbers: Numbers) -> Option<u64> {
    numbers.spent.checked_add(numbers.spent_below)
}
