// SPDX-License-Identifier: MPL-2.0

use temper_forge_model::{ItemNumber, PullRequestQuery};

use super::super::failure_evidence::FailureEvidenceServer;
use super::super::process::{engine_block_on, read_effective_configuration, write_snapshot};
use super::super::{CiRequestEvidence, LiveManifestEvidence};
use super::{LiveExecutionContext, required_mut, required_ref};

impl LiveExecutionContext<'_> {
    pub(super) fn finish(mut self) -> Result<LiveManifestEvidence, String> {
        let convergence = self
            .convergence
            .take()
            .ok_or_else(|| "execution ended without workflow.wait_convergence".to_string())?;
        let fake = self
            .fake
            .take()
            .ok_or_else(|| "execution ended without jig.fake_llm".to_string())?;
        write_snapshot(&self.logs.fake_llm_log, &fake.log_tail());
        if self.terminal_history.is_none() {
            if let (Some(forge), Some(repository)) = (&self.forge, &self.repository) {
                write_snapshot(
                    &self.logs.ci_diagnostics_log,
                    &super::super::convergence::ci_diagnostics(forge, repository),
                );
            }
        } else if let Some(history) = &self.terminal_history {
            write_snapshot(
                &self.logs.ci_diagnostics_log,
                &format!(
                    "terminal-history scenario: actionable PR #{} recovered={} cold_authority_rebuilt={}; per-artifact CI diagnostics intentionally omitted\n",
                    history.actionable_pull_request_number,
                    history.actionable_recovered,
                    history.cold_authority_rebuilt
                ),
            );
        }
        if let Some(standalone) = &mut self.standalone {
            standalone.kill();
        }
        let runner_running = required_mut(&mut self.runner, "forgejo_runner.ready")?.is_running();
        let fake_llm = fake.evidence(&self.logs.fake_llm_log);
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        let mut forge_pull_requests = if let Some(history) = &self.terminal_history {
            vec![
                engine_block_on(forge.get_pull_request_by_number(
                    repository,
                    ItemNumber::new(history.actionable_pull_request_number),
                ))
                .map_err(|error| format!("capture actionable terminal pull request: {error}"))?
                .ok_or_else(|| "actionable terminal pull request disappeared".to_string())?,
            ]
        } else {
            engine_block_on(forge.list_pull_requests(repository, PullRequestQuery::default()))
                .map_err(|error| format!("capture terminal pull-request inventory: {error}"))?
        }
        .iter()
        .map(super::super::convergence::pr_evidence)
        .collect::<Vec<_>>();
        forge_pull_requests.sort_by_key(|pull| pull.number);
        let ci_request_provenance = forge
            .request_provenance()
            .ok_or_else(|| "live Forge request provenance recorder was not enabled".to_string())?;
        let ci_request_capture_dropped = ci_request_provenance.dropped;
        let mut ci_requests: Vec<CiRequestEvidence> = ci_request_provenance
            .requests
            .into_iter()
            .map(|request| CiRequestEvidence {
                method: request.method.to_string(),
                path: request.path,
                query_keys: request.query_keys,
                authentication_present: request.authentication_present,
                authentication_scheme: request.authentication_scheme,
                accepts_json: request.accepts_json,
            })
            .collect();
        let actions_history = self.actions_history.take().map(|mut capture| {
            ci_requests.append(&mut capture.requests);
            capture.evidence.provenance_drop_count = ci_request_capture_dropped;
            capture.evidence
        });
        let ci_failure_evidence = self
            .failure_evidence
            .as_ref()
            .map(FailureEvidenceServer::evidence);
        if let Some(evidence) = &ci_failure_evidence {
            ci_requests.extend(evidence.requests.iter().cloned());
        }
        Ok(LiveManifestEvidence {
            _workspace: self.workspace,
            scenario_path: self.harness.scenario.scenario_path.clone(),
            manifest_path: self.harness.scenario.manifest_path.clone(),
            scenario_run_id: self.scenario_run_id,
            temper_log_format: self.harness.scenario.observability.log_format.clone(),
            rust_log: self.harness.scenario.observability.rust_log.clone(),
            temper_binary: self.harness.temper.binary().to_path_buf(),
            forge_url: required_ref(&self.server, "forgejo.provision")?
                .base_url()
                .to_string(),
            repo_slug: self.harness.scenario.repo.slug.clone(),
            repo_id: self.harness.scenario.repo.id.clone(),
            repo_default_branch: self.harness.scenario.repo.default_branch.clone(),
            forge_cache_hit: self.forge_cache_hit,
            runner_running,
            startup: self.started.elapsed().saturating_sub(convergence.elapsed),
            convergence: convergence.elapsed,
            total_elapsed: self.started.elapsed(),
            poll_backstop: self.harness.scenario.poll_backstop,
            effective_configuration: read_effective_configuration(
                &self.bundle_dir.join("config.toml"),
            )?,
            fake_llm,
            forge_pull_requests,
            final_state: convergence.final_state,
            ci_requests,
            ci_request_capture_dropped,
            ci_failure_evidence,
            handoff: convergence.handoff,
            codebase_memory: convergence.codebase_memory,
            plan_feature: convergence.plan_feature,
            terminal_history: self.terminal_history,
            actions_history,
            stimuli: self.stimuli,
            logs: self.logs,
        })
    }
}
