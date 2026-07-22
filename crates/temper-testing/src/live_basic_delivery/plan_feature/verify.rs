use std::time::Instant;

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{
    Issue, IssueQuery, ItemNumber, PullRequest, PullRequestQuery, PullRequestState, RepositoryId,
};
use temper_workflow::parse_metadata_block;

use super::audit::{ValidationAuditExpectation, validation_audit_evidence};
use super::{
    ASSERT_POLL, FIRST_CODE_TITLE, FOLLOWUP_CODE_TITLE, FOLLOWUP_VALIDATION_SUMMARY, IssueState,
    LANDING_TITLE, LivePlanFeatureEvidence, PLAN_TITLE, PullRequestCiJobEvidence,
    PullRequestStateEvidence, SECOND_CODE_TITLE, VALIDATION_SUMMARY,
};
use crate::live_basic_delivery::convergence::{ci_job_evidence, completed_ci_jobs};

#[derive(Default)]
struct Observations {
    second_blocked: bool,
    second_unblocked_after_first_closed: bool,
    landing_open_with_parents_open: bool,
    main_sha_before_landing: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn poll_plan_feature(
    deadline: Instant,
    standalone: &mut super::super::process::ChildGuard,
    forge: &ForgejoForge,
    repository: &RepositoryId,
    feature_issue: ItemNumber,
    default_branch: &str,
    initial_main_sha: &str,
    forge_url: &str,
    admin_token: &str,
    owner: &str,
    repo: &str,
) -> Result<LivePlanFeatureEvidence, String> {
    let mut observations = Observations::default();
    loop {
        if let Some(status) = standalone.try_wait()? {
            return Err(format!("{} exited early with {status:?}", standalone.label));
        }
        let current_main_sha =
            match super::remote_branch_head(forge_url, admin_token, owner, repo, default_branch) {
                Ok(sha) => sha,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(ASSERT_POLL);
                    continue;
                }
                Err(error) => return Err(error),
            };
        match super::super::process::engine_block_on(verify_plan_feature(
            forge,
            repository,
            feature_issue,
            default_branch,
            initial_main_sha,
            &current_main_sha,
            &mut observations,
        )) {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(ASSERT_POLL);
            }
            Err(error) => return Err(error),
        }
    }
}

async fn verify_plan_feature(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    feature_issue: ItemNumber,
    default_branch: &str,
    initial_main_sha: &str,
    current_main_sha: &str,
    observations: &mut Observations,
) -> Result<LivePlanFeatureEvidence, String> {
    let issues = forge
        .list_issues(repository, IssueQuery::default())
        .await
        .map_err(|error| format!("list issues: {error}"))?;
    let pulls = forge
        .list_pull_requests(repository, PullRequestQuery::default())
        .await
        .map_err(|error| format!("list pull requests: {error}"))?;

    observe_progress(
        &issues,
        &pulls,
        feature_issue,
        default_branch,
        initial_main_sha,
        current_main_sha,
        observations,
    )?;

    let feature = issue_by_number(&issues, feature_issue)?;
    let expected_branch = format!("agent/pr-for-feature-{}", feature.number.get());
    if expected_branch == default_branch {
        return Err(format!(
            "derived feature branch unexpectedly equals repository default {default_branch:?}"
        ));
    }
    require_issue_target(feature, default_branch)?;

    let plan = issue_by_label_and_title(&issues, "plan", PLAN_TITLE)?;
    let first = issue_by_label_and_title(&issues, "code", FIRST_CODE_TITLE)?;
    let second = issue_by_label_and_title(&issues, "code", SECOND_CODE_TITLE)?;
    let followup = issue_by_label_and_title(&issues, "code", FOLLOWUP_CODE_TITLE)
        .map_err(|error| format!("{error}\n{}", describe_state(&issues, &pulls)))?;
    for issue in [plan, first, second, followup] {
        require_issue_target(issue, &expected_branch)?;
    }

    let implementation_prs = pulls
        .iter()
        .filter(|pull| has_pr_label(pull, "implementation"))
        .collect::<Vec<_>>();
    if implementation_prs.len() != 3 {
        return Err(format!(
            "expected exactly three implementation PRs, found {}\n{}",
            implementation_prs.len(),
            describe_state(&issues, &pulls)
        ));
    }
    let first_pr = pr_by_title(&implementation_prs, FIRST_CODE_TITLE)?;
    let second_pr = pr_by_title(&implementation_prs, SECOND_CODE_TITLE)?;
    let followup_pr = pr_by_title(&implementation_prs, FOLLOWUP_CODE_TITLE)?;
    for pull in [first_pr, second_pr, followup_pr] {
        require_pr_target(pull, &expected_branch)?;
    }

    let landing_prs = pulls
        .iter()
        .filter(|pull| has_pr_label(pull, "feature-landing"))
        .collect::<Vec<_>>();
    if landing_prs.len() != 1 {
        return Err(format!(
            "expected exactly one feature-landing PR, found {}",
            landing_prs.len()
        ));
    }
    let landing_pr = landing_prs[0];
    if landing_pr.title != LANDING_TITLE {
        return Err(format!(
            "feature landing title mismatch: expected {LANDING_TITLE:?}, got {:?}",
            landing_pr.title
        ));
    }
    require_pr_branch(landing_pr, &expected_branch, default_branch)?;

    require_terminal_state(
        feature,
        plan,
        [first, second, followup],
        [first_pr, second_pr, followup_pr],
        landing_pr,
        observations,
    )?;
    require_merge_topology(
        initial_main_sha,
        observations,
        [first_pr, second_pr, followup_pr],
        landing_pr,
    )?;
    require_validation_order([first_pr, second_pr], followup, followup_pr, landing_pr)?;

    let (ci_jobs, ci_green_before_merge) = merged_pr_ci_evidence(
        forge,
        repository,
        &[first_pr, second_pr, followup_pr, landing_pr],
    )
    .await?;
    let validation_audits = validation_audit_evidence(
        forge,
        plan,
        &[
            ValidationAuditExpectation {
                outcome: "needs_followup",
                summary: FOLLOWUP_VALIDATION_SUMMARY,
                transition: "plan_validation_needs_followup",
            },
            ValidationAuditExpectation {
                outcome: "validated",
                summary: VALIDATION_SUMMARY,
                transition: "plan_validated_create_landing",
            },
        ],
    )
    .await?;

    Ok(LivePlanFeatureEvidence {
        feature_branch: expected_branch,
        feature_issue: issue_state(feature)?,
        plan_issue: issue_state(plan)?,
        first_code_issue: issue_state(first)?,
        second_code_issue: issue_state(second)?,
        followup_code_issue: issue_state(followup)?,
        first_pr: pr_state(first_pr),
        second_pr: pr_state(second_pr),
        followup_pr: pr_state(followup_pr),
        landing_pr: pr_state(landing_pr),
        ci_jobs,
        validation_audits,
        prompt_guidance: Vec::new(),
        initial_main_sha: initial_main_sha.to_string(),
        main_sha_before_landing: observations
            .main_sha_before_landing
            .clone()
            .unwrap_or_default(),
        final_main_sha: String::new(),
        observed_second_blocked: observations.second_blocked,
        observed_second_unblocked: observations.second_unblocked_after_first_closed,
        observed_landing_open_with_parents_open: observations.landing_open_with_parents_open,
        validation_waited_for_implementations: true,
        ci_green_before_merge,
    })
}

fn observe_progress(
    issues: &[Issue],
    pulls: &[PullRequest],
    feature_issue: ItemNumber,
    default_branch: &str,
    initial_main_sha: &str,
    current_main_sha: &str,
    observations: &mut Observations,
) -> Result<(), String> {
    let feature = issues.iter().find(|issue| issue.number == feature_issue);
    let plan = optional_issue(issues, "plan", PLAN_TITLE);
    let first = optional_issue(issues, "code", FIRST_CODE_TITLE);
    let second = optional_issue(issues, "code", SECOND_CODE_TITLE);

    if second.is_some_and(|issue| has_label(issue, "blocked")) {
        observations.second_blocked = true;
    }
    if first.is_some_and(issue_closed) && second.is_some_and(|issue| !has_label(issue, "blocked")) {
        observations.second_unblocked_after_first_closed = true;
    }

    let landing_prs = pulls
        .iter()
        .filter(|pull| has_pr_label(pull, "feature-landing"))
        .collect::<Vec<_>>();
    if landing_prs.len() > 1 {
        return Err(format!(
            "more than one feature-landing PR appeared: {}",
            landing_prs.len()
        ));
    }
    let landing = landing_prs.first().copied();
    if !landing.is_some_and(|pull| matches!(pull.state, PullRequestState::Merged))
        && current_main_sha != initial_main_sha
    {
        return Err(format!(
            "repository default branch advanced before aggregate landing: initial={initial_main_sha} current={current_main_sha}"
        ));
    }
    if let Some(landing) = landing {
        if !matches!(landing.state, PullRequestState::Open) {
            return Ok(());
        }
        if pulls.iter().any(|pull| {
            pull.number != landing.number
                && matches!(pull.state, PullRequestState::Merged)
                && pull.target.branch == default_branch
        }) {
            return Err(
                "a non-landing PR advanced the repository default branch before aggregate landing"
                    .to_string(),
            );
        }
        observations
            .main_sha_before_landing
            .get_or_insert_with(|| current_main_sha.to_string());
        if feature.is_some_and(|issue| !issue_closed(issue))
            && plan.is_some_and(|issue| !issue_closed(issue))
        {
            observations.landing_open_with_parents_open = true;
        }
    }
    Ok(())
}

fn require_terminal_state(
    feature: &Issue,
    plan: &Issue,
    code_issues: [&Issue; 3],
    implementation_prs: [&PullRequest; 3],
    landing: &PullRequest,
    observations: &Observations,
) -> Result<(), String> {
    if !observations.second_blocked || !observations.second_unblocked_after_first_closed {
        return Err("sequential dependency block/unblock was not fully observed".to_string());
    }
    if !code_issues.into_iter().all(issue_closed) {
        return Err("implementation issues are not all closed yet".to_string());
    }
    if !implementation_prs
        .into_iter()
        .all(|pull| matches!(pull.state, PullRequestState::Merged))
    {
        return Err("implementation PRs are not all merged yet".to_string());
    }
    if !matches!(landing.state, PullRequestState::Merged) {
        return Err("feature landing PR is not merged yet".to_string());
    }
    if !issue_closed(feature) || !issue_closed(plan) {
        return Err("feature and plan issues are not both closed yet".to_string());
    }
    if !observations.landing_open_with_parents_open {
        return Err(
            "feature and plan were not observed open while the aggregate landing PR was open"
                .to_string(),
        );
    }
    let landing_merged_at = landing
        .merge
        .as_ref()
        .map(|merge| merge.merged_at)
        .ok_or_else(|| "landing PR has no merge record".to_string())?;
    for issue in [feature, plan] {
        if issue
            .closed_at
            .is_none_or(|closed| closed < landing_merged_at)
        {
            return Err(format!(
                "{} #{} closed before aggregate landing merged",
                issue.title, issue.number
            ));
        }
    }
    Ok(())
}

fn require_merge_topology(
    initial_main_sha: &str,
    observations: &Observations,
    implementations: [&PullRequest; 3],
    landing: &PullRequest,
) -> Result<(), String> {
    let merge_shas = [
        merge_sha(implementations[0])?,
        merge_sha(implementations[1])?,
        merge_sha(implementations[2])?,
    ];
    if merge_shas.iter().any(|sha| *sha == initial_main_sha)
        || merge_shas[0] == merge_shas[1]
        || merge_shas[1] == merge_shas[2]
        || merge_shas[0] == merge_shas[2]
    {
        return Err(format!(
            "implementation merges did not advance the feature branch in three distinct steps: {merge_shas:?}"
        ));
    }
    require_sha(
        landing.head_sha.as_deref(),
        merge_shas[2],
        "aggregate landing head",
    )?;
    require_sha(
        observations.main_sha_before_landing.as_deref(),
        initial_main_sha,
        "main before aggregate landing",
    )?;
    Ok(())
}

fn require_validation_order(
    initial: [&PullRequest; 2],
    followup: &Issue,
    followup_pr: &PullRequest,
    landing: &PullRequest,
) -> Result<(), String> {
    for pull in initial {
        let merged_at = pull
            .merge
            .as_ref()
            .map(|merge| merge.merged_at)
            .ok_or_else(|| format!("implementation PR #{} has no merge time", pull.number))?;
        if followup.created_at < merged_at {
            return Err(format!(
                "validation follow-up #{} was created before implementation PR #{} merged",
                followup.number, pull.number
            ));
        }
    }
    let followup_merged_at = followup_pr
        .merge
        .as_ref()
        .map(|merge| merge.merged_at)
        .ok_or_else(|| "follow-up implementation PR has no merge time".to_string())?;
    if landing.created_at < followup_merged_at {
        return Err(
            "aggregate landing was created before the validation follow-up merged".to_string(),
        );
    }
    Ok(())
}

async fn merged_pr_ci_evidence(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    pulls: &[&PullRequest],
) -> Result<(Vec<PullRequestCiJobEvidence>, bool), String> {
    let mut evidence = Vec::new();
    for pull in pulls {
        let jobs = completed_ci_jobs(forge, repository, pull).await?;
        let latest = jobs
            .last()
            .ok_or_else(|| format!("no completed CI jobs for PR #{}", pull.number))?;
        if latest.conclusion != Some(temper_forge_model::CiJobConclusion::Success) {
            return Err(format!(
                "latest completed CI job for PR #{} was not successful: {:?}",
                pull.number, latest.conclusion
            ));
        }
        let merged_at = pull
            .merge
            .as_ref()
            .map(|merge| merge.merged_at)
            .ok_or_else(|| format!("PR #{} has no merge record", pull.number))?;
        let completed_at = latest.completed_at.unwrap_or(latest.updated_at);
        if completed_at > merged_at {
            return Err(format!(
                "PR #{} merged at {merged_at} before successful CI completed at {completed_at}",
                pull.number
            ));
        }
        evidence.extend(jobs.iter().map(|job| {
            let job = ci_job_evidence(job);
            PullRequestCiJobEvidence {
                pull_request_number: pull.number.get(),
                name: job.name,
                status: job.status,
                conclusion: job.conclusion,
                url: job.url,
            }
        }));
    }
    Ok((evidence, true))
}

fn require_issue_target(issue: &Issue, expected: &str) -> Result<(), String> {
    let actual = issue_target_branch(issue)?;
    if actual.as_deref() != Some(expected) {
        return Err(format!(
            "issue #{} target mismatch: expected {expected:?}, got {actual:?}",
            issue.number
        ));
    }
    Ok(())
}

fn issue_target_branch(issue: &Issue) -> Result<Option<String>, String> {
    parse_metadata_block(&issue.body)
        .map_err(|error| format!("parse issue #{} metadata: {error}", issue.number))
        .map(|metadata| metadata.and_then(|metadata| metadata.target_branch))
}

fn merge_sha(pull: &PullRequest) -> Result<&str, String> {
    pull.merge
        .as_ref()
        .map(|merge| merge.commit_sha.as_str())
        .ok_or_else(|| format!("PR #{} has no merge SHA", pull.number))
}

fn require_sha(actual: Option<&str>, expected: &str, label: &str) -> Result<(), String> {
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "{label} mismatch: expected {expected:?}, got {actual:?}"
        ))
    }
}

fn issue_by_number(issues: &[Issue], number: ItemNumber) -> Result<&Issue, String> {
    issues
        .iter()
        .find(|issue| issue.number == number)
        .ok_or_else(|| format!("issue #{number} not found"))
}

fn optional_issue<'a>(issues: &'a [Issue], label: &str, title: &str) -> Option<&'a Issue> {
    issues
        .iter()
        .find(|issue| has_label(issue, label) && issue.title == title)
}

fn issue_by_label_and_title<'a>(
    issues: &'a [Issue],
    label: &str,
    title: &str,
) -> Result<&'a Issue, String> {
    optional_issue(issues, label, title)
        .ok_or_else(|| format!("issue `{title}` with label `{label}` not found yet"))
}

fn pr_by_title<'a>(pulls: &'a [&PullRequest], title: &str) -> Result<&'a PullRequest, String> {
    pulls
        .iter()
        .copied()
        .find(|pull| pull.title == title)
        .ok_or_else(|| format!("implementation PR `{title}` not found yet"))
}

fn require_pr_branch(
    pull: &PullRequest,
    source_branch: &str,
    target_branch: &str,
) -> Result<(), String> {
    if pull.source.branch != source_branch || pull.target.branch != target_branch {
        return Err(format!(
            "PR #{} branch mismatch: expected {source_branch}->{target_branch}, got {}->{}",
            pull.number, pull.source.branch, pull.target.branch
        ));
    }
    Ok(())
}

fn require_pr_target(pull: &PullRequest, target_branch: &str) -> Result<(), String> {
    if pull.target.branch != target_branch {
        return Err(format!(
            "PR #{} target mismatch: expected {target_branch}, got {}->{}",
            pull.number, pull.source.branch, pull.target.branch
        ));
    }
    Ok(())
}

fn describe_state(issues: &[Issue], pulls: &[PullRequest]) -> String {
    let issues = issues
        .iter()
        .map(|issue| {
            format!(
                "issue #{} {:?} title={:?} labels={:?} deps={:?}",
                issue.number, issue.state, issue.title, issue.labels, issue.dependencies
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let pulls = pulls
        .iter()
        .map(|pull| {
            format!(
                "pr #{} {:?} title={:?} labels={:?} branch {}->{} deps={:?}",
                pull.number,
                pull.state,
                pull.title,
                pull.labels,
                pull.source.branch,
                pull.target.branch,
                pull.dependencies
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("observed issues:\n{issues}\nobserved pull requests:\n{pulls}")
}

fn has_label(issue: &Issue, label: &str) -> bool {
    issue.labels.iter().any(|candidate| candidate == label)
}

fn issue_closed(issue: &Issue) -> bool {
    matches!(issue.state, temper_forge_model::IssueState::Closed)
}

fn has_pr_label(pull: &PullRequest, label: &str) -> bool {
    pull.labels.iter().any(|candidate| candidate == label)
}

fn issue_state(issue: &Issue) -> Result<IssueState, String> {
    Ok(IssueState {
        number: issue.number.get(),
        title: issue.title.clone(),
        state: if issue_closed(issue) {
            "closed"
        } else {
            "open"
        }
        .to_string(),
        labels: issue.labels.clone(),
        target_branch: issue_target_branch(issue)?,
    })
}

fn pr_state(pull: &PullRequest) -> PullRequestStateEvidence {
    PullRequestStateEvidence {
        number: pull.number.get(),
        title: pull.title.clone(),
        state: match pull.state {
            PullRequestState::Open => "open",
            PullRequestState::Closed => "closed",
            PullRequestState::Merged => "merged",
        }
        .to_string(),
        labels: pull.labels.clone(),
        source_branch: pull.source.branch.clone(),
        target_branch: pull.target.branch.clone(),
        head_sha: pull.head_sha.clone(),
        base_sha: pull.base_sha.clone(),
        merged_sha: pull.merge.as_ref().map(|merge| merge.commit_sha.clone()),
    }
}
