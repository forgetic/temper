mod support;

use harness_forge::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobSort, CiJobSortField, CiJobStatus, Forge,
    ForgeError, PullRequestId, RepositoryId, SortDirection,
};
use support::{block_on, pull_request, repository, timestamp, write_ci_jobs, TestRoot};

fn ci_job(
    repo_id: &RepositoryId,
    id_suffix: &str,
    name: &str,
    commit_sha: &str,
    status: CiJobStatus,
    created_at: i64,
    updated_at: i64,
) -> CiJob {
    CiJob {
        id: CiJobId::new(format!("ci-job-{}-{id_suffix}", repo_id.as_str())),
        repo_id: repo_id.clone(),
        pull_request_id: None,
        commit_sha: commit_sha.into(),
        name: name.into(),
        status,
        conclusion: (status == CiJobStatus::Completed).then_some(CiJobConclusion::Success),
        url: None,
        created_at: timestamp(created_at),
        started_at: (status != CiJobStatus::Queued).then_some(timestamp(created_at + 1)),
        completed_at: (status == CiJobStatus::Completed).then_some(timestamp(updated_at)),
        updated_at: timestamp(updated_at),
    }
}

fn ci_job_names(ci_jobs: &[CiJob]) -> Vec<String> {
    ci_jobs.iter().map(|ci_job| ci_job.name.clone()).collect()
}

#[test]
fn ci_jobs_are_empty_for_new_repository() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    assert_eq!(
        block_on(forge.list_ci_jobs(&repository.id, CiJobQuery::default())).unwrap(),
        Vec::new()
    );
}

#[test]
fn ci_jobs_can_be_listed_and_looked_up_by_id() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let test = ci_job(
        &repository.id,
        "test",
        "Test",
        "abc123",
        CiJobStatus::Completed,
        3,
        6,
    );
    let build = ci_job(
        &repository.id,
        "build",
        "Build",
        "abc123",
        CiJobStatus::Running,
        2,
        5,
    );
    write_ci_jobs(&forge, &repository.id, &[test.clone(), build.clone()]);

    let listed = block_on(forge.list_ci_jobs(&repository.id, CiJobQuery::default())).unwrap();
    assert_eq!(listed, vec![build.clone(), test.clone()]);
    assert_eq!(block_on(forge.get_ci_job(&test.id)).unwrap(), Some(test));
    assert_eq!(
        block_on(forge.get_ci_job(&CiJobId::new("ci-job-missing"))).unwrap(),
        None
    );

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.get_ci_job(&build.id)).unwrap(),
        Some(build)
    );
}

#[test]
fn ci_job_lists_filter_by_pull_request_commit_sha_and_status() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let pull_request = block_on(
        forge.create_pull_request(&repository.id, pull_request(&repository.id, "Add checks")),
    )
    .unwrap();
    let other_pull_request =
        PullRequestId::new("pull-request-repo-0000000000009999-0000000000000001");
    let mut build = ci_job(
        &repository.id,
        "build",
        "Build",
        "abc123",
        CiJobStatus::Queued,
        2,
        2,
    );
    build.pull_request_id = Some(pull_request.id.clone());
    let test = ci_job(
        &repository.id,
        "test",
        "Test",
        "abc123",
        CiJobStatus::Running,
        3,
        4,
    );
    let mut deploy = ci_job(
        &repository.id,
        "deploy",
        "Deploy",
        "def456",
        CiJobStatus::Completed,
        5,
        8,
    );
    deploy.pull_request_id = Some(pull_request.id.clone());
    write_ci_jobs(
        &forge,
        &repository.id,
        &[build.clone(), test.clone(), deploy.clone()],
    );

    let pull_request_jobs = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            pull_request_id: Some(pull_request.id.clone()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(ci_job_names(&pull_request_jobs), vec!["Build", "Deploy"]);

    let missing_pull_request_jobs = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            pull_request_id: Some(other_pull_request),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(missing_pull_request_jobs, Vec::new());

    let commit_jobs = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            commit_sha: Some("abc123".into()),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(ci_job_names(&commit_jobs), vec!["Build", "Test"]);

    let completed_jobs = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            status: Some(CiJobStatus::Completed),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(completed_jobs, vec![deploy]);
}

#[test]
fn ci_job_lists_sort_deterministically() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    write_ci_jobs(
        &forge,
        &repository.id,
        &[
            ci_job(
                &repository.id,
                "b",
                "B",
                "sha-b",
                CiJobStatus::Running,
                3,
                9,
            ),
            ci_job(&repository.id, "c", "C", "sha-c", CiJobStatus::Queued, 4, 6),
            ci_job(
                &repository.id,
                "a",
                "A",
                "sha-a",
                CiJobStatus::Completed,
                5,
                7,
            ),
        ],
    );

    let default = block_on(forge.list_ci_jobs(&repository.id, CiJobQuery::default())).unwrap();
    assert_eq!(ci_job_names(&default), vec!["A", "B", "C"]);

    let name_desc = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            sort: Some(CiJobSort {
                field: CiJobSortField::Name,
                direction: SortDirection::Desc,
            }),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(ci_job_names(&name_desc), vec!["C", "B", "A"]);

    let created_desc = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            sort: Some(CiJobSort {
                field: CiJobSortField::CreatedAt,
                direction: SortDirection::Desc,
            }),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(ci_job_names(&created_desc), vec!["A", "C", "B"]);

    let updated_asc = block_on(forge.list_ci_jobs(
        &repository.id,
        CiJobQuery {
            sort: Some(CiJobSort {
                field: CiJobSortField::UpdatedAt,
                direction: SortDirection::Asc,
            }),
            ..CiJobQuery::default()
        },
    ))
    .unwrap();
    assert_eq!(ci_job_names(&updated_asc), vec!["C", "A", "B"]);
}

#[test]
fn ci_jobs_are_scoped_to_repositories() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let first_repository = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second_repository =
        block_on(forge.create_repository(repository("alice", "second"))).unwrap();
    let first_job = ci_job(
        &first_repository.id,
        "build",
        "Build first",
        "abc123",
        CiJobStatus::Completed,
        3,
        5,
    );
    let second_job = ci_job(
        &second_repository.id,
        "build",
        "Build second",
        "abc123",
        CiJobStatus::Completed,
        4,
        6,
    );
    write_ci_jobs(
        &forge,
        &first_repository.id,
        std::slice::from_ref(&first_job),
    );
    write_ci_jobs(
        &forge,
        &second_repository.id,
        std::slice::from_ref(&second_job),
    );

    assert_eq!(
        block_on(forge.list_ci_jobs(&first_repository.id, CiJobQuery::default())).unwrap(),
        vec![first_job.clone()]
    );
    assert_eq!(
        block_on(forge.list_ci_jobs(&second_repository.id, CiJobQuery::default())).unwrap(),
        vec![second_job.clone()]
    );
    assert_eq!(
        block_on(forge.get_ci_job(&first_job.id)).unwrap(),
        Some(first_job)
    );
    assert_eq!(
        block_on(forge.get_ci_job(&second_job.id)).unwrap(),
        Some(second_job)
    );
}

#[test]
fn duplicate_ci_job_ids_return_backend_error() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();
    let first = ci_job(
        &repository.id,
        "duplicate",
        "Build",
        "abc123",
        CiJobStatus::Completed,
        3,
        5,
    );
    let mut second = ci_job(
        &repository.id,
        "duplicate",
        "Test",
        "def456",
        CiJobStatus::Completed,
        4,
        6,
    );
    second.id = first.id.clone();
    write_ci_jobs(&forge, &repository.id, &[first.clone(), second]);

    let error = block_on(forge.list_ci_jobs(&repository.id, CiJobQuery::default())).unwrap_err();
    assert!(matches!(
        error,
        ForgeError::Backend(message)
            if message == format!(
                "filesystem storage contains duplicate CI job id {} in repository {}",
                first.id, repository.id
            )
    ));
}

#[test]
fn ci_job_operations_handle_missing_repository_and_job_targets() {
    let root = TestRoot::new("ci-jobs");
    let forge = root.forge();
    let missing_repository = RepositoryId::new("repo-0000000000009999");

    let list_error =
        block_on(forge.list_ci_jobs(&missing_repository, CiJobQuery::default())).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));

    assert_eq!(
        block_on(forge.get_ci_job(&CiJobId::new("ci-job-missing"))).unwrap(),
        None
    );
}
