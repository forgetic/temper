mod support;

use harness_forge::{
    CiJobId, Forge, ForgeError, RepositoryPath, RepositoryQuery, RepositorySort,
    RepositorySortField, SortDirection, UserId,
};
use support::{block_on, repository, TestRoot};

fn repository_names(repositories: &[harness_forge::Repository]) -> Vec<String> {
    repositories
        .iter()
        .map(|repository| format!("{}/{}", repository.owner, repository.name))
        .collect()
}

#[test]
fn current_user_is_bootstrapped_and_lookup_by_id() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    let user = block_on(forge.current_user()).expect("current user should be bootstrapped");

    assert_eq!(user.id, UserId::new("user-1"));
    assert_eq!(user.handle, "local");
    assert_eq!(block_on(forge.get_user(&user.id)).unwrap(), Some(user));
    assert_eq!(
        block_on(forge.get_user(&UserId::new("missing"))).unwrap(),
        None
    );
}

#[test]
fn repositories_can_be_created_and_reopened_by_id_and_path() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    let created = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    assert_eq!(created.id.as_str(), "repo-0000000000000001");
    assert_eq!(created.owner, "alice");
    assert_eq!(created.name, "project");
    assert_eq!(created.default_branch, "main");
    assert_eq!(created.created_at, created.updated_at);

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.get_repository(&created.id)).unwrap(),
        Some(created.clone())
    );
    assert_eq!(
        block_on(reopened.get_repository_by_path(&RepositoryPath::new("alice", "project")))
            .unwrap(),
        Some(created)
    );
}

#[test]
fn duplicate_repository_paths_are_rejected() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let error = block_on(forge.create_repository(repository("alice", "project"))).unwrap_err();

    assert!(matches!(
        error,
        ForgeError::AlreadyExists(message) if message == "repository alice/project"
    ));
}

#[test]
fn repository_lists_are_sorted_by_path_by_default() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    block_on(forge.create_repository(repository("beta", "api"))).unwrap();
    block_on(forge.create_repository(repository("alpha", "zeta"))).unwrap();
    block_on(forge.create_repository(repository("alpha", "alpha"))).unwrap();

    let repositories = block_on(forge.list_repositories(RepositoryQuery::default())).unwrap();

    assert_eq!(
        repository_names(&repositories),
        vec!["alpha/alpha", "alpha/zeta", "beta/api"]
    );
}

#[test]
fn repository_lists_apply_requested_sort_direction() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    block_on(forge.create_repository(repository("alpha", "first"))).unwrap();
    block_on(forge.create_repository(repository("beta", "second"))).unwrap();
    block_on(forge.create_repository(repository("gamma", "third"))).unwrap();

    let repositories = block_on(forge.list_repositories(RepositoryQuery {
        sort: Some(RepositorySort {
            field: RepositorySortField::CreatedAt,
            direction: SortDirection::Desc,
        }),
    }))
    .unwrap();

    assert_eq!(
        repository_names(&repositories),
        vec!["gamma/third", "beta/second", "alpha/first"]
    );

    let repositories = block_on(forge.list_repositories(RepositoryQuery {
        sort: Some(RepositorySort {
            field: RepositorySortField::Path,
            direction: SortDirection::Desc,
        }),
    }))
    .unwrap();

    assert_eq!(
        repository_names(&repositories),
        vec!["gamma/third", "beta/second", "alpha/first"]
    );
}

#[test]
fn repository_lists_support_updated_at_sorting() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    block_on(forge.create_repository(repository("alpha", "first"))).unwrap();
    block_on(forge.create_repository(repository("beta", "second"))).unwrap();

    let repositories = block_on(forge.list_repositories(RepositoryQuery {
        sort: Some(RepositorySort {
            field: RepositorySortField::UpdatedAt,
            direction: SortDirection::Asc,
        }),
    }))
    .unwrap();

    assert_eq!(
        repository_names(&repositories),
        vec!["alpha/first", "beta/second"]
    );
}

#[test]
fn unsupported_operations_return_documented_portable_error() {
    let root = TestRoot::new("repositories");
    let forge = root.forge();

    let error = block_on(forge.get_ci_job(&CiJobId::new("ci-job-1"))).unwrap_err();

    assert!(matches!(
        error,
        ForgeError::InvalidRequest(message)
            if message == "filesystem backend does not support get_ci_job yet"
    ));
}
