//! Independent durable-ledger referee: old records and current commit rows,
//! ordinary arithmetic, no production helpers or child state access.
use std::collections::{BTreeMap, BTreeSet};
use temper_engine_domain_tasks::{Balance, Funder, Key, Numbers, Stored, TaskRecord};
#[derive(Default, Debug)]
pub struct Accounting {
    before: BTreeMap<Key, Stored>,
    funding_before: BTreeMap<Funder, Numbers>,
    funding_emitted: BTreeSet<Funder>,
}
fn live(rows: &BTreeMap<Key, Stored>, number: u64) -> Option<&TaskRecord> {
    match rows.get(&Key::Live(number)) {
        Some(Stored::Live(task)) => Some(task),
        _ => None,
    }
}
fn current(rows: &BTreeMap<Key, Stored>, number: u64) -> Option<&TaskRecord> {
    live(rows, number).or_else(|| match rows.get(&Key::Ended(number)) {
        Some(Stored::Ended(task)) => Some(task),
        _ => None,
    })
}
impl Accounting {
    /// Root-authenticated finite source snapshots before this decision. The
    /// referee uses only before values; production's proposed after is ignored.
    pub fn begin(&mut self, balances: &[Balance]) {
        self.funding_before.clear();
        self.funding_emitted.clear();
        for balance in balances {
            if !matches!(balance.funder, Funder::Task(_)) {
                self.funding_before.insert(balance.funder, balance.before);
            }
        }
    }
    pub fn emitted(&mut self, funder: Funder) {
        self.funding_emitted.insert(funder);
    }
    fn posted(&self, rows: &BTreeMap<Key, Stored>, funder: Funder) -> u64 {
        rows.iter()
            .filter_map(|(key, row)| match row {
                Stored::Closure(closure) if closure.funder == funder && !self.before.contains_key(key) => {
                    Some(closure.spent)
                }
                Stored::Live(_)
                | Stored::Ended(_)
                | Stored::Stub(_)
                | Stored::Message(_)
                | Stored::ArchivedMessage(_)
                | Stored::Receipt(_)
                | Stored::Offer(_)
                | Stored::Question(_)
                | Stored::Subscription(_)
                | Stored::History(_)
                | Stored::Closure(_)
                | Stored::Funding { .. } => None,
            })
            .sum()
    }
    fn check_postings(&self, rows: &BTreeMap<Key, Stored>) -> Result<(), &'static str> {
        for row in self.before.values() {
            if let Stored::Live(old) = row
                && let Some(task) = current(rows, old.number)
                && old.allotment == task.allotment
            {
                let posted = self.posted(rows, Funder::Task(old.number));
                if task.numbers.spent_below != old.numbers.spent_below + posted {
                    return Err("closed spend not posted to actual task funder");
                }
            }
        }
        for funder in &self.funding_emitted {
            let before =
                self.funding_before.get(funder).ok_or("external balance lacks authenticated prior snapshot")?;
            let Some(Stored::Funding { numbers: after, .. }) = rows.get(&Key::Funding(*funder)) else {
                return Err("external balance emission missing");
            };
            let posted = self.posted(rows, *funder);
            if after.budget != before.budget
                || after.spent != before.spent
                || after.spent_below != before.spent_below + posted
            {
                return Err("closed spend not posted to actual external funder");
            }
            // Net every changed incoming live reservation independently. This
            // also covers amendments whose allotment generation stays the same.
            let mut old_reserved = 0;
            let mut new_reserved = 0;
            for row in self.before.values() {
                if let Stored::Live(task) = row
                    && task.funder == *funder
                {
                    old_reserved += task.numbers.budget;
                }
            }
            for row in rows.values() {
                if let Stored::Live(task) = row
                    && task.funder == *funder
                {
                    new_reserved += task.numbers.budget;
                }
            }
            if before.reserved.checked_sub(old_reserved).and_then(|rest| rest.checked_add(new_reserved))
                != Some(after.reserved)
            {
                return Err("external balance lost actual reservations");
            }
        }
        Ok(())
    }

    fn immutable(&self, rows: &BTreeMap<Key, Stored>) -> Result<(), &'static str> {
        for (key, row) in &self.before {
            if matches!(row, Stored::Closure(_) | Stored::History(_)) && rows.get(key) != Some(row) {
                return Err("immutable accounting/history row changed");
            }
        }
        Ok(())
    }
    /// # Errors
    /// Reports conservation, generation, or actual funding-link violations.
    pub fn committed(&mut self, rows: &BTreeMap<Key, Stored>) -> Result<(), &'static str> {
        self.immutable(rows)?;
        for (key, row) in rows {
            if let Stored::Closure(closure) = row {
                if self.before.contains_key(key) {
                    continue;
                }
                let old = live(&self.before, closure.task).ok_or("closure without live allotment")?;
                if old.allotment != closure.generation
                    || old.funder != closure.funder
                    || old.numbers.budget != closure.budget
                {
                    return Err("wrong allotment closure identity");
                }
                let mut spent = old.numbers.spent + old.numbers.spent_below;
                for (child_key, child_row) in rows {
                    if let Stored::Closure(child) = child_row
                        && child.funder == Funder::Task(closure.task)
                        && !self.before.contains_key(child_key)
                    {
                        spent += child.spent;
                    }
                }
                if closure.spent != spent {
                    return Err("closure lost actual descendant spend");
                }
                if let Some(task) = live(rows, closure.task) {
                    if task.allotment != closure.generation + 1
                        || task.numbers.budget.checked_add(spent) != Some(closure.budget)
                        || task.numbers.spent != 0
                        || task.numbers.spent_below != 0
                        || task.historical_spend != old.historical_spend + spent
                        || task.run_spent != old.run_spent
                    {
                        return Err("replacement lost promise or cumulative spend");
                    }
                } else if !rows.contains_key(&Key::Ended(closure.task)) {
                    return Err("closed allotment vanished");
                }
            }
        }
        for row in rows.values() {
            if let Stored::Live(task) = row {
                let reserved: u64 = rows
                    .values()
                    .filter_map(|row| match row {
                        Stored::Live(child) if child.funder == Funder::Task(task.number) => Some(child.numbers.budget),
                        Stored::Live(_)
                        | Stored::Ended(_)
                        | Stored::Stub(_)
                        | Stored::Message(_)
                        | Stored::ArchivedMessage(_)
                        | Stored::Receipt(_)
                        | Stored::Offer(_)
                        | Stored::Question(_)
                        | Stored::Subscription(_)
                        | Stored::History(_)
                        | Stored::Closure(_)
                        | Stored::Funding { .. } => None,
                    })
                    .sum();
                if reserved != task.numbers.reserved {
                    return Err("reservation differs from actual funding links");
                }
                if let Some(old) = live(&self.before, task.number) {
                    if task.attempt == old.attempt && task.run_spent < old.run_spent {
                        return Err("cumulative expense decreased");
                    }
                    if task.allotment == old.allotment && task.funder != old.funder {
                        return Err("funding replaced without closure");
                    }
                    if task.allotment != old.allotment
                        && !rows.contains_key(&Key::Closure { task: task.number, generation: old.allotment })
                    {
                        return Err("replacement lacks closure");
                    }
                    if old.requester != task.requester
                        && let temper_engine_domain_tasks::Party::Task(parent) = old.requester
                        && !task.references.contains(&parent)
                    {
                        return Err("move lost old requester reference");
                    }
                }
            }
        }
        for row in self.before.values() {
            if let Stored::Live(old) = row {
                let task = current(rows, old.number).ok_or("live allotment vanished")?;
                if matches!(task.phase, temper_engine_domain_tasks::Phase::Ended(_))
                    && !rows.contains_key(&Key::Closure { task: old.number, generation: old.allotment })
                {
                    return Err("ending lacks allotment closure");
                }
            }
        }
        self.check_postings(rows)?;
        self.before = rows.clone();
        self.funding_before.clear();
        self.funding_emitted.clear();
        Ok(())
    }
    pub fn reset(&mut self, rows: &BTreeMap<Key, Stored>) {
        self.before = rows.clone();
        self.funding_before.clear();
        self.funding_emitted.clear();
    }
}
