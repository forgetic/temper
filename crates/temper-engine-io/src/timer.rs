// SPDX-License-Identifier: MPL-2.0

//! Timer requests: sleep on the runtime, complete into the queue.

use std::time::Duration;

use skein::time::sleep;

use crate::queue::CqSender;
use crate::spawn::Spawner;

/// Arm a one-shot timer. After `delay`, `make_completion` is invoked (this is
/// where the shell may stamp "now") and the result is submitted to the
/// completion queue. The machine sees time purely as data.
pub fn arm_timer<C, F>(spawner: &dyn Spawner, cq: &CqSender<C>, delay: Duration, make_completion: F)
where
    C: Send + 'static,
    F: FnOnce() -> C + Send + 'static,
{
    let cq = cq.clone();
    spawner.spawn_with_cx(move |cx| async move {
        sleep(crate::runtime::timer_now(&cx), delay).await;
        let _ = cq.send(make_completion());
    });
}
