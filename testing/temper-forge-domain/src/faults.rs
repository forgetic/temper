//! Faults, drawn from the seed per call (testing.md, 4): a rate limit
//! per user per window, failures before a call is made and after, and the
//! latency of its answer, sometimes late.

use skein_lib::{Duration, Env, Time};

use crate::api::{Answer, Error};
use crate::domain::{self, Config, Domain};

/// A user's rate window: when it ends, and the calls taken in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Window {
    end: Time,
    calls: u32,
}

/// Takes a call from `user`, or refuses it: for the user's rate, or as
/// unavailable by chance, before anything is done.
pub(crate) fn admit(domain: &mut Domain, env: &Env<Config>, user: u64) -> Result<(), Error> {
    let config = &env.limits;
    if config.rate_limit > 0 {
        let fresh = Window { end: env.now.saturating_add(config.rate_window), calls: 1 };
        match domain.windows.get_mut(&user) {
            Some(window) if env.now < window.end => {
                if window.calls >= config.rate_limit {
                    domain.tally.limited = domain.tally.limited.saturating_add(1);
                    return Err(Error::RateLimited { reset: domain::time(config, window.end) });
                }
                window.calls = window.calls.saturating_add(1);
            }
            Some(window) => *window = fresh,
            None => {
                if domain.windows.len() >= domain.windows.capacity() {
                    reuse(domain, env)?;
                }
                domain.windows.insert(user, fresh).expect("checked for room above");
            }
        }
    }
    if domain.rng.chance(config.unavailable) {
        domain.tally.unavailable = domain.tally.unavailable.saturating_add(1);
        return Err(Error::Unavailable);
    }
    Ok(())
}

/// Makes room for a new caller's window by forgetting one that has ended,
/// the first in user order; or, when every window is running, refuses the
/// caller until the first of them ends, counted apart from the rate's
/// refusals.
fn reuse(domain: &mut Domain, env: &Env<Config>) -> Result<(), Error> {
    let mut ended = None;
    let mut first: Option<Time> = None;
    for (&user, window) in &domain.windows {
        if window.end <= env.now {
            ended = Some(user);
            break;
        }
        first = Some(match first {
            Some(end) => end.min(window.end),
            None => window.end,
        });
    }
    match ended {
        Some(user) => {
            domain.windows.remove(&user);
            Ok(())
        }
        None => {
            domain.tally.crowded = domain.tally.crowded.saturating_add(1);
            let first = first.expect("a full table has a window");
            Err(Error::RateLimited { reset: domain::time(&env.limits, first) })
        }
    }
}

/// Whether a call is to land late: answered as timed out, and made after.
pub(crate) fn lands_late(domain: &mut Domain, env: &Env<Config>) -> bool {
    domain.rng.chance(env.limits.landing)
}

/// What the caller hears of a call that was made: `result`, or, by chance, a
/// timeout.
pub(crate) fn finish(domain: &mut Domain, env: &Env<Config>, result: Result<Answer, Error>) -> Result<Answer, Error> {
    if domain.rng.chance(env.limits.timeouts) {
        domain.tally.timeouts = domain.tally.timeouts.saturating_add(1);
        return Err(Error::Timeout);
    }
    result
}

/// When a call's answer goes out: after its latency, or late.
pub(crate) fn latency(domain: &mut Domain, env: &Env<Config>) -> Time {
    let config = &env.limits;
    let span = if domain.rng.chance(config.late) {
        domain.tally.late = domain.tally.late.saturating_add(1);
        draw(domain, config.late_min, config.late_max)
    } else {
        draw(domain, config.latency_min, config.latency_max)
    };
    env.now.saturating_add(span)
}

/// A span drawn from `min..=max`.
pub(crate) fn draw(domain: &mut Domain, min: Duration, max: Duration) -> Duration {
    Duration::from_nanos(domain.rng.between(min.as_nanos(), max.as_nanos()))
}
