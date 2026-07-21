// SPDX-License-Identifier: MPL-2.0

use temper_testing::live_basic_delivery::{LiveBasicDeliveryEvidence, LivePlanFeatureEvidence};

use crate::run_evidence;

pub(super) fn evidence_lines(plan: &LivePlanFeatureEvidence) -> Vec<String> {
    vec![
        format!(
            "plan-centric feature branch: {} (feature #{} plan #{})",
            plan.feature_branch, plan.feature_issue.number, plan.plan_issue.number
        ),
        format!(
            "sequential children: first #{} closed, second #{} closed, observed_blocked={} observed_unblocked={}",
            plan.first_code_issue.number,
            plan.second_code_issue.number,
            plan.observed_second_blocked,
            plan.observed_second_unblocked
        ),
        format!(
            "implementation PR targets: #{} {}->{}, #{} {}->{}",
            plan.first_pr.number,
            plan.first_pr.source_branch,
            plan.first_pr.target_branch,
            plan.second_pr.number,
            plan.second_pr.source_branch,
            plan.second_pr.target_branch
        ),
        format!(
            "feature landing PR: #{} {}->{} state={} merged_sha={:?}",
            plan.landing_pr.number,
            plan.landing_pr.source_branch,
            plan.landing_pr.target_branch,
            plan.landing_pr.state,
            plan.landing_pr.merged_sha
        ),
        format!(
            "plan validation audit: ordinary comment {} author={} outcome={} role={} actor={} job={} transition={} coordination={}",
            plan.validation_audit.comment_id,
            plan.validation_audit.author_id,
            plan.validation_audit.outcome,
            plan.validation_audit.workflow_role,
            plan.validation_audit.forge_actor,
            plan.validation_audit.job_id,
            plan.validation_audit.routed_transition,
            plan.validation_audit.coordination_key
        ),
    ]
}

pub(super) fn final_state(
    evidence: &LiveBasicDeliveryEvidence,
    plan: &LivePlanFeatureEvidence,
) -> run_evidence::FinalStateEvidence {
    run_evidence::FinalStateEvidence {
        issues: vec![
            issue("feature", &plan.feature_issue),
            issue("plan", &plan.plan_issue),
            issue("first-code", &plan.first_code_issue),
            issue("second-code", &plan.second_code_issue),
        ],
        pull_requests: vec![
            pull_request("first-implementation", &plan.first_pr),
            pull_request("second-implementation", &plan.second_pr),
            pull_request("feature-landing", &plan.landing_pr),
        ],
        repositories: vec![run_evidence::RepositoryStateEvidence {
            id: Some(evidence.repo_id.clone()),
            slug: Some(evidence.repo_slug.clone()),
            branches: vec![
                run_evidence::RepositoryBranchStateEvidence {
                    name: evidence.repo_default_branch.clone(),
                    head_sha: plan.landing_pr.merged_sha.clone(),
                    contains_engineer_diff: Some(true),
                },
                run_evidence::RepositoryBranchStateEvidence {
                    name: plan.feature_branch.clone(),
                    head_sha: plan.second_pr.merged_sha.clone(),
                    contains_engineer_diff: Some(true),
                },
            ],
        }],
        ci: run_evidence::CiStateEvidence {
            completed_jobs: Some(plan.ci_jobs.len()),
            jobs: plan
                .ci_jobs
                .iter()
                .map(|job| run_evidence::CiJobEvidence {
                    name: job.name.clone(),
                    status: job.status.clone(),
                    pull_request_number: Some(job.pull_request_number),
                    conclusion: job.conclusion.clone(),
                    url: job.url.clone(),
                })
                .collect(),
        },
    }
}

fn issue(
    id: &str,
    issue: &temper_testing::live_basic_delivery::PlanIssueState,
) -> run_evidence::IssueStateEvidence {
    run_evidence::IssueStateEvidence {
        number: issue.number,
        id: Some(id.to_string()),
        title: Some(issue.title.clone()),
        state: Some(issue.state.clone()),
        labels: issue.labels.clone(),
    }
}

fn pull_request(
    id: &str,
    pull: &temper_testing::live_basic_delivery::PlanPullRequestStateEvidence,
) -> run_evidence::PullRequestStateEvidence {
    run_evidence::PullRequestStateEvidence {
        number: pull.number,
        id: Some(id.to_string()),
        title: Some(pull.title.clone()),
        body: None,
        state: Some(pull.state.clone()),
        labels: pull.labels.clone(),
        head_branch: Some(pull.source_branch.clone()),
        head_sha: None,
        merged_sha: pull.merged_sha.clone(),
    }
}
