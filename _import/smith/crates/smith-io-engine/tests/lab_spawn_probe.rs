//! Probe: can our shell pattern (Runtime::current_handle + handle.spawn) run
//! inside a LabRuntime task? If yes, the full machine+shell can be driven under
//! the lab with chaos; if not, the lab harness needs a different seam.
//!
//! This is exploratory — it asserts what works so the simulation harness builds
//! on solid ground rather than a guess.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use skein::lab::{LabConfig, LabRuntime};
use skein::types::Budget;

#[test]
fn current_handle_and_spawn_work_inside_a_lab_task() {
    let mut runtime = LabRuntime::new(LabConfig::new(1).with_auto_advance().max_steps(100_000));
    let region = runtime.state.create_root_region(Budget::INFINITE);

    let cx_resolved = Arc::new(AtomicU32::new(0));
    let handle_resolved = Arc::new(AtomicU32::new(0));
    let cx_resolved_in = Arc::clone(&cx_resolved);
    let handle_resolved_in = Arc::clone(&handle_resolved);

    let (task_id, _h) = runtime
        .state
        .create_task(region, Budget::INFINITE, async move {
            // The drive loop only needs the ambient Cx (for the clock).
            if skein::cx::Cx::current().is_some() {
                cx_resolved_in.fetch_add(1, Ordering::SeqCst);
            }
            // The shell needs the runtime handle (for spawning).
            if skein::runtime::Runtime::current_handle().is_some() {
                handle_resolved_in.fetch_add(1, Ordering::SeqCst);
            }
        })
        .expect("create lab task");
    runtime.scheduler.lock().schedule(task_id, 0);
    runtime.run_with_auto_advance();

    // Document the lab's capabilities for the harness design: the Cx (clock) is
    // available; the production Runtime handle (spawn) is NOT — which is exactly
    // why the shells need a Spawner seam to run under the lab.
    assert_eq!(
        cx_resolved.load(Ordering::SeqCst),
        1,
        "the ambient Cx (drive-loop clock) should resolve inside a lab task"
    );
    assert_eq!(
        handle_resolved.load(Ordering::SeqCst),
        0,
        "the production Runtime handle is absent inside a lab task (needs a Spawner seam)"
    );
}
