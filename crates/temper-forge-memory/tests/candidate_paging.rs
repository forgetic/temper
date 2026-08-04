use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, CandidatePageRequest, CreateIssue,
    CreateRepository, Forge, IssueCandidateQuery, IssueState, UpdateIssue,
};

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match Future::poll(future.as_mut(), &mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("memory forge future unexpectedly parked"),
    }
}

#[test]
fn terminal_candidate_pages_freeze_newer_concurrent_additions() {
    let forge = MemoryForge::new();
    let repo = block_on(forge.create_repository(CreateRepository {
        owner: "acme".into(),
        name: "paging".into(),
        default_branch: "main".into(),
        description: None,
    }))
    .unwrap()
    .id;
    let issue_input = || CreateIssue {
        title: "recovery".into(),
        body: String::new(),
        labels: vec!["recover".into()],
        assignees: Vec::new(),
    };
    let mut original = Vec::new();
    for _ in 0..3 {
        let issue = block_on(forge.create_issue(&repo, issue_input())).unwrap();
        original.push(
            block_on(forge.update_issue(
                &issue.id,
                UpdateIssue {
                    state: Some(IssueState::Closed),
                    ..UpdateIssue::default()
                },
            ))
            .unwrap()
            .id,
        );
    }

    let query = |page| IssueCandidateQuery {
        lifecycle: CandidateLifecycle::Terminal,
        labels: CandidateLabelSelection::AnyOf(vec!["recover".into()]),
        page: Some(page),
        ..IssueCandidateQuery::default()
    };
    let first = block_on(forge.list_issue_candidates(&repo, query(CandidatePageRequest::first(1))))
        .unwrap();
    assert_eq!((first.raw_count, first.returned_count), (3, 1));
    assert!(first.overflow);

    let concurrent = block_on(forge.create_issue(&repo, issue_input())).unwrap();
    block_on(forge.update_issue(
        &concurrent.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let second = block_on(forge.list_issue_candidates(
        &repo,
        query(CandidatePageRequest {
            limit: 1,
            continuation: first.continuation.clone(),
        }),
    ))
    .unwrap();
    let third = block_on(forge.list_issue_candidates(
        &repo,
        query(CandidatePageRequest {
            limit: 1,
            continuation: second.continuation.clone(),
        }),
    ))
    .unwrap();
    let swept = first
        .items
        .iter()
        .chain(&second.items)
        .chain(&third.items)
        .map(|issue| issue.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(swept, original);
    assert!(!swept.contains(&concurrent.id));
    assert!(third.exhausted && !third.overflow);
}
