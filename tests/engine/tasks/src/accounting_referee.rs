//! Independent conservation from durable task allocations, immutable closure
//! rows and admitted priced inputs (domain/tasks.md, 2 and 5).
use std::collections::BTreeMap;
use temper_engine_domain_tasks::{Funder, Key, Stored, TaskRecord};

#[derive(Default, Debug)]
pub struct Accounting {
    before: BTreeMap<Key, Stored>,
    charged: Option<(u64, u64)>,
}

fn live(rows: &BTreeMap<Key, Stored>, task: u64) -> Option<&TaskRecord> {
    match rows.get(&Key::Live(task)) {
        Some(Stored::Live(row)) => Some(row),
        _ => None,
    }
}

fn current(rows: &BTreeMap<Key, Stored>, task: u64) -> Option<&TaskRecord> {
    live(rows, task).or_else(|| match rows.get(&Key::Ended(task)) {
        Some(Stored::Ended(row)) => Some(row),
        _ => None,
    })
}

impl Accounting {
    /// Accepted cumulative input supplied by the scripted parent, independently
    /// of production counters (domain/tasks.md, 5).
    pub fn charged(&mut self, task: u64, cumulative: u64) {
        let old = live(&self.before, task).expect("priced input names a live allocation");
        self.charged = Some((task, cumulative.checked_sub(old.run_spent).expect("admitted cumulative grows")));
    }

    fn delta(&self, task: u64) -> u64 {
        self.charged.filter(|(number, _)| *number == task).map_or(0, |(_, delta)| delta)
    }

    /// # Errors
    /// Rejects changed identities, missing postings, lost reservations or
    /// invented expense at the durable boundary (domain/tasks.md, 2).
    #[expect(clippy::too_many_lines, reason = "independent bounded conservation checks remain together")]
    pub fn committed(&mut self, rows: &BTreeMap<Key, Stored>) -> Result<(), &'static str> {
        for (key, old) in &self.before {
            if matches!(old, Stored::Closure(_) | Stored::Ended(_)) && rows.get(key) != Some(old) {
                return Err("immutable accounting row changed");
            }
            if matches!(old, Stored::Ledger(_)) && !rows.contains_key(key) {
                return Err("funding source vanished");
            }
        }
        for old in self.before.values() {
            let Stored::Live(old) = old else {
                continue;
            };
            let task = current(rows, old.number).ok_or("live allocation vanished")?;
            if old.funder != task.funder
                || old.allotment != task.allotment
                || old.numbers.budget != task.numbers.budget
                || old.historical_spend != task.historical_spend
            {
                return Err("allocation identity or promise changed");
            }
            let posted: u64 = rows
                .iter()
                .filter_map(|(key, row)| match row {
                    Stored::Closure(closure)
                        if closure.funder == Funder::Task(old.number) && !self.before.contains_key(key) =>
                    {
                        Some(closure.spent)
                    }
                    Stored::Live(_) | Stored::Ended(_) | Stored::Ledger(_) | Stored::Closure(_) => None,
                })
                .sum();
            if task.numbers.spent != old.numbers.spent + self.delta(old.number)
                || task.numbers.spent_below != old.numbers.spent_below + posted
            {
                return Err("task expense or actual descendant posting differs");
            }
            if task.attempt == old.attempt && task.run_spent != old.run_spent + self.delta(old.number) {
                return Err("cumulative attempt expense differs");
            }
            if matches!(task.phase, temper_engine_domain_tasks::Phase::Ended(_))
                && !rows.contains_key(&Key::Closure { task: old.number, generation: old.allotment })
            {
                return Err("ending lacks allotment closure");
            }
        }
        for (key, row) in rows {
            match row {
                Stored::Closure(closure) if !self.before.contains_key(key) => {
                    let old = live(&self.before, closure.task).ok_or("closure without live allotment")?;
                    let task = current(rows, closure.task).ok_or("closure without ended task")?;
                    if closure.generation != old.allotment
                        || closure.funder != old.funder
                        || closure.budget != old.numbers.budget
                        || closure.spent != task.numbers.spent + task.numbers.spent_below
                    {
                        return Err("closure identity or expense differs");
                    }
                }
                Stored::Live(task) => {
                    let reserved: u64 = rows
                        .values()
                        .filter_map(|row| match row {
                            Stored::Live(child) if child.funder == Funder::Task(task.number) => {
                                Some(child.numbers.budget)
                            }
                            Stored::Live(_) | Stored::Ended(_) | Stored::Ledger(_) | Stored::Closure(_) => None,
                        })
                        .sum();
                    if reserved != task.numbers.reserved {
                        return Err("task reservation differs from actual funding links");
                    }
                    if !self.before.contains_key(key)
                        && (task.numbers.spent != 0 || task.numbers.spent_below != 0 || task.run_spent != 0)
                    {
                        return Err("new allocation carries invented expense");
                    }
                }
                Stored::Ledger(ledger) => {
                    let reserved: u64 = rows
                        .values()
                        .filter_map(|row| match row {
                            Stored::Live(task) if task.funder == ledger.funder => Some(task.numbers.budget),
                            Stored::Ledger(pool) if pool.parent == Some(ledger.funder) && !pool.closed => {
                                Some(pool.numbers.budget)
                            }
                            Stored::Live(_) | Stored::Ended(_) | Stored::Ledger(_) | Stored::Closure(_) => None,
                        })
                        .sum();
                    let posted: u64 = rows
                        .values()
                        .filter_map(|row| match row {
                            Stored::Closure(closure) if closure.funder == ledger.funder => Some(closure.spent),
                            Stored::Ledger(pool) if pool.parent == Some(ledger.funder) && pool.closed => {
                                Some(pool.numbers.spent + pool.numbers.spent_below)
                            }
                            Stored::Live(_) | Stored::Ended(_) | Stored::Ledger(_) | Stored::Closure(_) => None,
                        })
                        .sum();
                    if ledger.numbers.reserved != reserved
                        || ledger.numbers.spent != 0
                        || ledger.numbers.spent_below != posted
                    {
                        return Err("external reservation or actual closure posting differs");
                    }
                    if let Some(Stored::Ledger(old)) = self.before.get(key)
                        && (ledger.funder != old.funder
                            || ledger.parent != old.parent
                            || ledger.numbers.budget != old.numbers.budget
                            || ledger.closed != old.closed)
                    {
                        return Err("original source identity changed");
                    }
                }
                Stored::Ended(_) | Stored::Closure(_) => {}
            }
        }
        self.before = rows.clone();
        self.charged = None;
        Ok(())
    }

    pub fn reset(&mut self, rows: &BTreeMap<Key, Stored>) {
        self.before = rows.clone();
        self.charged = None;
    }
}
