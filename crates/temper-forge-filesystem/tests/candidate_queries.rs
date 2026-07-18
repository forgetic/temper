mod support;

use support::{TestRoot, block_on, issue_with, pull_request_with, repository};
use temper_forge_model::{
    CandidateLabelSelection, CandidateLifecycle, Forge, IssueCandidateQuery, IssueQuery,
    ItemListDetails, MergeMethod, MergePullRequest, PullRequestCandidateQuery,
    PullRequestUpdateState, UpdatePullRequest,
};

#[test]
fn filesystem_candidate_reads_match_any_label_without_changing_legacy_conjunction() {
    let root = TestRoot::new("candidate-any-label");
    let forge = root.forge();
    let repo = block_on(forge.create_repository(repository("acme", "service")))
        .unwrap()
        .id;
    let both = block_on(forge.create_issue(&repo, issue_with("both", "", &["code", "ready"], &[])))
        .unwrap();
    let code = block_on(forge.create_issue(&repo, issue_with("code", "", &["code"], &[]))).unwrap();

    let legacy = block_on(forge.list_issues(
        &repo,
        IssueQuery {
            labels: vec!["code".into(), "ready".into()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].id, both.id);

    let candidates = block_on(forge.list_issue_candidates(
        &repo,
        IssueCandidateQuery {
            labels: CandidateLabelSelection::AnyOf(vec![
                "ready".into(),
                "code".into(),
                "ready".into(),
            ]),
            ..IssueCandidateQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>(),
        vec![both.id, code.id]
    );
}

#[test]
fn filesystem_terminal_pull_bucket_and_exact_summary_are_portable() {
    let root = TestRoot::new("candidate-terminal-pulls");
    let forge = root.forge();
    let repo = block_on(forge.create_repository(repository("acme", "service")))
        .unwrap()
        .id;
    let closed = block_on(forge.create_pull_request(
        &repo,
        pull_request_with(&repo, "closed", "", &["landed"], &[]),
    ))
    .unwrap();
    let merged = block_on(forge.create_pull_request(
        &repo,
        pull_request_with(&repo, "merged", "", &["landed"], &[]),
    ))
    .unwrap();
    block_on(forge.update_pull_request(
        &closed.id,
        UpdatePullRequest {
            state: Some(PullRequestUpdateState::Closed),
            ..UpdatePullRequest::default()
        },
    ))
    .unwrap();
    block_on(forge.merge_pull_request(
        &merged.id,
        MergePullRequest {
            method: MergeMethod::Squash,
            commit_title: None,
            commit_body: None,
            delete_source_branch: false,
        },
    ))
    .unwrap();

    let candidates = block_on(forge.list_pull_request_candidates(
        &repo,
        PullRequestCandidateQuery {
            lifecycle: CandidateLifecycle::Terminal,
            labels: CandidateLabelSelection::AnyOf(vec!["landed".into()]),
            ..PullRequestCandidateQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].id, closed.id);
    assert_eq!(candidates[1].id, merged.id);

    let summary = block_on(forge.get_pull_request_by_number_with_details(
        &repo,
        merged.number,
        ItemListDetails::summary(),
    ))
    .unwrap()
    .unwrap();
    assert!(summary.dependencies.is_empty());
}
