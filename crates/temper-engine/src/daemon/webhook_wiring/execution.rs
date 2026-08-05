// SPDX-License-Identifier: MPL-2.0

//! Small execution and projection helpers for coordinated wake work.

use std::sync::Arc;

use temper_forge::{HintArtifactKind, RepositoryPath};
use temper_protocol_worker::Artifact;
use temper_runner::ArtifactAddress;

use super::super::CoordinatedMechanical;
use super::super::wake_coordinator::{WakeTargets, prioritized_targets};

pub(super) async fn execute_mechanical_work(
    mechanical: Option<&Arc<dyn CoordinatedMechanical>>,
    repo: &RepositoryPath,
    targets: &WakeTargets,
    broad: bool,
    failures: &mut Vec<String>,
) -> bool {
    let Some(mechanical) = mechanical else {
        return false;
    };
    let mut changed = false;

    for ((kind, number), change) in prioritized_targets(targets) {
        let address = ArtifactAddress::new(kind, number);
        match mechanical
            .run_coordinated_targeted(repo.clone(), address, change)
            .await
        {
            Ok(target_changed) => changed |= target_changed,
            Err(error) => failures.push(format!(
                "targeted mechanical wake failed for {}#{}: {error}",
                artifact_kind(address.kind),
                address.number
            )),
        }
    }

    if broad {
        match mechanical.run_coordinated_broad(repo.clone()).await {
            Ok(broad_changed) => changed |= broad_changed,
            Err(error) => failures.push(format!("broad mechanical wake failed: {error}")),
        }
    }
    changed
}

pub(super) fn protocol_artifact(address: ArtifactAddress) -> Artifact {
    Artifact {
        item: serde_json::json!(address.number.get()),
        kind: artifact_kind(address.kind).to_string(),
    }
}

pub(super) fn artifact_kind(kind: HintArtifactKind) -> &'static str {
    match kind {
        HintArtifactKind::Issue => "issue",
        HintArtifactKind::PullRequest => "pull_request",
    }
}

pub(super) fn repository_key(repo: &RepositoryPath) -> String {
    format!("{}/{}", repo.owner, repo.name)
}
