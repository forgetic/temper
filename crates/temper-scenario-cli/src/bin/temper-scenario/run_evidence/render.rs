// SPDX-License-Identifier: MPL-2.0

use std::path::Path;

use super::model::{
    ConvergenceEvidence, FinalStateEvidence, ObservabilityEvidence, ProviderEvidence,
    RunEvidenceArtifact, TopologyEvidence,
};

impl RunEvidenceArtifact {
    pub(crate) fn report_details(&self, path: &Path) -> Vec<String> {
        let mut details = vec![
            format!("run evidence artifact: `{}`", path.display()),
            format!("schema: `{}` version {}", self.schema, self.version),
            format!("verdict: `{}`", self.verdict.as_str()),
            format!("scenario: `{}`", self.scenario.name),
            format!("source: {}", self.scenario.source_description),
            format!("manifest: `{}`", self.scenario.manifest_path),
            format!(
                "execution topology: {} ({})",
                self.scenario.tier, self.scenario.tier_description
            ),
            self.scenario.runner_selection.clone(),
        ];
        for (field, value) in [
            ("feature", self.scenario.feature.as_deref()),
            ("plan", self.scenario.plan.as_deref()),
            ("mapping_id", self.scenario.mapping_id.as_deref()),
            ("mapped_scenario", self.scenario.mapped_scenario.as_deref()),
            ("source_branch", self.scenario.source_branch.as_deref()),
            (
                "checkout_head_sha",
                self.scenario.checkout_head_sha.as_deref(),
            ),
            (
                "resolved_content_digest",
                self.scenario.resolved_content_digest.as_deref(),
            ),
        ] {
            if let Some(value) = value {
                details.push(format!("scenario {field}: `{value}`"));
            }
        }
        if let Some(binary) = &self.binary {
            details.push(format!(
                "standalone binary: `{}` sha256={} size_bytes={}",
                binary.path, binary.sha256, binary.size_bytes
            ));
        }
        if let Some(execution) = &self.execution {
            details.push(format!(
                "execution: status={} total_duration_ms={}",
                execution.status, execution.total_duration_ms
            ));
            if let Some(failure) = execution.failure.as_deref() {
                details.push(format!("execution failure: {failure}"));
            }
        }
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
        if let Some(configuration) = &self.effective_configuration {
            details.push(format!(
                "effective standalone configuration: ci_poll_cadence_secs={} poll_cadence_secs={} mechanical_cadence_secs={}",
                configuration.ci_poll_cadence_secs,
                configuration.poll_cadence_secs,
                configuration.mechanical_cadence_secs
            ));
        }
        if let Some(convergence) = &self.convergence {
            details.extend(convergence_details(convergence));
        }
        if let Some(provider) = &self.provider {
            details.extend(provider_details(provider));
        }
        if let Some(observability) = &self.observability {
            details.extend(observability_details(observability));
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
        for stimulus in &self.stimuli {
            details.push(format!(
                "stimulus `{}`: action={} status={} attempts={} timeout_ms={} duration_ms={}",
                stimulus.id,
                stimulus.action,
                stimulus.status,
                stimulus.attempts,
                stimulus.timeout_ms,
                stimulus.duration_ms
            ));
            details.extend(
                stimulus
                    .details
                    .iter()
                    .map(|detail| format!("stimulus `{}` detail: {detail}", stimulus.id)),
            );
        }
        for limitation in &self.limitations {
            details.push(format!("limitation: {limitation}"));
        }
        if let Some(intent) = self.follow_up_intent.as_deref() {
            details.push(format!("follow-up intent: {intent}"));
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
    for repository in &final_state.repositories {
        let id = repository.id.as_deref().unwrap_or("unknown");
        let slug = repository.slug.as_deref().unwrap_or("unknown");
        details.push(format!("final repo: id={id} slug={slug}"));
        for branch in &repository.branches {
            let mut detail = format!("final repo branch: repo={id} name={}", branch.name);
            if let Some(head_sha) = branch.head_sha.as_deref() {
                detail.push_str(&format!(" head_sha={head_sha}"));
            }
            if let Some(contains) = branch.contains_engineer_diff {
                detail.push_str(&format!(" contains_engineer_diff={contains}"));
            }
            details.push(detail);
        }
    }
    if let Some(completed_jobs) = final_state.ci.completed_jobs {
        details.push(format!("final CI: {completed_jobs} completed job(s)"));
    }
    for job in &final_state.ci.jobs {
        details.push(format!(
            "final CI job: name={} status={} pr={:?} conclusion={:?} url={:?}",
            job.name, job.status, job.pull_request_number, job.conclusion, job.url
        ));
        if let Some(proof) = &job.verified_failure {
            details.push(format!(
                "verified CI failure proof: category={} repository={} pull_request={:?} commit={} run={} job={} attempt={} task={:?} producer={} issuer={} verification={}",
                proof.category,
                proof.repository_id,
                proof.pull_request_id,
                proof.commit_sha,
                proof.run_id,
                proof.job_id,
                proof.attempt,
                proof.task_id,
                proof.producer_id,
                proof.issuer_id,
                proof.verification
            ));
        }
    }
    for head in &final_state.ci.heads {
        details.push(format!(
            "CI head: phase={} sha={} observed_after_ms={} observations={}",
            head.phase,
            head.head_sha,
            head.observed_after_ms,
            head.observations.len()
        ));
        for job in &head.jobs {
            details.push(format!(
                "CI head job: phase={} name={} status={} run={:?} attempt={:?} commit={:?} conclusion={:?} provider_conclusion={:?}",
                head.phase,
                job.name,
                job.status,
                job.provider_run_id,
                job.provider_attempt,
                job.commit_sha,
                job.conclusion,
                job.provider_conclusion
            ));
            if let Some(proof) = &job.verified_failure {
                details.push(format!(
                    "CI head verified failure: phase={} category={} run={} job={} attempt={} task={:?} producer={} issuer={} verification={}",
                    head.phase,
                    proof.category,
                    proof.run_id,
                    proof.job_id,
                    proof.attempt,
                    proof.task_id,
                    proof.producer_id,
                    proof.issuer_id,
                    proof.verification
                ));
            }
        }
    }
    if let Some(service) = &final_state.ci.failure_evidence {
        details.push(format!(
            "CI failure evidence service: path={} issuer={} protected_producers={:?} published_proofs={}",
            service.endpoint_path,
            service.issuer,
            service.protected_producers,
            service.published_proofs
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
        ("request_log_path", provider.request_log_path.as_deref()),
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
    if let Some(request_count) = provider.request_count {
        details.push(format!("provider Jig request_count: {request_count}"));
    }
    for (role, count) in &provider.request_counts_by_role {
        details.push(format!("provider Jig requests: role={role} count={count}"));
    }
    for script in &provider.jig_script_paths {
        details.push(format!("provider Jig script: `{script}`"));
    }
    details
}

fn observability_details(observability: &ObservabilityEvidence) -> Vec<String> {
    let mut details = vec![
        format!(
            "observability scenario_run_id: `{}`",
            observability.scenario_run_id
        ),
        format!(
            "observability capture: TEMPER_LOG_FORMAT={} RUST_LOG={}",
            observability.log_format, observability.rust_log
        ),
        format!(
            "observability event log: `{}`",
            observability.event_log_path
        ),
        format!(
            "observability retained event logs: {:?}",
            observability.event_log_paths
        ),
        format!(
            "observability events captured: {}",
            observability.captured_events
        ),
    ];
    for event in observability.events.iter().take(12) {
        let artifact = event
            .fields
            .get("artifact.ref")
            .or_else(|| event.fields.get("pr.ref"))
            .map(|value| format!(" artifact={value}"))
            .unwrap_or_default();
        details.push(format!(
            "observability event #{}: {}{}",
            event.sequence, event.event, artifact
        ));
    }
    if observability.events.len() > 12 {
        details.push(format!(
            "observability event sample truncated: {} additional event(s)",
            observability.events.len() - 12
        ));
    }
    details
}
