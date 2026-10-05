//! Per-class task failure history and configured equal-jitter backoff
//! (domain/tasks.md, sections 5.5 and 14). Root classifies external failures;
//! this module uses injected randomness and owns no clock or peer state.
use skein_lib::{Duration, Rng};

/// Root-reported activation failure category; each category has its own bounded retry policy and
/// counter. (domain/tasks.md, sections 5.5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Class {
    /// Temporary peer/read/transcript failure reported by the root. (domain/tasks.md, sections 5.5
    /// and 14).
    Transient,
    /// Failure requiring an external correction, such as an unavailable repository or invalid
    /// assignment. (domain/tasks.md, sections 5.5 and 14).
    Permanent,
    /// `Run` itself reported execution failure. (domain/tasks.md, sections 5.5 and 14).
    Run,
    /// `Agent` crashed, violated its channel contract or stalled. (domain/tasks.md, sections 5.5
    /// and 14).
    Agent,
    /// Worker was lost. (domain/tasks.md, sections 5.5 and 14).
    Lost,
    /// Returned result violated the admitted task contract. (domain/tasks.md, sections 5.5 and 14).
    Invalid,
}

/// Task's per-class failure counters, updated with saturation; typed counts describe history rather
/// than a retry permission. (domain/tasks.md, sections 5.5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tries {
    /// Count of transient failures for this task. (domain/tasks.md, sections 5.5 and 14).
    pub transient: u32,
    /// Count of failures awaiting an external correction. (domain/tasks.md, sections 5.5 and 14).
    pub permanent: u32,
    /// Count of run-reported failures. (domain/tasks.md, sections 5.5 and 14).
    pub run: u32,
    /// Count of agent execution failures. (domain/tasks.md, sections 5.5 and 14).
    pub agent: u32,
    /// Count of lost-worker failures. (domain/tasks.md, sections 5.5 and 14).
    pub lost: u32,
    /// Count of results that violated the task contract. (domain/tasks.md, sections 5.5 and 14).
    pub invalid: u32,
}

impl Tries {
    /// All failure counters zero; a `new` task and a successful park start with this value.
    /// (domain/tasks.md, sections 5.5 and 14).
    pub const NONE: Tries = Tries { transient: 0, permanent: 0, run: 0, agent: 0, lost: 0, invalid: 0 };

    /// Pure selection of the counter or retry policy for `class`; no allocation, mutation, output
    /// or lifecycle transition. (domain/tasks.md, sections 5.5 and 14).
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

/// Root-configured retry count and equal-jitter exponential delay bounds; startup validates
/// representable usable settings. (domain/tasks.md, sections 5.5 and 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retry {
    /// Failures allowed before the task is held; startup rejects `u32::MAX`. (domain/tasks.md,
    /// sections 5.5 and 14).
    pub retries: u32,
    /// Positive initial exponential-backoff delay; startup rejects zero. (domain/tasks.md, sections
    /// 5.5 and 14).
    pub base: Duration,
    /// Backoff ceiling, required to be at least `base`; jitter selects within the upper half of the
    /// capped delay. (domain/tasks.md, sections 5.5 and 14).
    pub max: Duration,
}

/// One configured retry policy per exhaustive failure class. (domain/tasks.md, sections 5.5 and
/// 14).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Retries {
    /// `Transient`-failure and pre-execution refusal pause policy. (domain/tasks.md, sections 5.5
    /// and 14).
    pub transient: Retry,
    /// Policy for failures requiring an external correction. (domain/tasks.md, sections 5.5 and
    /// 14).
    pub permanent: Retry,
    /// Policy for run-reported failures. (domain/tasks.md, sections 5.5 and 14).
    pub run: Retry,
    /// Policy for agent execution failures. (domain/tasks.md, sections 5.5 and 14).
    pub agent: Retry,
    /// Policy for lost-worker failures. (domain/tasks.md, sections 5.5 and 14).
    pub lost: Retry,
    /// Policy for contract-invalid results. (domain/tasks.md, sections 5.5 and 14).
    pub invalid: Retry,
}

impl Retries {
    /// Pure selection of the counter or retry policy for `class`; no allocation, mutation, output
    /// or lifecycle transition. (domain/tasks.md, sections 5.5 and 14).
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
