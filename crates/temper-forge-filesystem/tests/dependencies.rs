mod support;

use support::{TestRoot, block_on, issue, pull_request, repository};
use temper_forge_model::{Forge, ForgeError, IssueQuery, ItemListDetails, ItemNumber, PullRequestQuery};

#[test]
fn dependency_links_are_set_like_persisted_and_deterministic() {
    let root = TestRoot::new("dependencies");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let source = block_on(forge.create_issue(&repository.id, issue("Source"))).unwrap();
    let first_target = block_on(forge.create_issue(&repository.id, issue("First target"))).unwrap();
    let second_target =
        block_on(forge.create_issue(&repository.id, issue("Second target"))).unwrap();

    let with_second = block_on(forge.add_issue_dependency(&source.id, second_target.number))
        .expect("dependency added");
    assert_eq!(with_second.dependencies, vec![second_target.number]);

    let with_both = block_on(forge.add_issue_dependency(&source.id, first_target.number))
        .expect("dependency added and sorted");
    assert_eq!(
        with_both.dependencies,
        vec![first_target.number, second_target.number]
    );
    let duplicate = block_on(forge.add_issue_dependency(&source.id, first_target.number))
        .expect("duplicate add is a no-op");
    assert_eq!(duplicate.dependencies, with_both.dependencies);
    assert_eq!(duplicate.version, with_both.version);

    let removed = block_on(forge.remove_issue_dependency(&source.id, first_target.number))
        .expect("dependency removed");
    assert_eq!(removed.dependencies, vec![second_target.number]);
    let removed_again = block_on(forge.remove_issue_dependency(&source.id, first_target.number))
        .expect("duplicate remove is a no-op");
    assert_eq!(removed_again.dependencies, removed.dependencies);
    assert_eq!(removed_again.version, removed.version);

    let reopened = root.forge();
    let listed = block_on(reopened.list_issues(&repository.id, IssueQuery::default())).unwrap();
    assert_eq!(listed[0].dependencies, vec![second_target.number]);
    let summaries = block_on(reopened.list_issues(
        &repository.id,
        IssueQuery {
            details: ItemListDetails::summary(),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert!(summaries[0].dependencies.is_empty());

    let pr =
        block_on(forge.create_pull_request(&repository.id, pull_request(&repository.id, "PR")))
            .expect("pull request created");
    let pr_with_dependency =
        block_on(forge.add_pull_request_dependency(&pr.id, second_target.number))
            .expect("pull request dependency added");
    assert_eq!(pr_with_dependency.dependencies, vec![second_target.number]);
    let pr_summaries = block_on(reopened.list_pull_requests(
        &repository.id,
        PullRequestQuery {
            details: ItemListDetails::summary(),
            ..PullRequestQuery::default()
        },
    ))
    .unwrap();
    assert!(pr_summaries[0].dependencies.is_empty());
    let pr_without_dependency =
        block_on(forge.remove_pull_request_dependency(&pr.id, second_target.number))
            .expect("pull request dependency removed");
    assert!(pr_without_dependency.dependencies.is_empty());

    let missing =
        block_on(forge.add_issue_dependency(&source.id, ItemNumber::new(999))).unwrap_err();
    assert!(matches!(missing, ForgeError::NotFound(_)));
}
