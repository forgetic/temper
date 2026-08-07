use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{CreateIssue, ItemNumber, PullRequestQuery, RepositoryId};
use temper_workflow::parse_metadata_block;

use super::codebase_memory::{FakeMcpServer, ToolConfiguration};
use super::failure_evidence::FailureEvidenceServer;
use super::process::{
    ChildGuard, TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout,
    engine_block_on, free_port, mint_site_admin_token, populate_repo, run_temper_init,
    spawn_temper_standalone, tune_init_config, wait_for_standalone, write_snapshot,
};
use super::runtime_fake::ManifestFake;
use super::{
    ConvergenceStrategy, FinalStateEvidence, LiveCodebaseMemoryEvidence, LiveHandoffEvidence,
    LiveLogPaths, LiveManifestEvidence, LiveManifestHarness, LivePlanFeatureEvidence,
    LiveTerminalHistoryEvidence, ManifestAction, ManifestStep, StimulusKind, StimulusOutcome,
};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, ForgejoServer, start_cached_bare_admin_server};

#[path = "runtime_actions.rs"]
mod actions;
#[path = "runtime/finish.rs"]
mod finish;

pub(super) fn execute(harness: &LiveManifestHarness) -> Result<LiveManifestEvidence, String> {
    let mut runtime = LiveExecutionContext::new(harness);
    let steps = harness.scenario.execution.steps.clone();
    let result = walk_steps(&steps, |step| {
        runtime.execute_step(step).map_err(|error| {
            format!(
                "manifest step `{}` ({}) failed: {error}",
                step.id,
                action_name(&step.action)
            )
        })
    });
    if let Err(error) = result {
        runtime.retain_failure_artifacts();
        return Err(error);
    }
    runtime.finish()
}

pub(super) fn walk_steps(
    steps: &[ManifestStep],
    mut handler: impl FnMut(&ManifestStep) -> Result<(), String>,
) -> Result<(), String> {
    for step in steps {
        handler(step)?;
    }
    Ok(())
}

struct LiveExecutionContext<'a> {
    harness: &'a LiveManifestHarness,
    started: Instant,
    workspace: RunWorkspace,
    bundle_dir: PathBuf,
    workspaces_dir: PathBuf,
    logs: LiveLogPaths,
    scenario_run_id: String,
    server: Option<ForgejoServer>,
    forge_cache_hit: bool,
    admin_token: Option<String>,
    runner: Option<ForgejoRunner>,
    fake: Option<ManifestFake>,
    jig_roles: BTreeSet<String>,
    mcp: Option<FakeMcpServer>,
    tool_configuration: Option<ToolConfiguration>,
    forge: Option<ForgejoForge>,
    repository: Option<RepositoryId>,
    standalone: Option<ChildGuard>,
    failure_evidence: Option<FailureEvidenceServer>,
    issues: BTreeMap<String, ItemNumber>,
    issue_order: Vec<ItemNumber>,
    initial_default_branch_sha: Option<String>,
    stimuli: Vec<StimulusOutcome>,
    convergence: Option<ConvergenceOutput>,
    terminal_history: Option<LiveTerminalHistoryEvidence>,
}

struct ConvergenceOutput {
    elapsed: Duration,
    final_state: FinalStateEvidence,
    handoff: Option<LiveHandoffEvidence>,
    codebase_memory: Option<LiveCodebaseMemoryEvidence>,
    plan_feature: Option<LivePlanFeatureEvidence>,
}

impl<'a> LiveExecutionContext<'a> {
    fn new(harness: &'a LiveManifestHarness) -> Self {
        let workspace = RunWorkspace::new(&harness.workspace_prefix);
        let bundle_dir = workspace.dir("bundle");
        let workspaces_dir = workspace.dir("workspaces");
        let logs = LiveLogPaths {
            workspace_root: workspace.path().to_path_buf(),
            init_log: workspace.join("logs/init.log"),
            repo_populate_log: workspace.join("logs/repo-populate.log"),
            standalone_log: workspace.join("logs/standalone.log"),
            fake_llm_log: workspace.join("logs/fake-llm.log"),
            ci_diagnostics_log: workspace.join("logs/ci-diagnostics.log"),
        };
        Self {
            harness,
            started: Instant::now(),
            scenario_run_id: super::scenario_run_id(&harness.scenario),
            workspace,
            bundle_dir,
            workspaces_dir,
            logs,
            server: None,
            forge_cache_hit: false,
            admin_token: None,
            runner: None,
            fake: None,
            jig_roles: BTreeSet::new(),
            mcp: None,
            tool_configuration: None,
            forge: None,
            repository: None,
            standalone: None,
            failure_evidence: None,
            issues: BTreeMap::new(),
            issue_order: Vec::new(),
            initial_default_branch_sha: None,
            stimuli: Vec::new(),
            convergence: None,
            terminal_history: None,
        }
    }

    fn execute_step(&mut self, step: &ManifestStep) -> Result<(), String> {
        match &step.action {
            ManifestAction::ProvisionForgejo => self.provision_forgejo(),
            ManifestAction::AwaitForgejoRunner => self.await_runner(),
            ManifestAction::SeedRepository {
                repo_id,
                seed_path,
                ci_source_path,
            } => self.seed_repository(repo_id, seed_path, ci_source_path),
            ManifestAction::StartJig {
                script_path,
                roles,
                late_stream_failure,
            } => self.start_jig(script_path, roles, late_stream_failure.as_ref()),
            ManifestAction::LaunchTemper { workflow_path } => self.launch_temper(workflow_path),
            ManifestAction::SeedIssue {
                issue_id,
                repo_id,
                binding,
                after_pr_binding,
            } => self.seed_issue(
                issue_id,
                repo_id,
                binding.as_deref(),
                after_pr_binding.as_deref(),
            ),
            ManifestAction::SeedTerminalHistory { fixture } => self.seed_terminal_history(fixture),
            ManifestAction::SeedPullRequest {
                repo_id,
                source_issue_id,
                title,
                body,
                metadata_kind,
                correlation_key,
            } => self.seed_pull_request(
                repo_id,
                source_issue_id,
                title,
                body,
                metadata_kind,
                correlation_key,
            ),
            ManifestAction::StartCodebaseMemoryMcp {
                project,
                safe_tools,
                hidden_tools,
                readiness_delay_ms,
                forced_systemic_failure,
            } => self.start_mcp(
                project,
                safe_tools,
                hidden_tools,
                *readiness_delay_ms,
                forced_systemic_failure.as_ref(),
            ),
            ManifestAction::ConfigureAgentTools {
                role,
                tool,
                mode,
                index,
                tool_timeout_secs,
                server_step,
            } => {
                self.configure_agent_tools(role, tool, mode, index, *tool_timeout_secs, server_step)
            }
            ManifestAction::Stimulus(stimulus) => self.execute_stimulus(stimulus),
            ManifestAction::WaitForConvergence { strategy } => self.converge(*strategy),
        }
    }

    fn seed_issue(
        &mut self,
        issue_id: &str,
        repo_id: &str,
        binding: Option<&str>,
        after_pr_binding: Option<&str>,
    ) -> Result<(), String> {
        if let Some(binding) = after_pr_binding {
            self.wait_for_implementation_pr(binding)?;
        }
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        require(
            repo_id == self.harness.scenario.repo.id,
            &format!("issue.seed references unavailable repository `{repo_id}`"),
        )?;
        let fixture = self.harness.scenario.issue(issue_id)?;
        require(
            fixture.repo_id == repo_id,
            &format!(
                "issue fixture `{issue_id}` belongs to `{}`, not `{repo_id}`",
                fixture.repo_id
            ),
        )?;
        let key = binding.unwrap_or(issue_id);
        require(
            !self.issues.contains_key(key),
            &format!("issue binding `{key}` has already been seeded"),
        )?;
        let has_seeded_pr = self.harness.scenario.execution.steps.iter().any(|step| {
            matches!(
                &step.action,
                ManifestAction::SeedPullRequest { source_issue_id, .. }
                    if source_issue_id == key || source_issue_id == issue_id
            )
        });
        let mut labels = fixture.labels.clone();
        if has_seeded_pr {
            labels.retain(|label| label != "ready");
        }
        let issue = engine_block_on(forge.create_issue(
            repository,
            CreateIssue {
                title: fixture.title.clone(),
                body: fixture.body.clone(),
                labels,
                assignees: Vec::new(),
            },
        ))
        .map_err(|error| format!("create issue fixture `{issue_id}` failed: {error}"))?;
        self.issues.insert(key.to_string(), issue.number);
        if key != issue_id {
            self.issues.insert(issue_id.to_string(), issue.number);
        }
        self.issue_order.push(issue.number);
        Ok(())
    }

    fn wait_for_implementation_pr(&mut self, binding: &str) -> Result<(), String> {
        let issue = self
            .issues
            .get(binding)
            .copied()
            .ok_or_else(|| format!("issue binding `{binding}` has not been seeded"))?;
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        let standalone = required_mut(&mut self.standalone, "temper.launch_standalone")?;
        let deadline = Instant::now() + convergence_timeout(self.harness.scenario.timeout);
        super::convergence::poll_until(deadline, standalone, || {
            engine_block_on(async {
                let pulls = forge
                    .list_pull_requests(repository, PullRequestQuery::default())
                    .await
                    .map_err(|error| format!("list implementation PRs: {error}"))?;
                pulls
                    .iter()
                    .find(|pull| {
                        pull.labels.iter().any(|label| label == "implementation")
                            && parse_metadata_block(&pull.body).ok().flatten().is_some_and(
                                |metadata| {
                                    metadata.parents.iter().any(|parent| {
                                        parent.is_same_repo() && parent.number == issue
                                    })
                                },
                            )
                    })
                    .map(|_| ())
                    .ok_or_else(|| {
                        format!("issue binding `{binding}` has no observed implementation PR yet")
                    })
            })
        })
    }

    fn seed_pull_request(
        &mut self,
        repo_id: &str,
        source_issue_id: &str,
        title: &str,
        body: &str,
        metadata_kind: &str,
        correlation_key: &str,
    ) -> Result<(), String> {
        require(
            repo_id == self.harness.scenario.repo.id,
            &format!("pr.seed_existing references unavailable repository `{repo_id}`"),
        )?;
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        let issue = self.issues.get(source_issue_id).copied().ok_or_else(|| {
            format!("source issue binding `{source_issue_id}` has not been seeded")
        })?;
        require(
            correlation_key == format!("$correlation:{source_issue_id}"),
            &format!(
                "pr.seed_existing correlation `{correlation_key}` does not reference source binding `{source_issue_id}`"
            ),
        )?;
        let pull = engine_block_on(super::handoff::seed_existing_pr(
            forge,
            repository,
            &self.harness.scenario.repo.default_branch,
            issue,
            title,
            body,
            metadata_kind,
        ))?;
        engine_block_on(super::handoff::mark_issue_ready(
            forge,
            &self.harness.scenario.repo.slug,
            issue,
        ))?;
        if let Some(fake) = &self.fake {
            fake.wait_for_handoff_refresh(Duration::from_secs(60))?;
        }
        engine_block_on(super::handoff::mark_stale_pr_as_implementation(
            forge, &pull,
        ))?;
        if let Some(fake) = &self.fake {
            fake.allow_handoff_refresh();
        }
        Ok(())
    }

    fn execute_stimulus(&mut self, stimulus: &super::StimulusSpec) -> Result<(), String> {
        let issue = match &stimulus.kind {
            StimulusKind::RepeatDelivery { artifact, .. }
            | StimulusKind::WaitProviderDeferred { artifact, .. }
            | StimulusKind::ProviderHealthWake { artifact, .. } => artifact
                .strip_prefix("issue:")
                .and_then(|binding| self.issues.get(binding))
                .copied()
                .ok_or_else(|| format!("stimulus references unseeded artifact `{artifact}`"))?,
            _ => self.primary_issue()?,
        };
        let result = super::stimuli::execute_live_stimuli(
            std::slice::from_ref(stimulus),
            super::stimuli::LiveStimulusResources {
                scenario: &self.harness.scenario,
                server: required_ref(&self.server, "forgejo.provision")?,
                runner: required_mut(&mut self.runner, "forgejo_runner.ready")?,
                temper: &self.harness.temper,
                bundle_dir: &self.bundle_dir,
                logs: &self.logs,
                scenario_run_id: &self.scenario_run_id,
                standalone: required_mut(&mut self.standalone, "temper.launch_standalone")?,
                forge: required_ref(&self.forge, "temper.launch_standalone")?,
                repository: required_ref(&self.repository, "temper.launch_standalone")?,
                issue,
            },
        )
        .map_err(|failure| failure.diagnostic())?;
        self.stimuli.extend(result);
        Ok(())
    }

    fn converge(&mut self, strategy: ConvergenceStrategy) -> Result<(), String> {
        require(self.convergence.is_none(), "convergence already ran")?;
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        let primary_issue = self.primary_issue()?;
        let timeout = convergence_timeout(self.harness.scenario.timeout);
        let mut standalone = self.standalone.take().ok_or_else(|| {
            "prerequisite action `temper.launch_standalone` has not completed".to_string()
        })?;
        let started = Instant::now();
        let result: Result<
            (
                FinalStateEvidence,
                Option<LiveHandoffEvidence>,
                Option<LiveCodebaseMemoryEvidence>,
                Option<LivePlanFeatureEvidence>,
            ),
            String,
        > = (|| {
            Ok(match strategy {
                ConvergenceStrategy::SinglePullRequest => (
                    super::convergence::drive_single_pull_request_convergence(
                        forge,
                        repository,
                        primary_issue,
                        &self.harness.admin_user,
                        &mut standalone,
                        timeout,
                    )?,
                    None,
                    None,
                    None,
                ),
                ConvergenceStrategy::ImplementationPrTerminalCi => (
                    super::convergence::drive_implementation_pr_terminal_ci_convergence(
                        forge,
                        repository,
                        primary_issue,
                        &self.harness.admin_user,
                        &mut standalone,
                        timeout,
                    )?,
                    None,
                    None,
                    None,
                ),
                ConvergenceStrategy::CiPollExactHeadRepair => (
                    super::convergence::drive_ci_poll_exact_head_repair_convergence(
                        forge,
                        repository,
                        primary_issue,
                        &self.harness.admin_user,
                        &mut standalone,
                        timeout,
                    )?,
                    None,
                    None,
                    None,
                ),
                ConvergenceStrategy::CodebaseMemory => {
                    let fake = required_ref(&self.fake, "jig.fake_llm")?.codebase()?;
                    let mcp = required_ref(&self.mcp, "mcp.fake_codebase_memory.start")?;
                    let (state, evidence) = super::codebase_memory::converge(
                        forge,
                        repository,
                        primary_issue,
                        &self.harness.admin_user,
                        &mut standalone,
                        timeout,
                        fake,
                        mcp,
                    )?;
                    (state, None, Some(evidence), None)
                }
                ConvergenceStrategy::ImplementationPrHandoff => {
                    let (state, evidence) = super::handoff::converge(
                        self.harness,
                        forge,
                        repository,
                        &mut standalone,
                        timeout,
                        &self.issues,
                    )?;
                    (state, Some(evidence), None, None)
                }
                ConvergenceStrategy::PlanFeatureLanding => {
                    let server = required_ref(&self.server, "forgejo.provision")?;
                    let token = required_ref(&self.admin_token, "forgejo.provision")?;
                    let initial_sha = required_ref(
                        &self.initial_default_branch_sha,
                        "repo.seed default-branch snapshot",
                    )?;
                    let fake = required_ref(&self.fake, "jig.fake_llm")?.plan_feature()?;
                    let (state, evidence) = super::plan_feature::converge(
                        self.harness,
                        forge,
                        repository,
                        primary_issue,
                        &mut standalone,
                        timeout,
                        initial_sha,
                        server.base_url(),
                        token,
                        fake,
                    )?;
                    (state, None, None, Some(evidence))
                }
                ConvergenceStrategy::HistoryIndependentTerminalRecovery => {
                    let history = self.terminal_history.as_ref().ok_or_else(|| {
                        "history-independent convergence requires history.seed_terminal evidence"
                            .to_string()
                    })?;
                    let state = super::terminal_history::converge(
                        forge,
                        repository,
                        ItemNumber::new(history.actionable_issue_number),
                        ItemNumber::new(history.actionable_pull_request_number),
                        &mut standalone,
                        timeout,
                        &self.logs.standalone_log,
                    )?;
                    let (actionable_recovered, cold_authority_rebuilt) =
                        super::terminal_history::recovery_observations(&self.logs.standalone_log);
                    let history = self.terminal_history.as_mut().expect("checked above");
                    history.actionable_recovered = actionable_recovered;
                    history.cold_authority_rebuilt = cold_authority_rebuilt;
                    (state, None, None, None)
                }
            })
        })();
        self.standalone = Some(standalone);
        let (final_state, handoff, codebase_memory, plan_feature) = result?;
        let elapsed = started.elapsed();
        required_ref(&self.fake, "jig.fake_llm")?.validate_after_convergence(strategy)?;
        if matches!(
            strategy,
            ConvergenceStrategy::SinglePullRequest
                | ConvergenceStrategy::ImplementationPrTerminalCi
                | ConvergenceStrategy::CiPollExactHeadRepair
        ) && elapsed >= self.harness.scenario.poll_backstop
        {
            return Err(format!(
                "converged in {elapsed:?}, not before the declared poll backstop {:?}; raw webhooks should wake the standalone engine",
                self.harness.scenario.poll_backstop
            ));
        }
        self.convergence = Some(ConvergenceOutput {
            elapsed,
            final_state,
            handoff,
            codebase_memory,
            plan_feature,
        });
        Ok(())
    }

    fn primary_issue(&self) -> Result<ItemNumber, String> {
        self.issue_order
            .first()
            .copied()
            .ok_or_else(|| "no issue.seed action has executed".to_string())
    }

    fn retain_failure_artifacts(&self) {
        self.workspace.retain_on_drop();
        if let Some(fake) = &self.fake {
            write_snapshot(&self.logs.fake_llm_log, &fake.log_tail());
        }
        if let (Some(forge), Some(repository)) = (&self.forge, &self.repository) {
            write_snapshot(
                &self.logs.ci_diagnostics_log,
                &super::convergence::ci_diagnostics(forge, repository),
            );
        }
    }
}

fn required_ref<'a, T>(value: &'a Option<T>, action: &str) -> Result<&'a T, String> {
    value
        .as_ref()
        .ok_or_else(|| format!("prerequisite action `{action}` has not completed"))
}

fn required_mut<'a, T>(value: &'a mut Option<T>, action: &str) -> Result<&'a mut T, String> {
    value
        .as_mut()
        .ok_or_else(|| format!("prerequisite action `{action}` has not completed"))
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.to_string())
}

pub(super) fn action_name(action: &ManifestAction) -> &'static str {
    match action {
        ManifestAction::ProvisionForgejo => "forgejo.provision",
        ManifestAction::AwaitForgejoRunner => "forgejo_runner.ready",
        ManifestAction::SeedRepository { .. } => "repo.seed",
        ManifestAction::StartJig { .. } => "jig.fake_llm",
        ManifestAction::LaunchTemper { .. } => "temper.launch_standalone",
        ManifestAction::SeedIssue { .. } => "issue.seed",
        ManifestAction::SeedTerminalHistory { .. } => "history.seed_terminal",
        ManifestAction::SeedPullRequest { .. } => "pr.seed_existing",
        ManifestAction::StartCodebaseMemoryMcp { .. } => "mcp.fake_codebase_memory.start",
        ManifestAction::ConfigureAgentTools { .. } => "agent.tools.configure",
        ManifestAction::WaitForConvergence { .. } => "workflow.wait_convergence",
        ManifestAction::Stimulus(stimulus) => stimulus.action(),
    }
}
