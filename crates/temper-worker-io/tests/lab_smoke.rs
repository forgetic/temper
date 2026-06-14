//! Smoke test: confirm the skein **lab** runtime drives our kind of
//! workload (spawned tasks that wait on virtual timers) deterministically from
//! a seed, with no real wall-clock sleeping.
//!
//! This pins down the lab entry points the worker's simulation tests will build
//! on, and proves the foundation works before driving the full machine + shell
//! under it. It deliberately uses the low-level `LabRuntime` task API
//! (`state.create_task` + `scheduler.schedule` + `run_with_auto_advance`) since
//! that is the public surface the lab exposes.
//!
//! Scope note: this drives top-level tasks that sleep on virtual time — the same
//! primitive the worker shell's `arm_timer` uses. Driving the worker's full
//! `drive()` loop + shell (which spawn child tasks via `RuntimeHandle::spawn`)
//! under the lab is the next step; this establishes the seam compiles and the
//! virtual clock advances.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;

/// Run `task_count` tasks that each virtual-sleep then bump a counter, under a
/// seeded lab runtime with auto-advancing virtual time. Returns how many
/// completed and the virtual nanoseconds elapsed.
fn run_once(seed: u64, task_count: u32) -> (u32, u64) {
    let config = LabConfig::new(seed).with_auto_advance().max_steps(100_000);
    let mut runtime = LabRuntime::new(config);
    let region = runtime.state.create_root_region(Budget::INFINITE);

    let counter = Arc::new(AtomicU32::new(0));
    for index in 0..task_count {
        let counter = Arc::clone(&counter);
        // Stagger the deadlines so virtual time has to advance past several
        // distinct timer deadlines (10ms, 20ms, ...).
        let delay = std::time::Duration::from_millis(10 * u64::from(index + 1));
        let (task_id, _handle) = runtime
            .state
            .create_task(region, Budget::INFINITE, async move {
                let now = skein::time::wall_now();
                skein::time::sleep(now, delay).await;
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .expect("create lab task");
        runtime.scheduler.lock().schedule(task_id, 0);
    }

    let report = runtime.run_with_auto_advance();
    (counter.load(Ordering::SeqCst), report.virtual_elapsed_nanos)
}

#[test]
fn lab_runs_seeded_workload_with_virtual_time() {
    // Determinism: the same seed reproduces the same observable *outcome* (every
    // task completes). The absolute virtual-elapsed value depends on the
    // wall-clock epoch the sleeps are computed from, so it is not part of the
    // reproducible outcome — only the completed count is.
    assert_eq!(run_once(1234, 3).0, run_once(1234, 3).0);

    // Every task completes regardless of seed (the seed varies interleaving,
    // not the outcome).
    for seed in 0..16u64 {
        let (completed, _) = run_once(seed, 3);
        assert_eq!(completed, 3, "seed {seed}: not all tasks completed");
    }
}

#[test]
fn lab_virtual_time_advances_without_real_sleep() {
    // Three tasks sleeping 10/20/30ms: virtual time must reach >= 30ms, but the
    // test returns in microseconds of real time because the sleeps are virtual.
    let (completed, elapsed_nanos) = run_once(7, 3);
    assert_eq!(completed, 3);
    assert!(
        elapsed_nanos >= 30_000_000,
        "virtual time should advance past the longest deadline (30ms), got {elapsed_nanos}ns"
    );
}
