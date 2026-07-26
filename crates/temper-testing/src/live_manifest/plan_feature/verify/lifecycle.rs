// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{Issue, PullRequest, PullRequestState};

use super::{Observations, has_label, issue_closed, merge_sha, require_sha};

pub(super) fn require_terminal_state(
    feature: &Issue,
    plan: &Issue,
    code_issues: [&Issue; 3],
    scenario_issue: &Issue,
    implementation_prs: [&PullRequest; 3],
    scenario_pr: &PullRequest,
    landing: &PullRequest,
    observations: &Observations,
) -> Result<(), String> {
    if !observations.second_blocked || !observations.second_unblocked_after_first_closed {
        return Err(
            "sequential product dependency block/unblock was not fully observed".to_string(),
        );
    }
    if !observations.scenario_blocked || !observations.scenario_unblocked_after_products_closed {
        return Err(
            "scenario child did not remain blocked until every product child landed".to_string(),
        );
    }
    if !code_issues.into_iter().all(issue_closed) {
        return Err("implementation issues are not all closed yet".to_string());
    }
    if !issue_closed(scenario_issue) {
        return Err("scenario-authoring issue is not closed yet".to_string());
    }
    if !matches!(scenario_pr.state, PullRequestState::Merged) {
        return Err("scenario PR is not merged yet".to_string());
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

pub(super) fn require_merge_topology(
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

pub(super) fn require_validation_order(
    initial: [&PullRequest; 2],
    scenario: &Issue,
    scenario_pr: &PullRequest,
    followup: &Issue,
    followup_pr: &PullRequest,
    landing: &PullRequest,
) -> Result<(), String> {
    if !has_label(scenario, "validation") {
        return Err("scenario child lost its validation identity".to_string());
    }
    let scenario_merged_at = scenario_pr
        .merge
        .as_ref()
        .map(|merge| merge.merged_at)
        .ok_or_else(|| "scenario PR has no merge time".to_string())?;
    for pull in initial {
        let merged_at = pull
            .merge
            .as_ref()
            .map(|merge| merge.merged_at)
            .ok_or_else(|| format!("implementation PR #{} has no merge time", pull.number))?;
        if scenario_merged_at < merged_at {
            return Err(format!(
                "scenario PR #{} merged before product PR #{}",
                scenario_pr.number, pull.number
            ));
        }
        if followup.created_at < scenario_merged_at {
            return Err(format!(
                "validation follow-up #{} was created before scenario PR #{} merged",
                followup.number, scenario_pr.number
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
