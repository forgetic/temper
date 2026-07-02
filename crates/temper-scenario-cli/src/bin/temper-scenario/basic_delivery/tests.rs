// SPDX-License-Identifier: MPL-2.0

use super::{IntakeSeed, RepoSeed, read_evidence};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{
    BranchRef, CiJob, CiJobConclusion, CiJobId, CiJobStatus, CreateIssue, CreatePullRequest, Forge,
    MergeMethod, MergePullRequest, PullRequest, RepositoryId,
};

#[test]
fn evidence_rejects_open_pr_even_with_passing_ci() {
    let error = temper_testing::block_on(async {
        let (forge, repo, seed, repo_seed) = fixture_state(false).await;
        read_evidence(&forge, &repo, &seed, &repo_seed)
            .await
            .expect_err("open PR must not satisfy the scenario contract")
            .to_string()
    });

    assert!(
        error.contains("implementation PR #1 was not merged"),
        "{error}"
    );
}

#[test]
fn evidence_rejects_merged_pr_when_parent_issue_is_still_open() {
    let error = temper_testing::block_on(async {
        let (forge, repo, seed, repo_seed) = fixture_state(true).await;
        read_evidence(&forge, &repo, &seed, &repo_seed)
            .await
            .expect_err("open parent issue must not satisfy the scenario contract")
            .to_string()
    });

    assert!(
        error.contains("seeded code issue #1 was not closed after merge"),
        "{error}"
    );
}

async fn fixture_state(merge_pr: bool) -> (MemoryForge, RepositoryId, IntakeSeed, RepoSeed) {
    let forge = MemoryForge::new();
    let repo = forge
        .create_repository(temper_testing::repo_input())
        .await
        .expect("repository created")
        .id;
    let seed = IntakeSeed {
        title: "Seeded contract work".to_string(),
        body: "Implement the contract.".to_string(),
        labels: Vec::new(),
    };
    let issue = forge
        .create_issue(
            &repo,
            CreateIssue {
                title: seed.title.clone(),
                body: seed.body.clone(),
                labels: vec!["code".to_string(), "in-progress".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("issue created");
    let pull_request = forge
        .create_pull_request(
            &repo,
            CreatePullRequest {
                title: format!("Implement #{}", issue.number),
                body: format!("Implementation for parent #{}.", issue.number),
                source: BranchRef {
                    repository_id: repo.clone(),
                    branch: "fake/pr-for-code-1".to_string(),
                },
                target: BranchRef {
                    repository_id: repo.clone(),
                    branch: "main".to_string(),
                },
                labels: vec!["implementation".to_string(), "landing".to_string()],
                assignees: Vec::new(),
            },
        )
        .await
        .expect("pull request created");
    forge.seed_ci_jobs(&repo, vec![ci_job(&repo, &pull_request)]);
    if merge_pr {
        forge
            .merge_pull_request(
                &pull_request.id,
                MergePullRequest {
                    method: MergeMethod::MergeCommit,
                    commit_title: None,
                    commit_body: None,
                    delete_source_branch: false,
                },
            )
            .await
            .expect("pull request merged");
    }
    let repo_seed = RepoSeed {
        id: "service".to_string(),
        slug: "acme/service".to_string(),
        default_branch: "main".to_string(),
    };
    (forge, repo, seed, repo_seed)
}

fn ci_job(repo: &RepositoryId, pull_request: &PullRequest) -> CiJob {
    let now = temper_testing::ts("2026-05-29T00:00:00Z");
    CiJob {
        id: CiJobId::new("ci-basic-delivery-contract"),
        repo_id: repo.clone(),
        pull_request_id: Some(pull_request.id.clone()),
        commit_sha: pull_request.head_sha.clone().unwrap_or_default(),
        name: "ci".to_string(),
        status: CiJobStatus::Completed,
        conclusion: Some(CiJobConclusion::Success),
        url: None,
        created_at: now,
        started_at: Some(now),
        completed_at: Some(now),
        updated_at: now,
    }
}
