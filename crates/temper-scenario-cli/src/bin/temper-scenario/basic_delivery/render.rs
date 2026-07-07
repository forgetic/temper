// SPDX-License-Identifier: MPL-2.0

use temper_runner::RunReport;

use crate::run_context::ScenarioRunFacts;
use crate::run_evidence;

use super::model::RunOutcome;
use super::state::{issue_state_value, issue_state_word, pr_state_evidence, pr_state_value};

pub(super) fn print_outcome(outcome: &RunOutcome, facts: &ScenarioRunFacts) {
    println!("scenario: {}", outcome.scenario_name);
    facts.print_stdout();
    println!("verdict: passed");
    println!("evidence:");
    for line in outcome_evidence_lines(outcome) {
        println!("  {line}");
    }
}

pub(super) fn outcome_evidence_lines(outcome: &RunOutcome) -> Vec<String> {
    vec![
        format!(
            "seeded issue: #{} \"{}\" {} as code",
            outcome.evidence.issue_number,
            outcome.evidence.issue_title,
            issue_state_word(outcome.evidence.issue_state)
        ),
        format!(
            "implementation PR: #{} {} with passing CI ({} completed job(s))",
            outcome.evidence.pr_number,
            pr_state_evidence(outcome.evidence.pr_state),
            outcome.evidence.completed_ci_jobs
        ),
        format!(
            "closed parent issues: {}",
            outcome.evidence.closed_parent_issues
        ),
        format!("actions: {}", action_counts(&outcome.report)),
        format!(
            "report: ticks={} workers={}",
            outcome.report.ticks,
            outcome.report.workers.len()
        ),
    ]
}

pub(super) fn outcome_artifact(
    outcome: &RunOutcome,
    context: &run_evidence::RunEvidenceContext,
) -> run_evidence::RunEvidenceArtifact {
    let mut artifact = context.artifact(run_evidence::FinalStateEvidence {
        issues: vec![run_evidence::IssueStateEvidence {
            number: outcome.evidence.issue_number.get(),
            id: Some("intake".to_string()),
            title: Some(outcome.evidence.issue_title.clone()),
            state: Some(issue_state_value(outcome.evidence.issue_state).to_string()),
            labels: outcome.evidence.issue_labels.clone(),
        }],
        pull_requests: vec![run_evidence::PullRequestStateEvidence {
            number: outcome.evidence.pr_number.get(),
            id: Some("implementation".to_string()),
            title: Some(outcome.evidence.pr_title.clone()),
            body: None,
            state: Some(pr_state_value(outcome.evidence.pr_state).to_string()),
            labels: outcome.evidence.pr_labels.clone(),
            head_branch: Some(outcome.evidence.pr_head_branch.clone()),
            head_sha: outcome.evidence.pr_head_sha.clone(),
            merged_sha: outcome.evidence.pr_merged_sha.clone(),
        }],
        repositories: vec![run_evidence::RepositoryStateEvidence {
            id: Some(outcome.evidence.repo_id.clone()),
            slug: Some(outcome.evidence.repo_slug.clone()),
            branches: vec![run_evidence::RepositoryBranchStateEvidence {
                name: outcome.evidence.default_branch.clone(),
                head_sha: outcome.evidence.default_branch_head_sha.clone(),
                contains_engineer_diff: Some(
                    outcome.evidence.default_branch_contains_engineer_diff,
                ),
            }],
        }],
        ci: run_evidence::CiStateEvidence {
            completed_jobs: Some(outcome.evidence.completed_ci_jobs),
            jobs: outcome.evidence.ci_jobs.clone(),
        },
    });
    artifact.convergence = Some(run_evidence::ConvergenceEvidence {
        ticks: Some(outcome.report.ticks),
        workers: outcome
            .report
            .workers
            .iter()
            .map(|worker| run_evidence::WorkerTickEvidence {
                name: worker.name.clone(),
                ticks: worker.ticks,
                actions: worker.actions,
            })
            .collect(),
        ..run_evidence::ConvergenceEvidence::default()
    });
    artifact.evidence_lines = outcome_evidence_lines(outcome);
    artifact
}

fn action_counts(report: &RunReport) -> String {
    report
        .workers
        .iter()
        .map(|worker| format!("{}={}", worker.name, worker.actions))
        .collect::<Vec<_>>()
        .join(", ")
}
