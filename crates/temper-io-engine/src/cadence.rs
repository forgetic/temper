// SPDX-License-Identifier: MPL-2.0

//! Fixed-delay cadence loops as machines: run a tick, wait the cadence, repeat.
//!
//! Replaces ad-hoc `loop { tick().await; sleep(cadence).await }` tasks: the
//! decision structure is the [`Machine`] below, the tick itself is a single
//! engine request executed by the shell.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::{drive, Executor};
use crate::machine::{EngineTime, Machine};
use crate::queue::{channel, CqSender};
use crate::spawn::Spawner;
use crate::timer::arm_timer;

pub enum CadenceCompletion {
    TimerFired,
    TickDone,
}

pub enum CadenceRequest {
    RunTick,
    StartTimer(Duration),
}

/// Pure cadence logic: tick immediately on start; after each completed tick
/// wait one cadence, then tick again. The delay is measured from tick
/// completion, matching the replaced sleep-after-tick loops.
pub struct CadenceMachine {
    cadence: Duration,
}

impl Machine for CadenceMachine {
    type Completion = CadenceCompletion;
    type Request = CadenceRequest;

    fn on_start(&mut self, _now: EngineTime) -> Vec<CadenceRequest> {
        vec![CadenceRequest::RunTick]
    }

    fn on_completion(
        &mut self,
        _now: EngineTime,
        completion: CadenceCompletion,
    ) -> Vec<CadenceRequest> {
        match completion {
            CadenceCompletion::TickDone => vec![CadenceRequest::StartTimer(self.cadence)],
            CadenceCompletion::TimerFired => vec![CadenceRequest::RunTick],
        }
    }
}

type BoxedTick = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

struct CadenceExecutor {
    spawner: Arc<dyn Spawner>,
    cq: CqSender<CadenceCompletion>,
    tick: BoxedTick,
}

impl Executor<CadenceMachine> for CadenceExecutor {
    fn execute(&self, request: CadenceRequest) {
        match request {
            CadenceRequest::RunTick => {
                let tick = Arc::clone(&self.tick);
                let cq = self.cq.clone();
                self.spawner.spawn_with_cx(move |_cx| async move {
                    tick().await;
                    let _ = cq.send(CadenceCompletion::TickDone);
                });
            }
            CadenceRequest::StartTimer(delay) => {
                arm_timer(&*self.spawner, &self.cq, delay, || {
                    CadenceCompletion::TimerFired
                });
            }
        }
    }
}

/// Spawn a cadence loop onto the runtime. The loop runs until the runtime
/// shuts down. `tick` is invoked once per cycle on an engine task.
pub fn spawn_cadence_loop<F, Fut>(spawner: &Arc<dyn Spawner>, cadence: Duration, tick: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (cq_tx, cq_rx) = channel();
    let executor = CadenceExecutor {
        spawner: Arc::clone(spawner),
        cq: cq_tx,
        tick: Arc::new(move || Box::pin(tick()) as Pin<Box<dyn Future<Output = ()> + Send>>),
    };
    spawner.spawn_with_cx(move |cx| async move {
        let _ = drive(cx, CadenceMachine { cadence }, &executor, cq_rx).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_machine_alternates_tick_and_timer() {
        let mut machine = CadenceMachine {
            cadence: Duration::from_secs(5),
        };
        let t = EngineTime::ZERO;
        assert!(matches!(machine.on_start(t)[..], [CadenceRequest::RunTick]));
        assert!(matches!(
            machine.on_completion(t, CadenceCompletion::TickDone)[..],
            [CadenceRequest::StartTimer(delay)] if delay == Duration::from_secs(5)
        ));
        assert!(matches!(
            machine.on_completion(t, CadenceCompletion::TimerFired)[..],
            [CadenceRequest::RunTick]
        ));
    }
}
