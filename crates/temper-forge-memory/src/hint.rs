//! Optional in-process change hints for [`MemoryForge`](crate::MemoryForge).

use crate::Inner;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::Duration;
use temper_forge_model::{
    ChangeHint, ChangeKind, ChangeSource, ChangeSourceEvent, HintArtifactKind, ItemNumber,
    PullRequest, RepositoryId, RepositoryPath,
};

/// In-process hint receiver returned by [`MemoryForge::subscribe_hints`].
pub struct MemoryHintReceiver {
    receiver: Receiver<ChangeHint>,
}

impl MemoryHintReceiver {
    pub(crate) fn new(receiver: Receiver<ChangeHint>) -> Self {
        Self { receiver }
    }
}

impl ChangeSource for MemoryHintReceiver {
    fn recv_timeout(&mut self, timeout: Duration) -> ChangeSourceEvent {
        match self.receiver.recv_timeout(timeout) {
            Ok(hint) => ChangeSourceEvent::Hint(hint),
            Err(RecvTimeoutError::Timeout) => ChangeSourceEvent::Timeout,
            Err(RecvTimeoutError::Disconnected) => ChangeSourceEvent::Closed,
        }
    }

    fn try_recv(&mut self) -> ChangeSourceEvent {
        match self.receiver.try_recv() {
            Ok(hint) => ChangeSourceEvent::Hint(hint),
            Err(TryRecvError::Empty) => ChangeSourceEvent::Timeout,
            Err(TryRecvError::Disconnected) => ChangeSourceEvent::Closed,
        }
    }
}

#[derive(Default)]
pub(crate) struct HintBus {
    senders: Vec<Sender<ChangeHint>>,
}

impl HintBus {
    pub(crate) fn subscribe(&mut self) -> MemoryHintReceiver {
        let (sender, receiver) = mpsc::channel();
        self.senders.push(sender);
        MemoryHintReceiver::new(receiver)
    }

    fn publish(&mut self, hint: ChangeHint) {
        self.senders
            .retain(|sender| sender.send(hint.clone()).is_ok());
    }
}

impl Inner {
    pub(crate) fn subscribe_hints(&mut self) -> MemoryHintReceiver {
        self.hints.subscribe()
    }

    pub(crate) fn publish_repo_hint(&mut self, repo_id: &RepositoryId, change: ChangeKind) {
        if let Some(repo) = self.state.find_repository_by_id(repo_id) {
            self.hints
                .publish(ChangeHint::repository(repo_path(&repo), change));
        }
    }

    pub(crate) fn publish_path_hint(&mut self, path: RepositoryPath, change: ChangeKind) {
        self.hints.publish(ChangeHint::repository(path, change));
    }

    fn publish_artifact_hint(
        &mut self,
        repo_id: &RepositoryId,
        kind: HintArtifactKind,
        number: ItemNumber,
        change: ChangeKind,
    ) {
        if let Some(repo) = self.state.find_repository_by_id(repo_id) {
            self.hints
                .publish(ChangeHint::artifact(repo_path(&repo), kind, number, change));
        }
    }

    pub(crate) fn publish_issue_hint(
        &mut self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        change: ChangeKind,
    ) {
        self.publish_artifact_hint(repo_id, HintArtifactKind::Issue, number, change);
    }

    pub(crate) fn publish_pull_request_number_hint(
        &mut self,
        repo_id: &RepositoryId,
        number: ItemNumber,
        change: ChangeKind,
    ) {
        self.publish_artifact_hint(repo_id, HintArtifactKind::PullRequest, number, change);
    }

    pub(crate) fn publish_pull_request_hint(&mut self, pr: &PullRequest, change: ChangeKind) {
        self.publish_pull_request_number_hint(&pr.repo_id, pr.number, change);
    }
}

fn repo_path(repo: &temper_forge_model::Repository) -> RepositoryPath {
    RepositoryPath::new(repo.owner.clone(), repo.name.clone())
}
