//! Carved funding, exact spend and transfers (domain/authority.md, section 7).
//!
//! These functions own no counters: the tasks child keeps its own numbers,
//! and the root translates their values. An arithmetic or accounting
//! refusal returns `None` without changing any caller's state. Batch slices
//! have already been admitted under the caller's task limit. The caller
//! checks actual funder links and closes each durable allotment generation
//! once; these snapshots cannot recognize a duplicate settlement.

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
/// Actual durable funding source, including the original period; the caller validates links and
/// generation identity. (domain/authority.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Funder {
    /// An allotment funded by another task. (domain/authority.md, section 7).
    Task(/** Actual funding task number, not necessarily the topology parent. (domain/authority.md, section 7). */ u64),
    /// An allotment funded from a person's project-period pool. (domain/authority.md, section 7).
    Pool {
        /** Project owning the person's pool. (domain/authority.md, section 7). */
        project: u32,
        /** Person whose pool funded the allotment. (domain/authority.md, section 7). */
        person: u64,
        /** Original funding period, unchanged by rollover. (domain/authority.md, section 7). */
        period: u64,
    },
    /// An allotment funded directly from a project period. (domain/authority.md, section 7).
    Period {
        /** Project whose period funded the allotment. (domain/authority.md, section 7). */
        project: u32,
        /** Original project funding period. (domain/authority.md, section 7). */
        period: u64,
    },
}

/// A funder and the snapshot the root gathered for this decision.
/// Root-gathered funding snapshot for one checked decision, with no retained state here.
/// (domain/authority.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Funding {
    /// Recorded actual funder. (domain/authority.md, section 7).
    pub by: Funder,
    /// That funder's accounting snapshot for the same decision. (domain/authority.md, section 7).
    pub numbers: Numbers,
}

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

/// A transfer either keeps the same allotment, or settles the old allotment
/// and opens a new one for its unspent amount. Historical spend remains with
/// the old funder; it is not charged again to the new one.
/// Successful atomic value transfer; the caller commits all funding replacements and history
/// together. (domain/authority.md, section 7).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Moved {
    /// Same actual funder: requires identical snapshots and no replacement budgets; preserves all
    /// counters. (domain/authority.md, section 7).
    Same {
        /** Unchanged funding snapshot for the same actual funder. (domain/authority.md, section 7). */
        funding: Funding,
        /** Unchanged task allotment and reservations. (domain/authority.md, section 7). */
        task: Numbers,
    },
    /// Different actual funder: settles old spend and reserves the unspent replacement without
    /// charging historical spend again. (domain/authority.md, section 7).
    Changed {
        /** Old funder after the old allotment settles. (domain/authority.md, section 7). */
        old: Funding,
        /** New funder after reserving the replacement allotment. (domain/authority.md, section 7). */
        new: Funding,
        /** New task allotment carrying the unspent amount and checked replacement reservations. (domain/authority.md, section 7). */
        task: Numbers,
    },
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
/// This closes a task's final allotment, or an old one during a move.
/// The caller verifies its recorded reservation and unclosed generation;
/// aggregate numbers alone cannot identify an allotment.
/// Pure checked closure returning replacement funder numbers or `None`; `ended` must have no open
/// reservations and the caller must verify and close its recorded generation exactly once.
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

/// Fund a moved task anew, atomically as values. The caller first computes
/// the old funding subtree's settlement from leaves upwards, retaining the
/// unspent amount for each live task. It gives the resulting zero-reserved
/// root here, with the replacement directly funded tasks' budgets. Their
/// reservations must fit in the new root's allotment; deeper replacements
/// are checked by the caller before committing. An overrun that leaves too
/// little for a promised task refuses the move, never trims it. The caller
/// supplies every promised replacement, retaining each unspent amount;
/// this function sees amounts, not the live tasks they belong to.
/// Settlements, replacements and history records form one commit, with no
/// intermediate externally visible end. Externally funded requester
/// descendants are separate funding components, checked by the caller.
///
/// A move to the same actual funder leaves all counters and reservations
/// intact: skip normalization and pass an empty replacement slice. Two
/// different snapshots for that same funder are refused. Different periods
/// name different funders even for the same person or project.
/// Pure checked transfer over caller-admitted `replacement_budgets`; returns all replacement values
/// or `None` with no partial mutation. Caller validates links, normalizes components and commits
/// the whole transfer once. (domain/authority.md, section 7).
#[must_use]
pub fn move_funding(old: Funding, new: Funding, task: Numbers, replacement_budgets: &[u64]) -> Option<Moved> {
    if old.by == new.by {
        if old.numbers != new.numbers || !replacement_budgets.is_empty() {
            return None;
        }
        spend(old.numbers)?;
        spend(task)?;
        return Some(Moved::Same { funding: old, task });
    }
    let old_numbers = settle(old.numbers, task)?;
    let unspent = task.budget.saturating_sub(spend(task)?);
    let new_numbers = carve(new.numbers, &[unspent])?;
    let task = carve(Numbers { budget: unspent, spent: 0, spent_below: 0, reserved: 0 }, replacement_budgets)?;
    Some(Moved::Changed {
        old: Funding { numbers: old_numbers, ..old },
        new: Funding { numbers: new_numbers, ..new },
        task,
    })
}

fn spend(numbers: Numbers) -> Option<u64> {
    numbers.spent.checked_add(numbers.spent_below)
}
