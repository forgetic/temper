use super::{ADMIN_USER, LiveWorld, REPO};
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CandidatePageRequest, CreateIssue,
    IssueCandidateQuery, IssueState, UpdateIssue, UpsertLabel,
};

pub(super) async fn candidate_label_filter_is_any_of(world: &LiveWorld) {
    for name in ["candidate-ready", "candidate-queued"] {
        world
            .forge
            .upsert_label(
                &world.repo_id,
                UpsertLabel {
                    name: name.to_string(),
                    color: Some("1d76db".to_string()),
                    description: None,
                },
            )
            .await
            .expect("candidate label should be created");
    }
    let created = world
        .forge
        .create_issue(
            &world.repo_id,
            CreateIssue {
                title: "Forgejo any-label candidate".to_string(),
                body: "Carries only one of the requested candidate labels.".to_string(),
                labels: vec!["candidate-ready".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("candidate issue should be created");

    let candidates = world
        .forge
        .list_issue_candidates(
            &world.repo_id,
            IssueCandidateQuery {
                lifecycle: CandidateLifecycle::Open,
                labels: CandidateLabelSelection::AnyOf(vec![
                    "candidate-ready".to_string(),
                    "candidate-queued".to_string(),
                ]),
                ..IssueCandidateQuery::default()
            },
        )
        .await
        .expect("candidate issue list should succeed");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.number)
            .collect::<Vec<_>>(),
        vec![created.number],
        "Forgejo candidate discovery must retain any-label semantics"
    );
}

pub(super) async fn bounded_candidate_contract_matches_live_forgejo(world: &LiveWorld) {
    for number in 0..3 {
        let issue = world
            .forge
            .create_issue(
                &world.repo_id,
                CreateIssue {
                    title: format!("bounded candidate {number}"),
                    body: "live ordering probe".to_string(),
                    labels: vec!["candidate-ready".to_string()],
                    assignees: Vec::new(),
                },
            )
            .await
            .expect("bounded probe issue creates");
        world
            .forge
            .update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            )
            .await
            .expect("bounded probe issue closes");
    }

    world.recorder.clear();
    let query = |continuation| IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["candidate-ready".to_string()]),
        page: Some(CandidatePageRequest {
            limit: 1,
            continuation,
        }),
        ..IssueCandidateQuery::default()
    };
    let first = world
        .forge
        .list_issue_candidates(&world.repo_id, query(None))
        .await
        .expect("first bounded live page");
    assert_eq!(first.len(), 1);
    assert!(first.overflow);
    let first_number = first[0].number;
    let second = world
        .forge
        .list_issue_candidates(&world.repo_id, query(first.continuation))
        .await
        .expect("continued bounded live page");
    assert_eq!(second.len(), 1);
    assert!(
        second[0].number > first_number,
        "equal timestamps need stable tie movement"
    );

    let requests = world.recorder.recorded();
    assert_eq!(requests.len(), 2, "one exact-repository request per page");
    assert!(requests.iter().all(|request| {
        request.path == format!("/api/v1/repos/{ADMIN_USER}/{REPO}/issues")
            && request
                .query
                .contains(&("sort".to_string(), "updated".to_string()))
            && request
                .query
                .contains(&("direction".to_string(), "asc".to_string()))
            && request.query.iter().any(|(key, _)| key == "before")
    }));
    assert!(requests[1].query.iter().any(|(key, _)| key == "since"));
}
