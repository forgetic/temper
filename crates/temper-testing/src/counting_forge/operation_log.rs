use std::collections::HashMap;
use std::sync::Mutex;

use temper_engine_io::{OneshotReceiver, OneshotSender};

use super::CountedForgeOp;

impl CountedForgeOp {
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::CreateRepository
                | Self::UpsertLabel
                | Self::CreateIssue
                | Self::UpdateIssue
                | Self::AddIssueDependency
                | Self::RemoveIssueDependency
                | Self::AddIssueComment
                | Self::CreatePullRequest
                | Self::UpdatePullRequest
                | Self::AddPullRequestDependency
                | Self::RemovePullRequestDependency
                | Self::RequestPullRequestReviewers
                | Self::SubmitPullRequestReview
                | Self::AddPullRequestComment
                | Self::MergePullRequest
                | Self::RetryCiAttempt
        )
    }
}

#[derive(Default)]
struct OperationLog {
    counts: HashMap<CountedForgeOp, usize>,
    trace: Vec<CountedForgeOp>,
    pause: Option<ArmedForgePause>,
}

struct ArmedForgePause {
    op: CountedForgeOp,
    occurrence: usize,
    reached: OneshotSender<()>,
    release: OneshotReceiver<()>,
}

/// One-shot permit for a `CountingForge` operation paused after its result has
/// been captured.
///
/// Await [`wait_until_paused`](Self::wait_until_paused) before changing the
/// wrapped fixture. Dropping or releasing the permit unblocks the Forge call.
pub struct ForgeOperationPause {
    reached: Option<OneshotReceiver<()>>,
    release: Option<OneshotSender<()>>,
}

impl ForgeOperationPause {
    pub async fn wait_until_paused(&mut self) {
        self.reached
            .take()
            .expect("Forge pause can only be awaited once")
            .recv()
            .await
            .expect("CountingForge was dropped before the selected operation paused");
    }

    pub fn release(mut self) {
        let _ = self
            .release
            .take()
            .expect("Forge pause can only be released once")
            .send(());
    }
}

#[derive(Default)]
pub(super) struct ForgeOperationLog {
    state: Mutex<OperationLog>,
}

impl ForgeOperationLog {
    pub(super) fn count(&self, op: CountedForgeOp) -> usize {
        *self
            .state
            .lock()
            .expect("operation log mutex")
            .counts
            .get(&op)
            .unwrap_or(&0)
    }

    pub(super) fn trace(&self) -> Vec<CountedForgeOp> {
        self.state
            .lock()
            .expect("operation log mutex")
            .trace
            .clone()
    }

    pub(super) fn read_count(&self) -> usize {
        self.state
            .lock()
            .expect("operation log mutex")
            .counts
            .iter()
            .filter(|(op, _)| !op.is_write())
            .map(|(_, count)| count)
            .sum()
    }

    pub(super) fn write_count(&self) -> usize {
        self.state
            .lock()
            .expect("operation log mutex")
            .counts
            .iter()
            .filter(|(op, _)| op.is_write())
            .map(|(_, count)| count)
            .sum()
    }

    pub(super) fn total_count(&self) -> usize {
        self.state.lock().expect("operation log mutex").trace.len()
    }

    pub(super) fn pause_after(&self, op: CountedForgeOp, occurrence: usize) -> ForgeOperationPause {
        assert!(
            occurrence > 0,
            "Forge operation occurrences are one-indexed"
        );
        let (reached, reached_rx) = temper_engine_io::oneshot();
        let (release, release_rx) = temper_engine_io::oneshot();
        let mut state = self.state.lock().expect("operation log mutex");
        assert!(
            state.pause.is_none(),
            "a Forge operation pause is already armed"
        );
        let completed = state.counts.get(&op).copied().unwrap_or(0);
        assert!(
            occurrence > completed,
            "cannot pause after completed {op:?} occurrence {occurrence} (current count: {completed})"
        );
        state.pause = Some(ArmedForgePause {
            op,
            occurrence,
            reached,
            release: release_rx,
        });
        ForgeOperationPause {
            reached: Some(reached_rx),
            release: Some(release),
        }
    }

    pub(super) fn tick(&self, op: CountedForgeOp) -> usize {
        let mut state = self.state.lock().expect("operation log mutex");
        state.trace.push(op);
        let count = state.counts.entry(op).or_insert(0);
        *count += 1;
        *count
    }

    pub(super) async fn pause_after_result(&self, op: CountedForgeOp, occurrence: usize) {
        let pause = {
            let mut state = self.state.lock().expect("operation log mutex");
            if state
                .pause
                .as_ref()
                .is_some_and(|pause| pause.op == op && pause.occurrence == occurrence)
            {
                state.pause.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            let _ = pause.reached.send(());
            let _ = pause.release.recv().await;
        }
    }
}
