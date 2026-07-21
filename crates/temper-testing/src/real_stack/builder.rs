use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use jig_server::FakeLlm;
use skein::runtime::RuntimeHandle;
use temper_agent::{ProviderConfig, SubmitForPrHost, default_submit_for_pr_host};
use temper_engine::{Daemon, ForgeApplier, LeaseApplier};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{CreateIssue, CreateRepository, Forge, UserId};
use temper_worker::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, RoleGitIdentity,
    TraceCollector, WorkerAgentTraceConfig, WorkerConfig,
};
use temper_workflow::{LeasePolicy, ValidatedWorkflow};

use super::DEFAULT_MAX_ITERATIONS;
use super::clock::MutableWallClock;
use super::git::{path_str, seed_origin};
use super::pause::PauseHooks;
use super::runner::{DaemonPrFreshnessGuard, HermeticActivityCounters, NativeJigAgentRunner};
use super::stack::{
    DaemonRouter, HermeticComponentHandles, HermeticDurableWorld, HermeticRealStack,
};
use super::types::{
    FakeModelResponse, FakeModelSetup, HermeticIssueSpec, HermeticRepoSpec, WorkerRoleSpec,
};

/// Builder for a hermetic daemon + worker + coding-agent world.
pub struct HermeticRealStackBuilder {
    repos: Vec<HermeticRepoSpec>,
    issue: HermeticIssueSpec,
    worker_roles: Vec<WorkerRoleSpec>,
    fake_model: Option<FakeModelSetup>,
    workflow: Option<ValidatedWorkflow>,
    max_iterations: usize,
    enable_subagents: bool,
    submit_for_pr: SubmitForPrHost,
    apply_grace: Option<Duration>,
    worker_heartbeat_interval: Duration,
    worker_liveness_limits: temper_worker::WorkerLivenessLimits,
    enable_agent_traces: bool,
    linux_supervisor_helper: Option<PathBuf>,
}

impl Default for HermeticRealStackBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HermeticRealStackBuilder {
    /// Creates a builder with one `acme/service` repo, one ready code issue, the
    /// reference-delivery workflow, an engineer worker identity, and a fake model
    /// that writes `service/HERMETIC_AGENT_OUTPUT.md`.
    pub fn new() -> Self {
        Self {
            repos: vec![HermeticRepoSpec::new("acme", "service")],
            issue: HermeticIssueSpec::ready_code(
                "Hermetic real-stack smoke",
                "Create HERMETIC_AGENT_OUTPUT.md with deterministic content.",
            ),
            worker_roles: vec![WorkerRoleSpec::engineer()],
            fake_model: None,
            workflow: None,
            max_iterations: DEFAULT_MAX_ITERATIONS,
            enable_subagents: false,
            submit_for_pr: default_submit_for_pr_host(),
            apply_grace: None,
            worker_heartbeat_interval: Duration::from_millis(50),
            worker_liveness_limits: Default::default(),
            enable_agent_traces: false,
            linux_supervisor_helper: None,
        }
    }

    /// Replaces the primary repo. Use [`Self::add_repo`] for coordinated
    /// multi-repo seeds.
    #[must_use]
    pub fn repo(mut self, repo: HermeticRepoSpec) -> Self {
        self.repos = vec![repo];
        self
    }

    /// Adds an extra local git remote and MemoryForge repository. The primary
    /// issue still lives in the first repo; add a `temper:workspace` block to
    /// the issue body when a test wants the daemon feed to include extras in the
    /// worker manifest.
    #[must_use]
    pub fn add_repo(mut self, repo: HermeticRepoSpec) -> Self {
        self.repos.push(repo);
        self
    }

    /// Replaces the seeded issue.
    #[must_use]
    pub fn issue(mut self, issue: HermeticIssueSpec) -> Self {
        self.issue = issue;
        self
    }

    /// Replaces the worker role identity/capacity list with one role.
    #[must_use]
    pub fn worker_role(mut self, worker_role: WorkerRoleSpec) -> Self {
        self.worker_roles = vec![worker_role];
        self
    }

    /// Adds another role identity/capability to the same hermetic worker.
    /// Useful for fast stories that run a read-only architect turn followed by
    /// an engineer coding turn through one daemon/worker stack.
    #[must_use]
    pub fn add_worker_role(mut self, worker_role: WorkerRoleSpec) -> Self {
        self.worker_roles.push(worker_role);
        self
    }

    /// Uses a typed write-then-summary fake model response.
    #[must_use]
    pub fn fake_model_response(mut self, response: FakeModelResponse) -> Self {
        self.fake_model = Some(FakeModelSetup::Response(response));
        self
    }

    /// Uses an arbitrary Jig script for tests that need custom model turns or
    /// verdicts.
    #[must_use]
    pub fn fake_model_script(mut self, script: jig_core::Script) -> Self {
        self.fake_model = Some(FakeModelSetup::Script(script));
        self
    }

    /// Uses a non-default workflow. The default is
    /// [`crate::workflow`](crate::workflow), the reference-delivery fixture.
    #[must_use]
    pub fn workflow(mut self, workflow: ValidatedWorkflow) -> Self {
        self.workflow = Some(workflow);
        self
    }

    /// Overrides the native agent iteration limit.
    #[must_use]
    pub fn max_iterations(mut self, max_iterations: usize) -> Self {
        self.max_iterations = max_iterations;
        self
    }

    /// Enables the native agent's read-only sub-agent tool.
    #[must_use]
    pub fn enable_subagents(mut self, enable_subagents: bool) -> Self {
        self.enable_subagents = enable_subagents;
        self
    }

    /// Overrides the host-controlled `submit_for_pr` gate used by writable
    /// engineer sessions.
    #[must_use]
    pub fn submit_for_pr_host(mut self, submit_for_pr: SubmitForPrHost) -> Self {
        self.submit_for_pr = submit_for_pr;
        self
    }

    /// Overrides the daemon's post-apply re-enqueue grace window. The default
    /// preserves the production daemon setting; hermetic retry tests can set
    /// this to zero when they explicitly drive the follow-up scan.
    #[must_use]
    pub fn apply_grace(mut self, apply_grace: Duration) -> Self {
        self.apply_grace = Some(apply_grace);
        self
    }

    /// Overrides the worker heartbeat cadence for lifecycle tests that need to
    /// isolate another worker/daemon protocol boundary.
    #[must_use]
    pub fn worker_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.worker_heartbeat_interval = interval;
        self
    }

    /// Overrides worker-owned watchdog limits for restart/cancellation tests.
    #[must_use]
    pub fn worker_liveness_limits(mut self, limits: temper_worker::WorkerLivenessLimits) -> Self {
        self.worker_liveness_limits = limits;
        self
    }

    /// Enables the worker trace spool and engine journal with metadata capture.
    #[must_use]
    pub fn enable_agent_traces(mut self) -> Self {
        self.enable_agent_traces = true;
        self
    }

    /// Forces worker-owned fixture commands through the real Linux supervisor
    /// using an explicitly built early-main helper. This instance-scoped test
    /// seam bypasses delegated cgroups without changing production automatic
    /// backend selection.
    #[must_use]
    pub fn linux_supervisor_helper(mut self, helper: impl Into<PathBuf>) -> Self {
        self.linux_supervisor_helper = Some(helper.into());
        self
    }

    /// Builds the hermetic world on the provided skein runtime handle.
    pub async fn build(self, handle: &RuntimeHandle) -> Result<HermeticRealStack, String> {
        let worker_containment_factory =
            linux_supervisor_containment_factory(self.linux_supervisor_helper.as_deref())?;
        let primary = self
            .repos
            .first()
            .ok_or_else(|| "at least one repository is required".to_string())?
            .clone();
        let primary_worker_role = self
            .worker_roles
            .first()
            .ok_or_else(|| "at least one worker role is required".to_string())?
            .clone();
        let primary_repo_path = primary.path();
        let temp = tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;
        let git_root = temp.path().join("git");
        let seed_root = temp.path().join("seed");
        let workspace_root = temp.path().join("workspaces");
        let trace_config = if self.enable_agent_traces {
            WorkerAgentTraceConfig {
                policy: temper_protocol_activity::AgentActivityCapturePolicyV1::default(),
                spool_root: Some(temp.path().join("agent-traces/spool")),
            }
        } else {
            WorkerAgentTraceConfig::default()
        };
        let trace_collector = TraceCollector::new(trace_config.clone());
        let trace_journal = if self.enable_agent_traces {
            Some(
                temper_engine::AgentTraceJournal::open(temper_engine::TraceJournalConfig {
                    root: temp.path().join("agent-traces/journal"),
                    policy: trace_config.policy.clone(),
                })
                .map_err(|error| format!("open hermetic trace journal: {error}"))?,
            )
        } else {
            None
        };

        let forge = Arc::new(MemoryForge::new());
        let mut repo_ids = BTreeMap::new();
        let mut origins = BTreeMap::new();
        for repo in &self.repos {
            let repo_path = repo.path();
            if repo_ids.contains_key(&repo_path) {
                return Err(format!("duplicate repository seed `{repo_path}`"));
            }
            let origin = seed_origin(&git_root, &seed_root, repo)?;
            let created = forge
                .create_repository(CreateRepository {
                    owner: repo.owner.clone(),
                    name: repo.name.clone(),
                    default_branch: repo.default_branch.clone(),
                    description: None,
                })
                .await
                .map_err(|error| format!("create MemoryForge repository {repo_path}: {error}"))?;
            repo_ids.insert(repo_path.clone(), created.id);
            origins.insert(repo_path, origin);
        }
        let primary_repo_id = repo_ids
            .get(&primary_repo_path)
            .cloned()
            .ok_or_else(|| format!("primary repository `{primary_repo_path}` was not seeded"))?;
        let issue = forge
            .create_issue(
                &primary_repo_id,
                CreateIssue {
                    title: self.issue.title,
                    body: self.issue.body,
                    labels: self.issue.labels,
                    assignees: Vec::<UserId>::new(),
                },
            )
            .await
            .map_err(|error| format!("create MemoryForge issue: {error}"))?;

        let workflow = Arc::new(self.workflow.unwrap_or_else(crate::workflow));
        let compiled = workflow.compile();
        let clock = MutableWallClock::new(
            super::DEFAULT_NOW
                .parse()
                .map_err(|error| format!("parse default timestamp: {error}"))?,
        );
        let hooks = PauseHooks::default();
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "hermetic-daemon",
            Arc::new(
                ForgeApplier::new(forge.clone(), workflow.clone())
                    .with_child_issue_hook(Arc::new(hooks.clone())),
            ),
            clock.capability(),
        ));
        let artifact_context =
            super::artifact_context::service(forge.clone(), workflow.clone(), &repo_ids);
        let daemon_handle = Daemon::with_applier(Arc::new(handle.clone()), applier)
            .with_artifact_context_service(artifact_context)
            .with_forge_context_reader(forge.clone(), workflow.clone());
        let daemon_handle = match trace_journal.as_ref() {
            Some(journal) => daemon_handle.with_trace_journal(journal.clone()),
            None => daemon_handle,
        };
        let daemon_handle = match self.apply_grace {
            Some(apply_grace) => daemon_handle.with_apply_grace(apply_grace),
            None => daemon_handle,
        };
        let daemon = Arc::new(daemon_handle);
        let router = Arc::new(DaemonRouter::new(daemon.clone()));
        let activity = Arc::new(HermeticActivityCounters::default());
        let router_for_context = Arc::clone(&router);
        let activity_for_context = Arc::clone(&activity);
        let context_worker_id = primary_worker_role.worker_id.clone();
        let forge_context: temper_worker::AgentForgeContextHost =
            Arc::new(move |job_id, attempt_id, fence, operation| {
                let router = Arc::clone(&router_for_context);
                let activity = Arc::clone(&activity_for_context);
                let worker_id = context_worker_id.clone();
                Box::pin(async move {
                    if !fence.is_open() {
                        return Err(
                            temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable,
                        );
                    }
                    activity.forge_context_started();
                    let outcome = async {
                        let request = temper_protocol_worker::FetchContext::new(
                            &worker_id,
                            &job_id,
                            &attempt_id,
                            operation,
                        );
                        let response = router
                            .current()
                            .deliver_protocol_message(
                                temper_protocol_worker::WorkerProtocolMessage::FetchContext(
                                    request,
                                ),
                            )
                            .await
                            .map_err(|_| {
                                temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable
                            })?
                            .ok_or(
                                temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable,
                            )?;
                        if !fence.is_open() {
                            return Err(
                                temper_protocol_worker::ForgeContextErrorCode::ForgeUnavailable,
                            );
                        }
                        let temper_protocol_worker::WorkerProtocolMessage::ContextResponse(
                            response,
                        ) = response
                        else {
                            return Err(
                                temper_protocol_worker::ForgeContextErrorCode::InvalidRequest,
                            );
                        };
                        if response.protocol_version
                            != temper_protocol_worker::WORKER_PROTOCOL_VERSION
                            || response.worker_id != worker_id
                            || response.job_id != job_id
                        {
                            return Err(
                                temper_protocol_worker::ForgeContextErrorCode::InvalidRequest,
                            );
                        }
                        match response.outcome {
                            temper_protocol_worker::ContextOutcome::Success { result } => {
                                Ok(result)
                            }
                            temper_protocol_worker::ContextOutcome::Error { code } => Err(code),
                        }
                    }
                    .await;
                    activity.forge_context_finished();
                    outcome
                })
            });
        let (result_tx, result_rx) = temper_engine_io::channel();

        let script = match self.fake_model {
            Some(FakeModelSetup::Response(response)) => response.into_script(),
            Some(FakeModelSetup::Script(script)) => script,
            None => FakeModelResponse::write_file(
                format!("{}/HERMETIC_AGENT_OUTPUT.md", primary.name),
                "hermetic agent output\n",
                "Created HERMETIC_AGENT_OUTPUT.md.",
            )
            .into_script(),
        };
        let fake_llm =
            FakeLlm::start(script).map_err(|error| format!("start Jig fake LLM: {error}"))?;
        let provider = ProviderConfig::new(
            "jig-openai-compatible",
            "hermetic-real-stack",
            "https://example.invalid/unused-production-url",
            "sk-jig-test",
        )
        .with_base_url_override(fake_llm.base_url());
        let submit_host = self.submit_for_pr;
        let activity_for_submit = Arc::clone(&activity);
        let submit_for_pr: SubmitForPrHost = Arc::new(move |request, context, cwd| {
            let submit_host = submit_host.clone();
            let activity = Arc::clone(&activity_for_submit);
            Box::pin(async move {
                activity.submit_started();
                let response = submit_host(request, context, cwd).await;
                activity.submit_finished();
                response
            })
        });
        let runner = Arc::new(NativeJigAgentRunner {
            handle: handle.clone(),
            provider,
            max_iterations: self.max_iterations,
            config_dir: None,
            enable_subagents: self.enable_subagents,
            submit_for_pr,
            forge_context,
            hooks: hooks.clone(),
            trace_policy: trace_config.policy.clone(),
            trace_collector: trace_collector.clone(),
            activity,
            observed_agent_sessions: Default::default(),
        });

        let role_identities = role_identities(&self.worker_roles);
        let worker_config = WorkerConfig {
            daemon_url: "in-process".to_string(),
            worker_id: primary_worker_role.worker_id.clone(),
            worker_pool: None,
            worker_auth: None,
            capabilities: self
                .worker_roles
                .iter()
                .flat_map(|role| {
                    self.repos.iter().map(move |repo| CapabilitySpec {
                        repo: repo.path(),
                        role: role.role.clone(),
                    })
                })
                .collect(),
            role_identities: role_identities.clone(),
            max_concurrent_jobs: self
                .worker_roles
                .iter()
                .map(|role| role.max_concurrent_jobs)
                .min()
                .unwrap_or(1),
            poll_wait: Duration::from_millis(25),
            heartbeat_interval: self.worker_heartbeat_interval,
            liveness_limits: self.worker_liveness_limits,
            result_root: workspace_root.join(".temper/worker-results"),
            agent_traces: trace_config,
            executor: ExecutorSelection::Stub,
        };
        let coding_config = CodingExecutorConfig {
            workspace_root: workspace_root.clone(),
            git_base_url: format!("file://{}", path_str(&git_root)?),
            role_identities,
        };
        let executor = CodingExecutor::new(coding_config.clone(), runner.clone())
            .with_pr_freshness_guard(Arc::new(DaemonPrFreshnessGuard::new(daemon.clone())));
        let executor = match worker_containment_factory.clone() {
            Some(factory) => executor.with_containment_factory(factory),
            None => executor,
        };
        let executor = Arc::new(executor);

        Ok(HermeticRealStack {
            world: HermeticDurableWorld {
                _temp: temp,
                _fake_llm: fake_llm,
                forge,
                workflow,
                compiled,
                result_tx,
                result_rx,
                published_results: Default::default(),
                published_releases: Default::default(),
                origins,
                repo_ids,
                workspace_root,
                primary_repo_path,
                primary_repo_id,
                issue_number: issue.number,
                role: primary_worker_role.role,
                worker_config,
                coding_config,
                worker_containment_factory,
                runner,
                clock,
                hooks,
                router,
                trace_collector,
                trace_journal,
                apply_grace: self.apply_grace,
                mechanical_journal: temper_workflow::InMemoryJournal::new(),
            },
            components: HermeticComponentHandles {
                daemon,
                executor,
                worker: None,
                recovered: BTreeMap::new(),
            },
        })
    }
}

fn linux_supervisor_containment_factory(
    helper: Option<&Path>,
) -> Result<Option<temper_process_containment::ContainmentFactory>, String> {
    let Some(helper) = helper else {
        return Ok(None);
    };

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt as _;
        use temper_process_containment::{
            ContainmentBackendFactory, ContainmentBackendPolicy, ContainmentFactory,
            LinuxSupervisorBackendFactory,
        };

        let actionable = || {
            "build the `temper-real-stack-supervisor-helper` binary and pass its path from \
             `env!(\"CARGO_BIN_EXE_temper-real-stack-supervisor-helper\")`"
        };
        let helper = helper.canonicalize().map_err(|error| {
            format!(
                "HermeticRealStack Linux supervisor helper `{}` cannot be selected: {error}; {}",
                helper.display(),
                actionable()
            )
        })?;
        let metadata = helper.metadata().map_err(|error| {
            format!(
                "HermeticRealStack Linux supervisor helper `{}` cannot be inspected: {error}; {}",
                helper.display(),
                actionable()
            )
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!(
                "HermeticRealStack Linux supervisor helper `{}` is not an executable file; {}",
                helper.display(),
                actionable()
            ));
        }
        let backend: Arc<dyn ContainmentBackendFactory> = Arc::new(
            LinuxSupervisorBackendFactory::with_helper_executable(helper),
        );
        Ok(Some(ContainmentFactory::new(
            ContainmentBackendPolicy::ForceLinuxSupervisor,
            backend,
        )))
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err(format!(
            "HermeticRealStack Linux supervisor helper `{}` was configured on unsupported target `{}`",
            helper.display(),
            std::env::consts::OS
        ))
    }
}

fn role_identities(roles: &[WorkerRoleSpec]) -> BTreeMap<String, RoleGitIdentity> {
    roles
        .iter()
        .map(|role| {
            (
                role.role.clone(),
                RoleGitIdentity {
                    user: role.git_user.clone(),
                    email: role.git_email.clone(),
                    token: role.git_token.clone(),
                },
            )
        })
        .collect()
}
