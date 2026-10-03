//! Random settings and clients: small limits, slow and failing git, a forge
//! that is unreachable, refuses or lacks what is asked, branches others
//! advance, and clients that abort, release, save and push twice.

use temper_lib::{Duration, Rng, Time};
use temper_worker_model_checkout::Limits;

use crate::client::{Interrupt, Plan};
use crate::{LIMITS, Settings, Span, World};

/// Settings drawn from `seed`: small limits, faults, and latencies that race
/// the deadlines.
#[must_use]
pub fn noisy(seed: u64) -> Settings {
    let mut rng = Rng::new(seed);
    let calm = Settings::calm(seed);
    let mut millis = |low: u64, high: u64| Duration::from_millis(rng.between(low, high));
    let remote_timeout = millis(300, 3_000);
    let local_timeout = millis(50, 600);
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut pick = |low: u64, high: u64| u32::try_from(rng.between(low, high)).expect("small numbers");
    Settings {
        checkout: Limits {
            workspaces: pick(1, 3),
            repositories: pick(1, 3),
            remote_timeout,
            local_timeout,
            facts: pick(0, 32),
            ..LIMITS
        },
        repositories: pick(2, 5),
        workstreams: pick(1, 6),
        local: Span::millis(1, 300),
        remote: Span::millis(10, 2_000),
        think: Span::millis(0, 3_000),
        network: Span::millis(1, 50),
        broken: pick(0, 40),
        ambiguous: pick(0, 60),
        leftovers: pick(0, 1000),
        unreachable: pick(0, 60),
        refusing: pick(0, 60),
        missing: pick(0, 60),
        advance: pick(0, 400),
        reshape: pick(0, 150),
        edit: pick(300, 1000),
        cancels_lost: pick(0, 500),
        granule: if pick(0, 1) == 1 { Duration::from_millis(100) } else { Duration::ZERO },
        ..calm
    }
}

/// Clients drawn from `seed`: a few, arriving within a minute, on the
/// workstreams of `settings`, pushing, saving, interrupting and erring.
pub fn submit_noisily(world: &mut World, settings: &Settings, seed: u64) {
    let mut rng = Rng::new(seed.wrapping_add(2));
    for _ in 0..rng.between(3, 12) {
        let at = Time::ZERO.saturating_add(Duration::from_millis(rng.between(0, 60_000)));
        let interrupt = if rng.chance(300) {
            let how = if rng.chance(500) { Interrupt::Abort } else { Interrupt::Release };
            Some((Duration::from_millis(rng.between(0, 6_000)), how))
        } else {
            None
        };
        let plan = Plan {
            workstream: u32::try_from(rng.below(u64::from(settings.workstreams))).expect("small"),
            start: None,
            pushes: u32::try_from(rng.between(0, 3)).expect("small"),
            save: rng.chance(400),
            interrupt,
            invalid: rng.chance(30),
            twice: rng.chance(80),
        };
        world.submit(at, plan);
    }
}
