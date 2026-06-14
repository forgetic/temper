//! Probe: the drive-loop's ambient capability inside a LabRuntime task.
//!
//! The drive loop needs only the ambient [`Cx`] (for the clock); this confirms
//! it resolves inside a lab task, so the full machine+shell can be driven under
//! the lab with chaos. Spawning is *not* ambient — there is no
//! `Runtime::current_handle` (skein removed it, and the agent no longer
//! reinstates a thread-local); production threads a `RuntimeHandle` explicitly
//! and the lab provides a `Spawner` seam. That explicit-capability design is
//! exactly what lets the same shells run under both runtimes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;

#[test]
fn ambient_cx_resolves_inside_a_lab_task() {
    let mut runtime = LabRuntime::new(LabConfig::new(1).with_auto_advance().max_steps(100_000));
    let region = runtime.state.create_root_region(Budget::INFINITE);

    let cx_resolved = Arc::new(AtomicU32::new(0));
    let cx_resolved_in = Arc::clone(&cx_resolved);

    let (task_id, _h) = runtime
        .state
        .create_task(region, Budget::INFINITE, async move {
            // The drive loop only needs the ambient Cx (for the clock).
            if skein::cx::Cx::current().is_some() {
                cx_resolved_in.fetch_add(1, Ordering::SeqCst);
            }
        })
        .expect("create lab task");
    runtime.scheduler.lock().schedule(task_id, 0);
    runtime.run_with_auto_advance();

    // The Cx (clock) is available inside a lab task. Spawning is supplied
    // explicitly via a Spawner seam, never an ambient handle — so the same
    // shells run under the production runtime and the lab unchanged.
    assert_eq!(
        cx_resolved.load(Ordering::SeqCst),
        1,
        "the ambient Cx (drive-loop clock) should resolve inside a lab task"
    );
}
