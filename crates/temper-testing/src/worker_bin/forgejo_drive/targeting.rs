use temper_forge_model::{
    ChangeHint, ChangeKind, Forge, HintArtifactKind, HintTarget, ItemNumber, RepositoryPath,
};
use temper_runner::{ArtifactAddress, MechanicalWorker, RepositorySet, WorkerError};
use temper_workflow::{CommandJournal, RecoveryPolicy};

const MECHANICAL_TARGETED_WAKE_CAP: usize = 32;

type ArtifactWake = (ItemNumber, HintArtifactKind, ChangeKind);
type RepositoryArtifactWake = (RepositoryPath, ArtifactAddress, ChangeKind);

pub(super) async fn targeted_single_repo_hints<F, J, P>(
    worker: &MechanicalWorker<'_, F, J, P>,
    hints: &[ChangeHint],
) -> Result<Option<Vec<ArtifactWake>>, WorkerError>
where
    F: Forge + ?Sized,
    J: CommandJournal,
    P: RecoveryPolicy + Send + Sync,
{
    let repo_path = worker.repository_path().await?;
    let targets = targeted_hints_for_path(Some(&repo_path), hints).map(|items| {
        items
            .into_iter()
            .map(|(_, artifact, change)| (artifact.number, artifact.kind, change))
            .collect()
    });
    Ok(targets)
}

pub(super) fn targeted_multi_repo_hints(
    repositories: &RepositorySet,
    hints: &[ChangeHint],
) -> Option<Vec<RepositoryArtifactWake>> {
    targeted_hints_for_path(None, hints).and_then(|targets| {
        if targets.iter().all(|(path, _, _)| {
            !repositories
                .matching_hints(&[ChangeHint::repository(path.clone(), ChangeKind::Unknown)])
                .is_empty()
        }) {
            Some(targets)
        } else {
            None
        }
    })
}

fn targeted_hints_for_path(
    single_repo: Option<&RepositoryPath>,
    hints: &[ChangeHint],
) -> Option<Vec<RepositoryArtifactWake>> {
    let mut targets = Vec::new();
    for hint in hints {
        if matches!(hint.change, ChangeKind::Push | ChangeKind::Unknown) {
            return None;
        }
        if single_repo
            .is_some_and(|path| path.owner != hint.repo.owner || path.name != hint.repo.name)
        {
            return None;
        }
        let HintTarget::Artifact { kind, number } = hint.target else {
            return None;
        };
        targets.push((
            hint.repo.clone(),
            ArtifactAddress::new(kind, number),
            hint.change,
        ));
    }
    targets.sort_by(|left, right| {
        (&left.0.owner, &left.0.name, left.1, left.2).cmp(&(
            &right.0.owner,
            &right.0.name,
            right.1,
            right.2,
        ))
    });
    targets.dedup();
    if targets.len() > MECHANICAL_TARGETED_WAKE_CAP {
        None
    } else {
        Some(targets)
    }
}

pub(super) fn known_hints_for(
    repositories: &RepositorySet,
    hints: &[ChangeHint],
) -> Vec<ChangeHint> {
    let mut known = Vec::new();
    for hint in hints {
        if repositories
            .matching_hints(std::slice::from_ref(hint))
            .is_empty()
        {
            tracing::warn!(
                target: "temper_testing_worker",
                repo_owner = %hint.repo.owner,
                repo_name = %hint.repo.name,
                "wake hint for unconfigured repo; treating wake as broad scan if no configured hints remain"
            );
        } else {
            known.push(hint.clone());
        }
    }
    known
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_hint(n: u64, kind: HintArtifactKind, change: ChangeKind) -> ChangeHint {
        ChangeHint::artifact(
            RepositoryPath::new("ai", "temper"),
            kind,
            ItemNumber::new(n),
            change,
        )
    }

    #[test]
    fn routable_item_hints_return_typed_targets() {
        let hints = [
            item_hint(7, HintArtifactKind::PullRequest, ChangeKind::Ci),
            item_hint(7, HintArtifactKind::PullRequest, ChangeKind::Ci),
        ];

        let out = targeted_hints_for_path(None, &hints).expect("routable hints target items");

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, RepositoryPath::new("ai", "temper"));
        assert_eq!(out[0].1.number, ItemNumber::new(7));
        assert_eq!(out[0].1.kind, HintArtifactKind::PullRequest);
        assert_eq!(out[0].2, ChangeKind::Ci);
    }

    #[test]
    fn review_and_comment_cannot_lose_pull_request_kind() {
        let hints = [
            item_hint(7, HintArtifactKind::PullRequest, ChangeKind::Review),
            item_hint(8, HintArtifactKind::PullRequest, ChangeKind::Comment),
        ];
        let out = targeted_hints_for_path(None, &hints).unwrap();
        assert!(
            out.iter()
                .all(|target| target.1.kind == HintArtifactKind::PullRequest)
        );
    }

    #[test]
    fn repository_target_falls_back() {
        let hints = [ChangeHint::repository(
            RepositoryPath::new("ai", "temper"),
            ChangeKind::Ci,
        )];

        assert!(targeted_hints_for_path(None, &hints).is_none());
    }

    #[test]
    fn unroutable_change_falls_back() {
        assert!(
            targeted_hints_for_path(
                None,
                &[item_hint(7, HintArtifactKind::Issue, ChangeKind::Push)]
            )
            .is_none()
        );
        assert!(
            targeted_hints_for_path(
                None,
                &[item_hint(7, HintArtifactKind::Issue, ChangeKind::Unknown)]
            )
            .is_none()
        );
    }

    #[test]
    fn wrong_single_repo_falls_back() {
        let other = RepositoryPath::new("ai", "other");

        assert!(
            targeted_hints_for_path(
                Some(&other),
                &[item_hint(7, HintArtifactKind::PullRequest, ChangeKind::Ci)]
            )
            .is_none()
        );
    }

    #[test]
    fn over_cap_falls_back_but_exact_cap_targets() {
        let exactly_cap: Vec<ChangeHint> = (1..=MECHANICAL_TARGETED_WAKE_CAP as u64)
            .map(|n| item_hint(n, HintArtifactKind::PullRequest, ChangeKind::Ci))
            .collect();
        assert!(targeted_hints_for_path(None, &exactly_cap).is_some());

        let over_cap: Vec<ChangeHint> = (1..=(MECHANICAL_TARGETED_WAKE_CAP as u64 + 1))
            .map(|n| item_hint(n, HintArtifactKind::PullRequest, ChangeKind::Ci))
            .collect();
        assert!(targeted_hints_for_path(None, &over_cap).is_none());
    }
}
