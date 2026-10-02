//! One budget per run (agent-model.md, 4.2): turns, tokens and time, across
//! every conversation the run opens.

use temper_lib::Duration;

/// What a run may spend across all its conversations: completions, tokens of
/// each kind as providers count them, and time from its admission.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Budget {
    pub turns: u32,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub time: Duration,
}

/// What was spent: completions, and tokens of each kind as the provider counts
/// them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Spend {
    pub turns: u32,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// The part of a budget that ran out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Exhausted {
    Turns,
    Input,
    Output,
    CacheRead,
    CacheWrite,
    Time,
}

impl Spend {
    pub const ZERO: Spend = Spend { turns: 0, input: 0, output: 0, cache_read: 0, cache_write: 0 };

    #[must_use]
    pub const fn saturating_add(self, other: Spend) -> Spend {
        Spend {
            turns: self.turns.saturating_add(other.turns),
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }

    /// What this spends beyond `other`, kind by kind, or nothing of a kind it
    /// spends no more of.
    #[must_use]
    pub const fn saturating_sub(self, other: Spend) -> Spend {
        Spend {
            turns: self.turns.saturating_sub(other.turns),
            input: self.input.saturating_sub(other.input),
            output: self.output.saturating_sub(other.output),
            cache_read: self.cache_read.saturating_sub(other.cache_read),
            cache_write: self.cache_write.saturating_sub(other.cache_write),
        }
    }
}

impl Budget {
    /// Whether this budget asks for no more than `limit`, part by part.
    pub(crate) fn within(&self, limit: &Budget) -> bool {
        self.turns <= limit.turns
            && self.input <= limit.input
            && self.output <= limit.output
            && self.cache_read <= limit.cache_read
            && self.cache_write <= limit.cache_write
            && self.time <= limit.time
    }

    /// Whether this budget leaves room for any work: a turn, its input and
    /// output, and time. Caching may be given no budget.
    pub(crate) fn is_workable(&self) -> bool {
        self.turns > 0 && self.input > 0 && self.output > 0 && self.time > Duration::ZERO
    }

    /// The first part of this budget that `spent` has gone past, if any. Time
    /// is the run's alarm, not a part that is spent.
    pub(crate) fn overspent(&self, spent: Spend) -> Option<Exhausted> {
        if spent.turns > self.turns {
            Some(Exhausted::Turns)
        } else if spent.input > self.input {
            Some(Exhausted::Input)
        } else if spent.output > self.output {
            Some(Exhausted::Output)
        } else if spent.cache_read > self.cache_read {
            Some(Exhausted::CacheRead)
        } else if spent.cache_write > self.cache_write {
            Some(Exhausted::CacheWrite)
        } else {
            None
        }
    }

    /// What is left of this budget once `spent` is spent, with `time` to go.
    pub(crate) fn remainder(&self, spent: Spend, time: Duration) -> Budget {
        Budget {
            turns: self.turns.saturating_sub(spent.turns),
            input: self.input.saturating_sub(spent.input),
            output: self.output.saturating_sub(spent.output),
            cache_read: self.cache_read.saturating_sub(spent.cache_read),
            cache_write: self.cache_write.saturating_sub(spent.cache_write),
            time,
        }
    }
}
