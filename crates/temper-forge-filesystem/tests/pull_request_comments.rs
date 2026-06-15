mod support;

use support::{TestRoot, block_on, comment, pull_request, repository};
use temper_forge_model::{Comment, Forge, ForgeError, PullRequestId, UserId};

fn comment_bodies(comments: &[Comment]) -> Vec<String> {
    comments
        .iter()
        .map(|comment| comment.body.clone())
        .collect()
}

#[test]
fn pull_request_comments_are_empty_for_pull_request_with_no_comments() {
    let root = TestRoot::new("pull-request-comments");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Needs discussion"),
    ))
    .unwrap();

    assert_eq!(
        block_on(forge.list_pull_request_comments(&pull_request.id)).unwrap(),
        Vec::new()
    );
}

#[test]
fn pull_request_comments_can_be_added_with_deterministic_identity_author_and_timestamps() {
    let root = TestRoot::new("pull-request-comments");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Needs discussion"),
    ))
    .unwrap();
    let unchanged_pull_request = pull_request.clone();

    let first =
        block_on(forge.add_pull_request_comment(&pull_request.id, comment("First comment")))
            .unwrap();
    let second =
        block_on(forge.add_pull_request_comment(&pull_request.id, comment("Second comment")))
            .unwrap();

    assert_eq!(
        first.id.as_str(),
        "comment-pull-request-repo-0000000000000001-0000000000000001-0000000000000001"
    );
    assert_eq!(
        second.id.as_str(),
        "comment-pull-request-repo-0000000000000001-0000000000000001-0000000000000002"
    );
    assert_eq!(first.author_id, UserId::new("user-1"));
    assert_eq!(second.author_id, UserId::new("user-1"));
    assert_eq!(first.body, "First comment");
    assert_eq!(second.body, "Second comment");
    assert_eq!(first.created_at, first.updated_at);
    assert_eq!(second.created_at, second.updated_at);
    assert_eq!(first.created_at.timestamp(), 3);
    assert_eq!(second.created_at.timestamp(), 4);
    assert!(first.created_at < second.created_at);

    let comments = block_on(forge.list_pull_request_comments(&pull_request.id)).unwrap();
    assert_eq!(comments, vec![first, second]);
    assert_eq!(
        block_on(forge.get_pull_request(&pull_request.id)).unwrap(),
        Some(unchanged_pull_request)
    );
}

#[test]
fn pull_request_comments_are_persisted_and_reopened() {
    let root = TestRoot::new("pull-request-comments");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(forge.create_pull_request(
        &repository.id,
        pull_request(&repository.id, "Needs discussion"),
    ))
    .unwrap();
    let first =
        block_on(forge.add_pull_request_comment(&pull_request.id, comment("Persist me"))).unwrap();
    let second =
        block_on(forge.add_pull_request_comment(&pull_request.id, comment("Persist me too")))
            .unwrap();

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.list_pull_request_comments(&pull_request.id)).unwrap(),
        vec![first, second]
    );
}

#[test]
fn pull_request_comment_operations_return_not_found_for_missing_pull_request() {
    let root = TestRoot::new("pull-request-comments");
    let forge = root.forge();
    let missing = PullRequestId::new("pull-request-repo-0000000000000001-0000000000009999");

    let list_error = block_on(forge.list_pull_request_comments(&missing)).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message)
            if message == "pull request pull-request-repo-0000000000000001-0000000000009999"
    ));

    let add_error =
        block_on(forge.add_pull_request_comment(&missing, comment("Missing"))).unwrap_err();
    assert!(matches!(
        add_error,
        ForgeError::NotFound(message)
            if message == "pull request pull-request-repo-0000000000000001-0000000000009999"
    ));
}

#[test]
fn pull_request_comments_are_scoped_to_their_pull_request_and_repository() {
    let root = TestRoot::new("pull-request-comments");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();
    let first_pull_request = block_on(forge.create_pull_request(
        &first_repository.id,
        pull_request(&first_repository.id, "First repo pull request"),
    ))
    .unwrap();
    let second_pull_request = block_on(forge.create_pull_request(
        &first_repository.id,
        pull_request(&first_repository.id, "Second pull request"),
    ))
    .unwrap();
    let other_repo_pull_request = block_on(forge.create_pull_request(
        &second_repository.id,
        pull_request(&second_repository.id, "Other repo pull request"),
    ))
    .unwrap();

    let first_comment = block_on(forge.add_pull_request_comment(
        &first_pull_request.id,
        comment("First pull request comment"),
    ))
    .unwrap();
    let second_comment = block_on(forge.add_pull_request_comment(
        &second_pull_request.id,
        comment("Second pull request comment"),
    ))
    .unwrap();
    let other_repo_comment = block_on(forge.add_pull_request_comment(
        &other_repo_pull_request.id,
        comment("Other repo pull request comment"),
    ))
    .unwrap();

    assert_ne!(first_comment.id, second_comment.id);
    assert_ne!(first_comment.id, other_repo_comment.id);
    assert_eq!(
        comment_bodies(
            &block_on(forge.list_pull_request_comments(&first_pull_request.id)).unwrap()
        ),
        vec!["First pull request comment"]
    );
    assert_eq!(
        comment_bodies(
            &block_on(forge.list_pull_request_comments(&second_pull_request.id)).unwrap()
        ),
        vec!["Second pull request comment"]
    );
    assert_eq!(
        comment_bodies(
            &block_on(forge.list_pull_request_comments(&other_repo_pull_request.id)).unwrap()
        ),
        vec!["Other repo pull request comment"]
    );
}
