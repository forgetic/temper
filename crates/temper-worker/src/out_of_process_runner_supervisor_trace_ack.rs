use temper_protocol_activity::{AgentActivityEventV1, RunFinishedV1, RunStatusV1};

use crate::trace::TraceCollector;

use super::*;

pub(super) fn finish_cancelled_after_delayed_ack<F: Future>(
    mut future: std::pin::Pin<&mut F>,
    config: &WorkerAgentTraceConfig,
) -> F::Output {
    let collector = TraceCollector::new(config.clone());
    let deadline = Instant::now() + Duration::from_secs(2);
    let (run_id, terminal_sequence) = loop {
        let mut task_context = Context::from_waker(Waker::noop());
        assert!(
            matches!(future.as_mut().poll(&mut task_context), Poll::Pending),
            "cancelled runner completed before terminal acknowledgement"
        );
        let runs = collector.recover().expect("recover pending trace");
        if let Some((run_id, sequence)) = runs.first().and_then(|run| {
            run.events.last().and_then(|terminal| {
                matches!(
                    &terminal.event,
                    AgentActivityEventV1::RunFinished(RunFinishedV1 {
                        status: RunStatusV1::Cancelled,
                        ..
                    })
                )
                .then(|| (run.manifest.run_id.clone(), terminal.seq))
            })
        }) {
            break (run_id, sequence);
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for durable cancelled terminal"
        );
        std::thread::sleep(Duration::from_millis(1));
    };

    std::thread::sleep(Duration::from_millis(300));
    let mut task_context = Context::from_waker(Waker::noop());
    assert!(
        matches!(future.as_mut().poll(&mut task_context), Poll::Pending),
        "the historical 250 ms timeout must not prove out-of-process quiescence"
    );
    collector
        .acknowledge(&run_id, terminal_sequence)
        .expect("acknowledge out-of-process cancellation terminal");
    super::poll_until_ready(future)
}

pub(super) fn assert_cancelled_terminal(config: &WorkerAgentTraceConfig) {
    let recovered = TraceCollector::new(config.clone()).recover().unwrap();
    assert_eq!(recovered.len(), 1);
    assert!(
        recovered[0].acknowledged_seq > 0,
        "terminal sequence must be durably acknowledged"
    );
    if let Some(terminal) = recovered[0].events.last() {
        assert!(matches!(
            &terminal.event,
            AgentActivityEventV1::RunFinished(RunFinishedV1 {
                status: RunStatusV1::Cancelled,
                ..
            })
        ));
        assert!(recovered[0].acknowledged_seq >= terminal.seq);
    }
}
