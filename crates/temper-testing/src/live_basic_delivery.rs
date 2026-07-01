//! Shared live Forgejo harness for the checked-in `basic-delivery` scenario.
//!
//! This module owns the heavyweight end-to-end path that used to live directly
//! in the root `tests/basic_delivery_forgejo_e2e.rs` integration test: cached
//! bare-admin Forgejo, a host-mode `forgejo-runner`, Jig's scripted fake LLM,
//! a real standalone `temper` binary, and scenario-bundle fixture seeding.  The
//! root test now supplies only the binary path/serialization lock and delegates
//! the topology proof here so later CLI wiring can reuse the same evidence model.

mod bundle;
mod convergence;
mod fake_llm;
mod process;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

pub use bundle::{IntakeFixture, RepoFixture, ScenarioBundle};
use convergence::{
    admin_forge, ci_diagnostics, drive_full_basic_delivery, repository, seed_intake,
};
use fake_llm::BasicDeliveryFake;
use process::{
    TemperInitRequest, assert_init_workflow_yaml_matches, convergence_timeout, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone, write_snapshot,
};

const ENGINEER: &str = "engineer";
const DEFAULT_ADMIN_USER: &str = "basicadmin";
const DEFAULT_ADMIN_PASSWORD: &str = "Basic-Delivery-Admin-1!";
const DEFAULT_ADMIN_EMAIL: &str = "basicadmin@example.invalid";
const INIT_PROVIDER_KEY: &str = "basic-delivery-jig-dummy-key";

const DEFAULT_WORKSPACE_PREFIX: &str = "temper-basic-delivery-e2e";
const DEFAULT_CONVERGENCE_SECS: u64 = 360;
const DEFAULT_DAEMON_POLL_BACKSTOP_SECS: u64 = 600;
const DEFAULT_MECHANICAL_CADENCE_SECS: u64 = 1;

/// Explicit injection seam for the standalone `temper` binary used by the live
/// harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TemperCommand {
    binary: PathBuf,
}

impl TemperCommand {
    /// Uses `binary` whenever the harness needs to spawn `temper init` or
    /// `temper serve standalone`.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// Path to the injected `temper` binary.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub(crate) fn command(&self) -> Command {
        Command::new(&self.binary)
    }
}

/// Configured live basic-delivery harness.
#[derive(Clone)]
pub struct LiveBasicDeliveryHarness {
    pub scenario: ScenarioBundle,
    pub temper: TemperCommand,
    pub admin_user: String,
    pub admin_password: String,
    pub admin_email: String,
    pub workspace_prefix: String,
}

impl LiveBasicDeliveryHarness {
    /// Builds the harness with the default local Forgejo administrator and
    /// workspace settings.
    pub fn new(scenario: ScenarioBundle, temper: TemperCommand) -> Self {
        Self {
            scenario,
            temper,
            admin_user: DEFAULT_ADMIN_USER.to_string(),
            admin_password: DEFAULT_ADMIN_PASSWORD.to_string(),
            admin_email: DEFAULT_ADMIN_EMAIL.to_string(),
            workspace_prefix: DEFAULT_WORKSPACE_PREFIX.to_string(),
        }
    }

    /// Runs the full live topology proof and returns structured evidence. The
    /// returned evidence owns the temporary workspace until it is dropped, so log
    /// paths named in the evidence remain readable long enough for callers to
    /// print or archive them.
    pub fn run(&self) -> Result<LiveBasicDeliveryEvidence, String> {
        let started = Instant::now();
        self.scenario.assert_workflow_matches_reference()?;

        let cached = start_cached_bare_admin_server(
            &self.admin_user,
            &self.admin_password,
            &self.admin_email,
        )
        .map_err(|error| format!("cached bare-admin Forgejo starts: {error}"))?;
        let server = cached.server;
        let mut runner = ForgejoRunner::register(&server)
            .map_err(|error| format!("forgejo-runner registers: {error}"))?;
        if !runner.is_running() {
            return Err(format!(
                "forgejo-runner exited immediately\n--- runner log ---\n{}",
                runner.log_tail()
            ));
        }
        let admin_token = mint_site_admin_token(&server, &self.admin_user)?;

        let fake = BasicDeliveryFake::start();
        let workspace = RunWorkspace::new(&self.workspace_prefix);
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

        let bind_port = free_port()?;
        run_temper_init(TemperInitRequest {
            temper: &self.temper,
            server: &server,
            scenario: &self.scenario,
            bundle_dir: &bundle_dir,
            workspaces_dir: &workspaces_dir,
            bind_port,
            fake_llm_url: &fake.base_url(),
            log: &logs.init_log,
            admin_user: &self.admin_user,
            admin_password: &self.admin_password,
        })?;
        assert_init_workflow_yaml_matches(&bundle_dir.join("workflow.yaml"), &self.scenario)?;
        tune_init_config(
            &bundle_dir.join("config.toml"),
            self.scenario.poll_backstop.as_secs(),
            self.scenario.mechanical_cadence.as_secs(),
        )?;

        populate_repo(
            server.base_url(),
            &admin_token,
            workspace.path(),
            &self.scenario.repo,
            &logs.repo_populate_log,
        )?;

        let mut standalone =
            spawn_temper_standalone(&self.temper, &bundle_dir, &logs.standalone_log)?;
        wait_for_standalone(&mut standalone)?;

        let forge = admin_forge(server.base_url(), &admin_token, &self.scenario.repo);
        let repository = process::engine_block_on(repository(&forge, &self.scenario.repo))?;
        let issue =
            process::engine_block_on(seed_intake(&forge, &repository, &self.scenario.intake))?;

        let timeout = convergence_timeout(self.scenario.timeout);
        let convergence_start = Instant::now();
        let final_state = match drive_full_basic_delivery(
            &forge,
            &repository,
            issue,
            &self.admin_user,
            &mut standalone,
            timeout,
        ) {
            Ok(final_state) => final_state,
            Err(error) => {
                write_snapshot(&logs.fake_llm_log, &fake.log_tail());
                write_snapshot(
                    &logs.ci_diagnostics_log,
                    &ci_diagnostics(&forge, &repository),
                );
                return Err(format!(
                    "live basic-delivery did not converge within {timeout:?}: {error}\n\
                     forge_url={} repo={} intake_issue=#{} runner_running={}\n\
                     runner log tail:\n{}\n\
                     --- init log ({}) ---\n{}\n\
                     --- repo populate log ({}) ---\n{}\n\
                     --- standalone daemon/worker/agent log ({}) ---\n{}\n\
                     --- fake LLM request tail ({}) ---\n{}\n\
                     --- CI diagnostics ({}) ---\n{}\n\
                     --- Forgejo web log ---\n{}",
                    server.base_url(),
                    self.scenario.repo.slug,
                    issue,
                    runner.is_running(),
                    runner.log_tail(),
                    logs.init_log.display(),
                    read_tail(&logs.init_log, 120),
                    logs.repo_populate_log.display(),
                    read_tail(&logs.repo_populate_log, 120),
                    logs.standalone_log.display(),
                    standalone.log_tail(),
                    logs.fake_llm_log.display(),
                    fake.log_tail(),
                    logs.ci_diagnostics_log.display(),
                    ci_diagnostics(&forge, &repository),
                    read_tail(&server.data_dir().join("web.log"), 80),
                ));
            }
        };
        let convergence = convergence_start.elapsed();

        if convergence >= self.scenario.poll_backstop {
            return Err(format!(
                "converged in {convergence:?}, not before the long poll backstop {:?}; raw webhooks should wake the standalone engine\n--- standalone log ---\n{}",
                self.scenario.poll_backstop,
                standalone.log_tail()
            ));
        }
        if fake.architect_requests() < 2 {
            return Err(format!(
                "fake LLM never served the architect tool loop\n{}",
                fake.log_tail()
            ));
        }
        if fake.engineer_requests() < 2 {
            return Err(format!(
                "fake LLM never served the engineer tool loop\n{}",
                fake.log_tail()
            ));
        }

        write_snapshot(&logs.fake_llm_log, &fake.log_tail());
        write_snapshot(
            &logs.ci_diagnostics_log,
            &ci_diagnostics(&forge, &repository),
        );

        standalone.kill();
        Ok(LiveBasicDeliveryEvidence {
            _workspace: workspace,
            scenario_path: self.scenario.scenario_path.clone(),
            manifest_path: self.scenario.manifest_path.clone(),
            temper_binary: self.temper.binary.clone(),
            forge_url: server.base_url().to_string(),
            repo_slug: self.scenario.repo.slug.clone(),
            forge_cache_hit: cached.cache_hit,
            runner_running: runner.is_running(),
            startup: started.elapsed().saturating_sub(convergence),
            convergence,
            total_elapsed: started.elapsed(),
            poll_backstop: self.scenario.poll_backstop,
            fake_llm: FakeLlmEvidence {
                base_url: fake.base_url(),
                architect_requests: fake.architect_requests(),
                engineer_requests: fake.engineer_requests(),
                log_path: logs.fake_llm_log.clone(),
            },
            final_state,
            logs,
        })
    }
}

/// Convenience wrapper around [`LiveBasicDeliveryHarness::run`].
pub fn run_live_basic_delivery(
    scenario: ScenarioBundle,
    temper: TemperCommand,
) -> Result<LiveBasicDeliveryEvidence, String> {
    LiveBasicDeliveryHarness::new(scenario, temper).run()
}

/// Structured evidence emitted by a successful live basic-delivery run.
pub struct LiveBasicDeliveryEvidence {
    _workspace: RunWorkspace,
    pub scenario_path: PathBuf,
    pub manifest_path: PathBuf,
    pub temper_binary: PathBuf,
    pub forge_url: String,
    pub repo_slug: String,
    pub forge_cache_hit: bool,
    pub runner_running: bool,
    pub startup: Duration,
    pub convergence: Duration,
    pub total_elapsed: Duration,
    pub poll_backstop: Duration,
    pub fake_llm: FakeLlmEvidence,
    pub final_state: FinalStateEvidence,
    pub logs: LiveLogPaths,
}

impl LiveBasicDeliveryEvidence {
    /// Compact human-readable rendering for ignored-test stdout/CI logs.
    pub fn to_report(&self) -> String {
        let mut lines = vec![
            "live_basic_delivery evidence:".to_string(),
            format!("  scenario: {}", self.scenario_path.display()),
            format!("  manifest: {}", self.manifest_path.display()),
            format!("  temper_binary: {}", self.temper_binary.display()),
            format!("  forge_url: {}", self.forge_url),
            format!("  repo: {}", self.repo_slug),
            format!(
                "  forge_cache_hit: {} runner_running: {} startup: {:?}",
                self.forge_cache_hit, self.runner_running, self.startup
            ),
            format!(
                "  convergence: {:?} (poll_backstop: {:?}, total: {:?})",
                self.convergence, self.poll_backstop, self.total_elapsed
            ),
            format!(
                "  fake_llm: {} architect_requests={} engineer_requests={} log={}",
                self.fake_llm.base_url,
                self.fake_llm.architect_requests,
                self.fake_llm.engineer_requests,
                self.fake_llm.log_path.display()
            ),
            format!(
                "  source_issue: #{} state={} labels={:?}",
                self.final_state.issue.number,
                self.final_state.issue.state,
                self.final_state.issue.labels
            ),
            format!(
                "  implementation_pr: #{} state={} merged_by={:?} labels={:?} head={} sha={:?}",
                self.final_state.pull_request.number,
                self.final_state.pull_request.state,
                self.final_state.pull_request.merged_by,
                self.final_state.pull_request.labels,
                self.final_state.pull_request.head_branch,
                self.final_state.pull_request.head_sha
            ),
            "  ci_jobs:".to_string(),
        ];
        for job in &self.final_state.ci_jobs {
            lines.push(format!(
                "    - {} status={} conclusion={:?} url={:?}",
                job.name, job.status, job.conclusion, job.url
            ));
        }
        lines.extend([
            "  logs:".to_string(),
            format!("    workspace_root: {}", self.logs.workspace_root.display()),
            format!("    init: {}", self.logs.init_log.display()),
            format!(
                "    repo_populate: {}",
                self.logs.repo_populate_log.display()
            ),
            format!("    standalone: {}", self.logs.standalone_log.display()),
            format!("    fake_llm: {}", self.logs.fake_llm_log.display()),
            format!(
                "    ci_diagnostics: {}",
                self.logs.ci_diagnostics_log.display()
            ),
        ]);
        lines.join("\n")
    }
}

/// Paths to logs/snapshots produced by the live harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveLogPaths {
    pub workspace_root: PathBuf,
    pub init_log: PathBuf,
    pub repo_populate_log: PathBuf,
    pub standalone_log: PathBuf,
    pub fake_llm_log: PathBuf,
    pub ci_diagnostics_log: PathBuf,
}

/// Evidence about the Jig fake LLM server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FakeLlmEvidence {
    pub base_url: String,
    pub architect_requests: usize,
    pub engineer_requests: usize,
    pub log_path: PathBuf,
}

/// Terminal Forge state proving the scenario converged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalStateEvidence {
    pub issue: IssueEvidence,
    pub pull_request: PullRequestEvidence,
    pub ci_jobs: Vec<CiJobEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssueEvidence {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestEvidence {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub labels: Vec<String>,
    pub author: String,
    pub merged_by: Option<String>,
    pub head_branch: String,
    pub head_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiJobEvidence {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub url: Option<String>,
}
