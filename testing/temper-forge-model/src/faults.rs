//! Faults, drawn from the seed per call (testing-pyramid.md, 4): a rate limit
//! per user per window, failures before a call is made and after, and the
//! latency of its answer, sometimes late.

use temper_lib::{Duration, Env, Time};

use crate::api::{Answer, Error};
use crate::model::{self, Config, Model};

/// A user's rate window: when it ends, and the calls taken in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Window {
    end: Time,
    calls: u32,
}

/// Takes a call from `user`, or refuses it: for the user's rate, or as
/// unavailable by chance, before anything is done.
pub(crate) fn admit(model: &mut Model, env: &Env<Config>, user: u64) -> Result<(), Error> {
    let config = &env.limits;
    if config.rate_limit > 0 {
        let fresh = Window { end: env.now.saturating_add(config.rate_window), calls: 1 };
        match model.windows.get_mut(&user) {
            Some(window) if env.now < window.end => {
                if window.calls >= config.rate_limit {
                    model.tally.limited = model.tally.limited.saturating_add(1);
                    return Err(Error::RateLimited { reset: model::time(config, window.end) });
                }
                window.calls = window.calls.saturating_add(1);
            }
            Some(window) => *window = fresh,
            None => {
                if model.windows.len() >= model.windows.capacity() {
                    reuse(model, env)?;
                }
                model.windows.insert(user, fresh).expect("checked for room above");
            }
        }
    }
    if model.rng.chance(config.unavailable) {
        model.tally.unavailable = model.tally.unavailable.saturating_add(1);
        return Err(Error::Unavailable);
    }
    Ok(())
}

/// Makes room for a new caller's window by forgetting one that has ended,
/// the first in user order; or, when every window is running, refuses the
/// caller until the first of them ends, counted apart from the rate's
/// refusals.
fn reuse(model: &mut Model, env: &Env<Config>) -> Result<(), Error> {
    let mut ended = None;
    let mut first: Option<Time> = None;
    for (&user, window) in &model.windows {
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
            model.windows.remove(&user);
            Ok(())
        }
        None => {
            model.tally.crowded = model.tally.crowded.saturating_add(1);
            let first = first.expect("a full table has a window");
            Err(Error::RateLimited { reset: model::time(&env.limits, first) })
        }
    }
}

/// Whether a call is to land late: answered as timed out, and made after.
pub(crate) fn lands_late(model: &mut Model, env: &Env<Config>) -> bool {
    model.rng.chance(env.limits.landing)
}

/// What the caller hears of a call that was made: `result`, or, by chance, a
/// timeout.
pub(crate) fn finish(model: &mut Model, env: &Env<Config>, result: Result<Answer, Error>) -> Result<Answer, Error> {
    if model.rng.chance(env.limits.timeouts) {
        model.tally.timeouts = model.tally.timeouts.saturating_add(1);
        return Err(Error::Timeout);
    }
    result
}

/// When a call's answer goes out: after its latency, or late.
pub(crate) fn latency(model: &mut Model, env: &Env<Config>) -> Time {
    let config = &env.limits;
    let span = if model.rng.chance(config.late) {
        model.tally.late = model.tally.late.saturating_add(1);
        draw(model, config.late_min, config.late_max)
    } else {
        draw(model, config.latency_min, config.latency_max)
    };
    env.now.saturating_add(span)
}

/// A span drawn from `min..=max`.
pub(crate) fn draw(model: &mut Model, min: Duration, max: Duration) -> Duration {
    Duration::from_nanos(model.rng.between(min.as_nanos(), max.as_nanos()))
}
