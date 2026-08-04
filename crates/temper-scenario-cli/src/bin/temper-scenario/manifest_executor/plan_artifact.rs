// SPDX-License-Identifier: MPL-2.0

use temper_testing::live_manifest::{LiveManifestEvidence, LivePlanFeatureEvidence};

use crate::run_evidence;

pub(super) fn evidence_lines(plan: &LivePlanFeatureEvidence) -> Vec<String> {
    let mut lines = vec![
        format!(
            "plan-centric derived feature branch: {} (default metadata on feature #{}; plan #{})",
            plan.feature_branch, plan.feature_issue.number, plan.plan_issue.number
        ),
        format!(
            "issue target branches: feature={:?} plan={:?} first={:?} second={:?} scenario={:?} followup={:?}",
            plan.feature_issue.target_branch,
            plan.plan_issue.target_branch,
            plan.first_code_issue.target_branch,
            plan.second_code_issue.target_branch,
            plan.scenario_issue.target_branch,
            plan.followup_code_issue.target_branch
        ),
        format!(
            "sequential children: first #{} closed, second #{} closed, scenario #{} closed, followup #{} closed, code_blocked={} code_unblocked={} scenario_blocked={} scenario_unblocked={}",
            plan.first_code_issue.number,
            plan.second_code_issue.number,
            plan.scenario_issue.number,
            plan.followup_code_issue.number,
            plan.observed_second_blocked,
            plan.observed_second_unblocked,
            plan.observed_scenario_blocked,
            plan.observed_scenario_unblocked
        ),
        format!(
            "implementation PR targets: #{} {}->{}, #{} {}->{}, #{} {}->{}, #{} {}->{}",
            plan.first_pr.number,
            plan.first_pr.source_branch,
            plan.first_pr.target_branch,
            plan.second_pr.number,
            plan.second_pr.source_branch,
            plan.second_pr.target_branch,
            plan.scenario_pr.number,
            plan.scenario_pr.source_branch,
            plan.scenario_pr.target_branch,
            plan.followup_pr.number,
            plan.followup_pr.source_branch,
            plan.followup_pr.target_branch
        ),
        format!(
            "main topology: initial={} before_landing={} final={} validation_waited={} landing_open_with_parents_open={} ci_green_before_merge={}",
            plan.initial_main_sha,
            plan.main_sha_before_landing,
            plan.final_main_sha,
            plan.validation_waited_for_implementations,
            plan.observed_landing_open_with_parents_open,
            plan.ci_green_before_merge
        ),
        format!(
            "single feature landing PR: #{} {}->{} state={} merged_sha={:?}",
            plan.landing_pr.number,
            plan.landing_pr.source_branch,
            plan.landing_pr.target_branch,
            plan.landing_pr.state,
            plan.landing_pr.merged_sha
        ),
    ];
    lines.extend(plan.validation_audits.iter().map(|audit| {
        format!(
            "plan validation audit: ordinary comment {} author={} outcome={} role={} actor={} job={} transition={} coordination={}",
            audit.comment_id,
            audit.author_id,
            audit.outcome,
            audit.workflow_role,
            audit.forge_actor,
            audit.job_id,
            audit.routed_transition,
            audit.coordination_key
        )
    }));
    lines.extend(plan.prompt_guidance.iter().map(|prompt| {
        format!(
            "captured {} prompts: requests={} role_guidance={:?} prompt_guidance={:?} tool_guidance={:?} constraints={:?}",
            prompt.role,
            prompt.request_count,
            prompt.role_guidance_excerpt,
            prompt.prompt_guidance_excerpt,
            prompt.tool_guidance_excerpt,
            prompt.constraint_excerpts
        )
    }));
    lines
}

pub(super) fn final_state(
    evidence: &LiveManifestEvidence,
    plan: &LivePlanFeatureEvidence,
) -> run_evidence::FinalStateEvidence {
    run_evidence::FinalStateEvidence {
        issues: vec![
            issue("feature", &plan.feature_issue),
            issue("plan", &plan.plan_issue),
            issue("first-code", &plan.first_code_issue),
            issue("second-code", &plan.second_code_issue),
            issue("scenario", &plan.scenario_issue),
            issue("validation-followup", &plan.followup_code_issue),
        ],
        pull_requests: vec![
            pull_request("first-implementation", &plan.first_pr),
            pull_request("second-implementation", &plan.second_pr),
            pull_request("scenario-implementation", &plan.scenario_pr),
            pull_request("followup-implementation", &plan.followup_pr),
            pull_request("feature-landing", &plan.landing_pr),
        ],
        repositories: vec![run_evidence::RepositoryStateEvidence {
            id: Some(evidence.repo_id.clone()),
            slug: Some(evidence.repo_slug.clone()),
            branches: vec![
                run_evidence::RepositoryBranchStateEvidence {
                    name: evidence.repo_default_branch.clone(),
                    head_sha: Some(plan.final_main_sha.clone()),
                    contains_engineer_diff: Some(true),
                },
                run_evidence::RepositoryBranchStateEvidence {
                    name: plan.feature_branch.clone(),
                    head_sha: plan.followup_pr.merged_sha.clone(),
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
                    job_id: Some(job.job_id.clone()),
                    provider_run_id: job.provider_run_id.clone(),
                    provider_attempt: job.provider_attempt.clone(),
                    commit_sha: Some(job.commit_sha.clone()),
                    name: job.name.clone(),
                    status: job.status.clone(),
                    pull_request_number: Some(job.pull_request_number),
                    conclusion: job.conclusion.clone(),
                    provider_conclusion: job.provider_conclusion.clone(),
                    url: job.url.clone(),
                    verified_failure: job
                        .verified_failure
                        .as_ref()
                        .map(super::live::verified_failure_proof),
                })
                .collect(),
            observations: Vec::new(),
            heads: Vec::new(),
            failure_evidence: None,
            requests: super::live::ci_requests(evidence),
            request_capture_dropped: Some(evidence.ci_request_capture_dropped),
        },
    }
}

fn issue(
    id: &str,
    issue: &temper_testing::live_manifest::PlanIssueState,
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
    pull: &temper_testing::live_manifest::PlanPullRequestStateEvidence,
) -> run_evidence::PullRequestStateEvidence {
    run_evidence::PullRequestStateEvidence {
        number: pull.number,
        id: Some(id.to_string()),
        title: Some(pull.title.clone()),
        body: None,
        state: Some(pull.state.clone()),
        labels: pull.labels.clone(),
        head_branch: Some(pull.source_branch.clone()),
        head_sha: pull.head_sha.clone(),
        merged_sha: pull.merged_sha.clone(),
    }
}
