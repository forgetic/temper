// SPDX-License-Identifier: MPL-2.0

//! Guard for the timer lost-wakeup bug: a bare sleep on an otherwise idle
//! production runtime must fire. asupersync 0.3.1 hung here (the I/O leader
//! blocked the reactor with no timeout; we carried a vendored patch), fixed
//! upstream in 0.3.2 by folding the timer deadline into the leader's poll
//! timeout and capping the idle poll at 250ms. Keep this test when bumping
//! asupersync — it is the cheapest canary for scheduler regressions.

use std::time::{Duration, Instant};

#[test]
fn bare_sleep_fires_on_idle_runtime() {
    let started = Instant::now();
    temper_io_engine::block_on(async {
        temper_io_engine::runtime::sleep_for(Duration::from_millis(300)).await;
    });
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(250),
        "sleep returned too early: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(30),
        "sleep took far too long: {elapsed:?}"
    );
}
