mod support;

use support::{TestRoot, block_on, comment, issue, issue_with, repository, user_ids};
use temper_forge_model::{
    Comment, Forge, ForgeError, Issue, IssueId, IssueQuery, IssueState, ItemNumber, ItemSort,
    ItemSortField, RepositoryId, SortDirection, UpdateIssue, UserId,
};

fn comment_bodies(comments: &[Comment]) -> Vec<String> {
    comments
        .iter()
        .map(|comment| comment.body.clone())
        .collect()
}

fn issue_titles(issues: &[Issue]) -> Vec<String> {
    issues.iter().map(|issue| issue.title.clone()).collect()
}

#[test]
fn issues_are_empty_for_new_repository() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    assert_eq!(
        block_on(forge.list_issues(&repository.id, IssueQuery::default())).unwrap(),
        Vec::new()
    );
}

#[test]
fn issues_can_be_created_with_deterministic_identity_and_author() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    let created = block_on(forge.create_issue(
        &repository.id,
        issue_with(
            "Broken login",
            "Users cannot sign in.",
            &["triage", "bug", "bug"],
            &["user-2", "user-1", "user-2"],
        ),
    ))
    .unwrap();

    assert_eq!(
        created.id.as_str(),
        "issue-repo-0000000000000001-0000000000000001"
    );
    assert_eq!(created.repo_id, repository.id);
    assert_eq!(created.number, ItemNumber::new(1));
    assert_eq!(created.title, "Broken login");
    assert_eq!(created.body, "Users cannot sign in.");
    assert_eq!(created.state, IssueState::Open);
    assert_eq!(created.author_id, UserId::new("user-1"));
    assert_eq!(created.labels, vec!["bug", "triage"]);
    assert_eq!(created.assignees, user_ids(&["user-1", "user-2"]));
    assert_eq!(created.created_at, created.updated_at);
    assert_eq!(created.created_at.timestamp(), 2);
    assert_eq!(created.closed_at, None);

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.list_issues(&repository.id, IssueQuery::default())).unwrap(),
        vec![created]
    );
}

#[test]
fn issues_can_be_looked_up_by_id() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();
    let issue =
        block_on(forge.create_issue(&second_repository.id, issue("Second repo issue"))).unwrap();

    assert_eq!(block_on(forge.get_issue(&issue.id)).unwrap(), Some(issue));
    assert_eq!(
        block_on(forge.get_issue(&IssueId::new(
            "issue-repo-0000000000000001-0000000000009999"
        )))
        .unwrap(),
        None
    );
    assert_eq!(
        block_on(forge.list_issues(&first_repository.id, IssueQuery::default())).unwrap(),
        Vec::new()
    );
}

#[test]
fn issues_can_be_looked_up_by_repository_scoped_number() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first = block_on(forge.create_issue(&repository.id, issue("First"))).unwrap();
    let second = block_on(forge.create_issue(&repository.id, issue("Second"))).unwrap();

    assert_eq!(
        block_on(forge.get_issue_by_number(&repository.id, ItemNumber::new(1))).unwrap(),
        Some(first)
    );
    assert_eq!(
        block_on(forge.get_issue_by_number(&repository.id, ItemNumber::new(2))).unwrap(),
        Some(second)
    );
    assert_eq!(
        block_on(forge.get_issue_by_number(&repository.id, ItemNumber::new(999))).unwrap(),
        None
    );
    assert_eq!(
        block_on(forge.get_issue_by_number(
            &RepositoryId::new("repo-0000000000009999"),
            ItemNumber::new(1),
        ))
        .unwrap(),
        None
    );
}

#[test]
fn issue_operations_return_not_found_for_missing_repository_targets() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let missing = RepositoryId::new("repo-0000000000009999");

    let list_error = block_on(forge.list_issues(&missing, IssueQuery::default())).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));

    let create_error = block_on(forge.create_issue(&missing, issue("Missing repo"))).unwrap_err();
    assert!(matches!(
        create_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));
}

#[test]
fn updating_missing_issue_returns_not_found() {
    let root = TestRoot::new("issues");
    let forge = root.forge();

    let error = block_on(forge.update_issue(
        &IssueId::new("issue-repo-0000000000000001-0000000000009999"),
        UpdateIssue {
            title: Some("New title".into()),
            ..UpdateIssue::default()
        },
    ))
    .unwrap_err();

    assert!(matches!(
        error,
        ForgeError::NotFound(message)
            if message == "issue issue-repo-0000000000000001-0000000000009999"
    ));
}

#[test]
fn issue_lists_filter_by_state_labels_author_and_assignee() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let open_bug = block_on(forge.create_issue(
        &repository.id,
        issue_with("Open bug", "", &["bug", "urgent"], &["user-2"]),
    ))
    .unwrap();
    let docs = block_on(forge.create_issue(
        &repository.id,
        issue_with("Docs", "", &["docs"], &["user-3"]),
    ))
    .unwrap();
    let closed_bug = block_on(forge.create_issue(
        &repository.id,
        issue_with("Closed bug", "", &["bug", "urgent", "backend"], &["user-3"]),
    ))
    .unwrap();
    block_on(forge.update_issue(
        &closed_bug.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();

    let open = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            state: Some(IssueState::Open),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issue_titles(&open), vec!["Open bug", "Docs"]);

    let closed = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            state: Some(IssueState::Closed),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issue_titles(&closed), vec!["Closed bug"]);

    let bug_and_urgent = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            labels: vec!["bug".into(), "urgent".into()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issue_titles(&bug_and_urgent),
        vec!["Open bug", "Closed bug"]
    );

    let bug_and_missing = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            labels: vec!["bug".into(), "missing".into()],
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(bug_and_missing, Vec::new());

    let current_author = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            author_id: Some(UserId::new("user-1")),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issue_titles(&current_author),
        vec!["Open bug", "Docs", "Closed bug"]
    );

    let missing_author = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            author_id: Some(UserId::new("missing")),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(missing_author, Vec::new());

    let assigned_to_user_3 = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            assignee_id: Some(UserId::new("user-3")),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issue_titles(&assigned_to_user_3),
        vec!["Docs", "Closed bug"]
    );

    assert_eq!(open_bug.number, ItemNumber::new(1));
    assert_eq!(docs.number, ItemNumber::new(2));
}

#[test]
fn issue_lists_sort_deterministically() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first = block_on(forge.create_issue(&repository.id, issue("First"))).unwrap();
    block_on(forge.create_issue(&repository.id, issue("Second"))).unwrap();
    block_on(forge.create_issue(&repository.id, issue("Third"))).unwrap();

    let default = block_on(forge.list_issues(&repository.id, IssueQuery::default())).unwrap();
    assert_eq!(issue_titles(&default), vec!["First", "Second", "Third"]);

    let number_desc = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            sort: Some(ItemSort {
                field: ItemSortField::Number,
                direction: SortDirection::Desc,
            }),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issue_titles(&number_desc), vec!["Third", "Second", "First"]);

    let limited = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            limit: Some(2),
            sort: Some(ItemSort {
                field: ItemSortField::Number,
                direction: SortDirection::Desc,
            }),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(issue_titles(&limited), vec!["Third", "Second"]);
    let empty = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            limit: Some(0),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert!(empty.is_empty());

    let created_desc = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            sort: Some(ItemSort {
                field: ItemSortField::CreatedAt,
                direction: SortDirection::Desc,
            }),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issue_titles(&created_desc),
        vec!["Third", "Second", "First"]
    );

    block_on(forge.update_issue(
        &first.id,
        UpdateIssue {
            title: Some("First updated".into()),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    let updated_desc = block_on(forge.list_issues(
        &repository.id,
        IssueQuery {
            sort: Some(ItemSort {
                field: ItemSortField::UpdatedAt,
                direction: SortDirection::Desc,
            }),
            ..IssueQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(
        issue_titles(&updated_desc),
        vec!["First updated", "Third", "Second"]
    );
}

#[test]
fn issue_comments_are_empty_for_issue_with_no_comments() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(&repository.id, issue("Needs discussion"))).unwrap();

    assert_eq!(
        block_on(forge.list_issue_comments(&issue.id)).unwrap(),
        Vec::new()
    );
}

#[test]
fn issue_comments_can_be_added_with_deterministic_identity_author_and_timestamps() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(&repository.id, issue("Needs discussion"))).unwrap();

    let first = block_on(forge.add_issue_comment(&issue.id, comment("First comment"))).unwrap();
    let second = block_on(forge.add_issue_comment(&issue.id, comment("Second comment"))).unwrap();

    assert_eq!(
        first.id.as_str(),
        "comment-issue-repo-0000000000000001-0000000000000001-0000000000000001"
    );
    assert_eq!(
        second.id.as_str(),
        "comment-issue-repo-0000000000000001-0000000000000001-0000000000000002"
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

    let comments = block_on(forge.list_issue_comments(&issue.id)).unwrap();
    assert_eq!(comments, vec![first, second]);
}

#[test]
fn issue_comments_are_persisted_and_reopened() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(&repository.id, issue("Needs discussion"))).unwrap();
    let first = block_on(forge.add_issue_comment(&issue.id, comment("Persist me"))).unwrap();
    let second = block_on(forge.add_issue_comment(&issue.id, comment("Persist me too"))).unwrap();

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.list_issue_comments(&issue.id)).unwrap(),
        vec![first, second]
    );
}

#[test]
fn issue_comment_operations_return_not_found_for_missing_issue() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let missing = IssueId::new("issue-repo-0000000000000001-0000000000009999");

    let list_error = block_on(forge.list_issue_comments(&missing)).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message)
            if message == "issue issue-repo-0000000000000001-0000000000009999"
    ));

    let add_error = block_on(forge.add_issue_comment(&missing, comment("Missing"))).unwrap_err();
    assert!(matches!(
        add_error,
        ForgeError::NotFound(message)
            if message == "issue issue-repo-0000000000000001-0000000000009999"
    ));
}

#[test]
fn issue_comments_are_scoped_to_their_issue_and_repository() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();
    let first_issue =
        block_on(forge.create_issue(&first_repository.id, issue("First repo issue"))).unwrap();
    let second_issue =
        block_on(forge.create_issue(&first_repository.id, issue("Second issue"))).unwrap();
    let other_repo_issue =
        block_on(forge.create_issue(&second_repository.id, issue("Other repo issue"))).unwrap();

    let first_comment =
        block_on(forge.add_issue_comment(&first_issue.id, comment("First issue comment"))).unwrap();
    let second_comment =
        block_on(forge.add_issue_comment(&second_issue.id, comment("Second issue comment")))
            .unwrap();
    let other_repo_comment = block_on(
        forge.add_issue_comment(&other_repo_issue.id, comment("Other repo issue comment")),
    )
    .unwrap();

    assert_ne!(first_comment.id, second_comment.id);
    assert_ne!(first_comment.id, other_repo_comment.id);
    assert_eq!(
        comment_bodies(&block_on(forge.list_issue_comments(&first_issue.id)).unwrap()),
        vec!["First issue comment"]
    );
    assert_eq!(
        comment_bodies(&block_on(forge.list_issue_comments(&second_issue.id)).unwrap()),
        vec!["Second issue comment"]
    );
    assert_eq!(
        comment_bodies(&block_on(forge.list_issue_comments(&other_repo_issue.id)).unwrap()),
        vec!["Other repo issue comment"]
    );
}

#[test]
fn issue_label_updates_apply_set_then_removals_then_additions() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(
        &repository.id,
        issue_with("Labels", "", &["bug", "triage"], &[]),
    ))
    .unwrap();

    let updated = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            set_labels: Some(vec![
                "enhancement".into(),
                "bug".into(),
                "enhancement".into(),
            ]),
            remove_labels: vec!["bug".into(), "missing".into()],
            add_labels: vec!["docs".into(), "enhancement".into()],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.labels, vec!["docs", "enhancement"]);

    let updated = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            remove_labels: vec!["docs".into()],
            add_labels: vec!["bug".into(), "docs".into()],
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(updated.labels, vec!["bug", "docs", "enhancement"]);
}

#[test]
fn issue_assignee_updates_are_idempotent_set_operations() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(
        &repository.id,
        issue_with("Assignees", "", &[], &["user-2", "user-1", "user-2"]),
    ))
    .unwrap();
    assert_eq!(issue.assignees, user_ids(&["user-1", "user-2"]));

    let update = UpdateIssue {
        add_assignees: user_ids(&["user-2", "user-3", "user-3"]),
        remove_assignees: user_ids(&["user-1", "missing"]),
        ..UpdateIssue::default()
    };
    let updated = block_on(forge.update_issue(&issue.id, update.clone())).unwrap();
    assert_eq!(updated.assignees, user_ids(&["user-2", "user-3"]));

    let updated = block_on(forge.update_issue(&issue.id, update)).unwrap();
    assert_eq!(updated.assignees, user_ids(&["user-2", "user-3"]));
}

#[test]
fn issue_close_and_reopen_updates_closed_at_deterministically() {
    let root = TestRoot::new("issues");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let issue = block_on(forge.create_issue(&repository.id, issue("Lifecycle"))).unwrap();

    let closed = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(closed.state, IssueState::Closed);
    assert_eq!(closed.closed_at, Some(closed.updated_at));
    assert!(closed.updated_at > issue.updated_at);

    let reopened = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Open),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(reopened.state, IssueState::Open);
    assert_eq!(reopened.closed_at, None);
    assert!(reopened.updated_at > closed.updated_at);

    let closed_again = block_on(forge.update_issue(
        &issue.id,
        UpdateIssue {
            state: Some(IssueState::Closed),
            ..UpdateIssue::default()
        },
    ))
    .unwrap();
    assert_eq!(closed_again.closed_at, Some(closed_again.updated_at));
    assert!(closed_again.updated_at > reopened.updated_at);
}
