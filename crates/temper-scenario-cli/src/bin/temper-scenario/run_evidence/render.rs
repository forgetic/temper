// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use super::model::{
    ConvergenceEvidence, FinalStateEvidence, ProviderEvidence, RunEvidenceArtifact,
    TopologyEvidence,
};

impl RunEvidenceArtifact {
    pub(crate) fn report_details(&self, path: &Path) -> Vec<String> {
        let mut details = vec![
            format!("run evidence artifact: `{}`", path.display()),
            format!("schema: `{}` version {}", self.schema, self.version),
            format!("scenario: `{}`", self.scenario.name),
            format!("source: {}", self.scenario.source_description),
            format!("manifest: `{}`", self.scenario.manifest_path),
            format!(
                "confidence tier: {} ({})",
                self.scenario.tier, self.scenario.tier_description
            ),
            self.scenario.runner_selection.clone(),
        ];
        if self.scenario.topology.is_empty() {
            details.push("manifest topology: not declared".to_string());
        } else {
            details.extend(
                self.scenario
                    .topology
                    .field_values()
                    .into_iter()
                    .map(|(field, value)| format!("manifest topology.{field}: `{value}`")),
            );
        }
        for fixture in &self.fixtures {
            details.push(format!(
                "fixture {}: `{}` -> `{}`",
                fixture.field, fixture.value, fixture.resolved_path
            ));
        }
        details.extend(final_state_details(&self.final_state));
        if let Some(convergence) = &self.convergence {
            details.extend(convergence_details(convergence));
        }
        if let Some(provider) = &self.provider {
            details.extend(provider_details(provider));
        }
        for path in &self.artifacts.log_paths {
            details.push(format!("log path: `{path}`"));
        }
        for path in &self.artifacts.artifact_paths {
            details.push(format!("artifact path: `{path}`"));
        }
        for line in &self.evidence_lines {
            details.push(format!("runner evidence: {line}"));
        }
        if let Some(assertions) = &self.assertions {
            details.extend(assertions.report_details());
        }
        details
    }
}

impl TopologyEvidence {
    fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.forge.is_none()
            && self.runner.is_none()
            && self.temper.is_none()
            && self.agent_model.is_none()
    }

    fn field_values(&self) -> Vec<(&'static str, &str)> {
        [
            ("kind", self.kind.as_deref()),
            ("forge", self.forge.as_deref()),
            ("runner", self.runner.as_deref()),
            ("temper", self.temper.as_deref()),
            ("agent_model", self.agent_model.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name, value)))
        .collect()
    }
}

fn final_state_details(final_state: &FinalStateEvidence) -> Vec<String> {
    let mut details = Vec::new();
    for issue in &final_state.issues {
        let state = issue.state.as_deref().unwrap_or("unknown");
        let title = issue.title.as_deref().unwrap_or("untitled");
        details.push(format!(
            "final issue: #{} `{}` state={} labels={:?}",
            issue.number, title, state, issue.labels
        ));
        if let Some(id) = issue.id.as_deref() {
            details.push(format!(
                "final issue id: #{number} -> `{id}`",
                number = issue.number
            ));
        }
    }
    for pull_request in &final_state.pull_requests {
        let state = pull_request.state.as_deref().unwrap_or("unknown");
        let title = pull_request.title.as_deref().unwrap_or("untitled");
        let mut detail = format!(
            "final PR: #{} `{}` state={} labels={:?}",
            pull_request.number, title, state, pull_request.labels
        );
        if let Some(head_branch) = pull_request.head_branch.as_deref() {
            detail.push_str(&format!(" head={head_branch}"));
        }
        if let Some(head_sha) = pull_request.head_sha.as_deref() {
            detail.push_str(&format!(" head_sha={head_sha}"));
        }
        if let Some(merged_sha) = pull_request.merged_sha.as_deref() {
            detail.push_str(&format!(" merged_sha={merged_sha}"));
        }
        if let Some(id) = pull_request.id.as_deref() {
            detail.push_str(&format!(" id={id}"));
        }
        details.push(detail);
    }
    if let Some(completed_jobs) = final_state.ci.completed_jobs {
        details.push(format!("final CI: {completed_jobs} completed job(s)"));
    }
    for job in &final_state.ci.jobs {
        details.push(format!(
            "final CI job: name={} status={} pr={:?} conclusion={:?} url={:?}",
            job.name, job.status, job.pull_request_number, job.conclusion, job.url
        ));
    }
    details
}

fn convergence_details(convergence: &ConvergenceEvidence) -> Vec<String> {
    let mut details = Vec::new();
    if let Some(ticks) = convergence.ticks {
        details.push(format!("convergence ticks: {ticks}"));
    }
    for worker in &convergence.workers {
        details.push(format!(
            "convergence worker: {} ticks={} actions={}",
            worker.name, worker.ticks, worker.actions
        ));
    }
    for (field, value) in [
        ("startup_ms", convergence.startup_ms),
        ("convergence_ms", convergence.convergence_ms),
        ("poll_backstop_ms", convergence.poll_backstop_ms),
        ("total_elapsed_ms", convergence.total_elapsed_ms),
    ] {
        if let Some(value) = value {
            details.push(format!("convergence {field}: {value}"));
        }
    }
    details
}

fn provider_details(provider: &ProviderEvidence) -> Vec<String> {
    let mut details = Vec::new();
    for (field, value) in [
        ("forgejo_url", provider.forgejo_url.as_deref()),
        ("repo_slug", provider.repo_slug.as_deref()),
        ("head_branch", provider.head_branch.as_deref()),
        ("merged_sha", provider.merged_sha.as_deref()),
        ("temper_binary", provider.temper_binary.as_deref()),
        ("fake_llm_url", provider.fake_llm_url.as_deref()),
    ] {
        if let Some(value) = value {
            details.push(format!("provider {field}: `{value}`"));
        }
    }
    if let Some(issue_number) = provider.issue_number {
        details.push(format!("provider issue_number: #{issue_number}"));
    }
    if let Some(pr_number) = provider.pr_number {
        details.push(format!("provider pr_number: #{pr_number}"));
    }
    details
}
