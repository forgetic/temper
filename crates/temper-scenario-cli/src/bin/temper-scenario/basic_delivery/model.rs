// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{IssueState, ItemNumber, PullRequestState};
use temper_runner::RunReport;

use crate::run_evidence;

#[derive(Debug)]
pub(super) struct RunOutcome {
    pub(super) scenario_name: String,
    pub(super) evidence: RunEvidence,
    pub(super) report: RunReport,
}

#[derive(Debug)]
pub(super) struct RunEvidence {
    pub(super) issue_number: ItemNumber,
    pub(super) issue_title: String,
    pub(super) issue_state: IssueState,
    pub(super) issue_labels: Vec<String>,
    pub(super) pr_number: ItemNumber,
    pub(super) pr_title: String,
    pub(super) pr_state: PullRequestState,
    pub(super) pr_labels: Vec<String>,
    pub(super) pr_head_branch: String,
    pub(super) pr_head_sha: Option<String>,
    pub(super) pr_merged_sha: Option<String>,
    pub(super) repo_id: String,
    pub(super) repo_slug: String,
    pub(super) default_branch: String,
    pub(super) default_branch_head_sha: Option<String>,
    pub(super) default_branch_contains_engineer_diff: bool,
    pub(super) completed_ci_jobs: usize,
    pub(super) ci_jobs: Vec<run_evidence::CiJobEvidence>,
    pub(super) closed_parent_issues: usize,
}
