use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use jig_server::FakeLlm;
use skein::runtime::RuntimeHandle;
use temper_agent::{ProviderConfig, SubmitForPrHost, default_submit_for_pr_host};
use temper_engine::{Daemon, ForgeApplier, LeaseApplier, system_clock};
use temper_forge_memory::MemoryForge;
use temper_forge_model::{CreateIssue, CreateRepository, Forge, UserId};
use temper_worker::{
    CapabilitySpec, CodingExecutor, CodingExecutorConfig, ExecutorSelection, RoleGitIdentity,
    WorkerConfig,
};
use temper_workflow::{LeasePolicy, ValidatedWorkflow};

use super::DEFAULT_MAX_ITERATIONS;
use super::git::{path_str, seed_origin};
use super::runner::{DaemonPrFreshnessGuard, NativeJigAgentRunner};
use super::stack::HermeticRealStack;
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

    /// Builds the hermetic world on the provided skein runtime handle.
    pub async fn build(self, handle: &RuntimeHandle) -> Result<HermeticRealStack, String> {
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
        let applier = Arc::new(LeaseApplier::new(
            forge.clone(),
            LeasePolicy::new(chrono::Duration::seconds(300)),
            "hermetic-daemon",
            Arc::new(ForgeApplier::new(forge.clone(), workflow.clone())),
            system_clock(),
        ));
        let daemon_handle = Daemon::with_applier(Arc::new(handle.clone()), applier);
        let daemon_handle = match self.apply_grace {
            Some(apply_grace) => daemon_handle.with_apply_grace(apply_grace),
            None => daemon_handle,
        };
        let daemon = Arc::new(daemon_handle);
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
        let runner = Arc::new(NativeJigAgentRunner {
            handle: handle.clone(),
            provider,
            max_iterations: self.max_iterations,
            config_dir: None,
            enable_subagents: self.enable_subagents,
            submit_for_pr: self.submit_for_pr,
        });

        let role_identities = role_identities(&self.worker_roles);
        let worker_config = WorkerConfig {
            daemon_url: "in-process".to_string(),
            worker_id: primary_worker_role.worker_id.clone(),
            worker_pool: None,
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
            heartbeat_interval: Duration::from_millis(50),
            executor: ExecutorSelection::Stub,
        };
        let executor = Arc::new(
            CodingExecutor::new(
                CodingExecutorConfig {
                    workspace_root: workspace_root.clone(),
                    git_base_url: format!("file://{}", path_str(&git_root)?),
                    role_identities,
                },
                runner,
            )
            .with_pr_freshness_guard(Arc::new(DaemonPrFreshnessGuard::new(daemon.clone()))),
        );

        Ok(HermeticRealStack {
            _temp: temp,
            _fake_llm: fake_llm,
            forge,
            workflow,
            compiled,
            daemon,
            result_tx,
            result_rx,
            origins,
            repo_ids,
            workspace_root,
            primary_repo_path,
            primary_repo_id,
            issue_number: issue.number,
            role: primary_worker_role.role,
            worker_config,
            executor,
            worker_started: false,
        })
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
