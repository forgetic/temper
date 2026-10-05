//! Six failure classes and equal-jitter exponential backoff, ported from
//! legacy work's tracked.rs and boundary.rs (domain/tasks.md, 5.5).
use skein_lib::{Duration, Rng};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class {
    Transient,
    Permanent,
    Run,
    Agent,
    Lost,
    Invalid,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tries {
    pub transient: u32,
    pub permanent: u32,
    pub run: u32,
    pub agent: u32,
    pub lost: u32,
    pub invalid: u32,
}

impl Tries {
    pub const NONE: Tries = Tries { transient: 0, permanent: 0, run: 0, agent: 0, lost: 0, invalid: 0 };

    #[must_use]
    pub const fn of(&self, class: Class) -> u32 {
        match class {
            Class::Transient => self.transient,
            Class::Permanent => self.permanent,
            Class::Run => self.run,
            Class::Agent => self.agent,
            Class::Lost => self.lost,
            Class::Invalid => self.invalid,
        }
    }

    pub(crate) fn add(&mut self, class: Class) {
        match class {
            Class::Transient => self.transient = self.transient.saturating_add(1),
            Class::Permanent => self.permanent = self.permanent.saturating_add(1),
            Class::Run => self.run = self.run.saturating_add(1),
            Class::Agent => self.agent = self.agent.saturating_add(1),
            Class::Lost => self.lost = self.lost.saturating_add(1),
            Class::Invalid => self.invalid = self.invalid.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retry {
    pub retries: u32,
    pub base: Duration,
    pub max: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retries {
    pub transient: Retry,
    pub permanent: Retry,
    pub run: Retry,
    pub agent: Retry,
    pub lost: Retry,
    pub invalid: Retry,
}

impl Retries {
    #[must_use]
    pub const fn of(&self, class: Class) -> Retry {
        match class {
            Class::Transient => self.transient,
            Class::Permanent => self.permanent,
            Class::Run => self.run,
            Class::Agent => self.agent,
            Class::Lost => self.lost,
            Class::Invalid => self.invalid,
        }
    }
}

pub(crate) fn backoff(times: u32, retry: Retry, rng: &mut Rng) -> Duration {
    let factor = 1_u64.checked_shl(times.saturating_sub(1)).unwrap_or(u64::MAX);
    let ceiling = retry.base.saturating_mul(factor).min(retry.max);
    let half = ceiling.as_nanos().div_euclid(2);
    Duration::from_nanos(half.saturating_add(rng.below(half.saturating_add(1))))
}
