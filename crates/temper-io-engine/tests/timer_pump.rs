// SPDX-License-Identifier: MPL-2.0

//! Guard for the vendored timer-pump patch: a bare sleep on an otherwise idle
//! production runtime must fire (upstream asupersync 0.3.1 never pumped the
//! timer wheel outside the lab runtime).

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
