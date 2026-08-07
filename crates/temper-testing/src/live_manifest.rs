//! Data-driven live Forgejo manifest harness for checked-in scenario bundles.
//!
//! This module owns the heavyweight end-to-end path that used to live directly
//! in the root `tests/basic_delivery_forgejo_e2e.rs` integration test: cached
//! bare-admin Forgejo, a host-mode `forgejo-runner`, Jig's scripted fake LLM,
//! a real standalone `temper` binary, and scenario-bundle fixture seeding.  The
//! root test now supplies only the binary path/serialization lock and delegates
//! the topology proof here so later CLI wiring can reuse the same evidence model.

mod bundle;
mod codebase_memory;
mod convergence;
mod execution_plan;
mod failure_evidence;
mod fake_llm;
mod handoff;
mod late_stream_jig;
mod plan_feature;
mod process;
mod runtime;
mod runtime_fake;
#[cfg(test)]
mod runtime_tests;
#[cfg(target_os = "linux")]
mod standalone_shutdown;
mod stimuli;
#[cfg(test)]
mod stimulus_manifest_tests;
mod terminal_history;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::forgejo_runtime::RunWorkspace;
pub use bundle::{
    AgentFixture, ConvergenceStrategy, ForcedSystemicFailureFixture, IntakeFixture,
    LateStreamFailureBurst, LateStreamFailureFixture, ManifestAction, ManifestExecutionPlan,
    ManifestStep, ObservabilityFixture, RecoveryFixture, RepoFixture, ScenarioBundle,
    TerminalHistorySeedFixture,
};
pub use failure_evidence::{CiFailureEvidenceFixture, LiveCiFailureEvidence};
pub use handoff::{LiveHandoffCaseEvidence, LiveHandoffEvidence};
pub use plan_feature::{
    IssueState as PlanIssueState, LivePlanFeatureEvidence,
    PullRequestCiJobEvidence as PlanCiJobEvidence,
    PullRequestStateEvidence as PlanPullRequestStateEvidence,
};
#[cfg(target_os = "linux")]
pub use standalone_shutdown::{
    StandaloneShutdownEvidence, StandaloneShutdownRequest, run_standalone_shutdown_acceptance,
};
pub use stimuli::{
    StimulusFailure, StimulusKind, StimulusOutcome, StimulusRuntime, StimulusSpec, StimulusStatus,
    execute_stimuli,
};

const ENGINEER: &str = "engineer";
const DEFAULT_ADMIN_USER: &str = "basicadmin";
const DEFAULT_ADMIN_PASSWORD: &str = "Basic-Delivery-Admin-1!";
const DEFAULT_ADMIN_EMAIL: &str = "basicadmin@example.invalid";
const INIT_PROVIDER_KEY: &str = "basic-delivery-jig-dummy-key";
pub(super) const PROVIDER_HEALTH_SECRET: &str = "temper-live-manifest-provider-health-v1";

const DEFAULT_WORKSPACE_PREFIX: &str = "temper-basic-delivery-e2e";
const DEFAULT_CONVERGENCE_SECS: u64 = 360;
const DEFAULT_DAEMON_POLL_BACKSTOP_SECS: u64 = 600;
const DEFAULT_CI_POLL_CADENCE_SECS: u64 = 60;
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

/// Configured validation-grade live manifest harness.
#[derive(Clone)]
pub struct LiveManifestHarness {
    pub scenario: ScenarioBundle,
    pub temper: TemperCommand,
    pub admin_user: String,
    pub admin_password: String,
    pub admin_email: String,
    pub workspace_prefix: String,
}

impl LiveManifestHarness {
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
    pub fn run(&self) -> Result<LiveManifestEvidence, String> {
        runtime::execute(self)
    }
}

/// Convenience wrapper around [`LiveManifestHarness::run`].
pub fn run_live_manifest(
    scenario: ScenarioBundle,
    temper: TemperCommand,
) -> Result<LiveManifestEvidence, String> {
    LiveManifestHarness::new(scenario, temper).run()
}

fn scenario_run_id(scenario: &ScenarioBundle) -> String {
    let scenario_name = scenario
        .scenario_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("scenario");
    let safe_name = scenario_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{safe_name}-{}-{nanos}", std::process::id())
}

/// Structured evidence emitted by a successful live manifest run.
pub struct LiveManifestEvidence {
    _workspace: RunWorkspace,
    pub scenario_path: PathBuf,
    pub manifest_path: PathBuf,
    pub scenario_run_id: String,
    pub temper_log_format: String,
    pub rust_log: String,
    pub temper_binary: PathBuf,
    pub forge_url: String,
    pub repo_slug: String,
    pub repo_id: String,
    pub repo_default_branch: String,
    pub forge_cache_hit: bool,
    pub runner_running: bool,
    pub startup: Duration,
    pub convergence: Duration,
    pub total_elapsed: Duration,
    pub poll_backstop: Duration,
    /// Secret-free cadence values read back from the generated standalone config.
    pub effective_configuration: EffectiveConfigurationEvidence,
    pub fake_llm: FakeLlmEvidence,
    /// Complete terminal Forge PR inventory, including unexpected publications.
    pub forge_pull_requests: Vec<PullRequestEvidence>,
    pub final_state: FinalStateEvidence,
    /// Bounded, secret-free provenance for requests made by the harness's live
    /// Forge observer. This includes the CI reads retained in `final_state`.
    pub ci_requests: Vec<CiRequestEvidence>,
    pub ci_request_capture_dropped: usize,
    /// Secret-free protected-workflow publication and service facts.
    pub ci_failure_evidence: Option<LiveCiFailureEvidence>,
    pub handoff: Option<LiveHandoffEvidence>,
    pub codebase_memory: Option<LiveCodebaseMemoryEvidence>,
    pub plan_feature: Option<LivePlanFeatureEvidence>,
    /// Scenario-seeded terminal-history shape and webhook-isolation facts.
    pub terminal_history: Option<LiveTerminalHistoryEvidence>,
    pub stimuli: Vec<StimulusOutcome>,
    pub logs: LiveLogPaths,
}

/// Structured facts from a bounded terminal-history seed action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveTerminalHistoryEvidence {
    pub actionable_issue_number: u64,
    pub actionable_pull_request_number: u64,
    pub first_history_pull_request_number: u64,
    pub target_closed_issues: usize,
    pub target_closed_pull_requests: usize,
    pub sibling_repo_slug: String,
    pub sibling_closed_issues: usize,
    pub webhook_delivery: String,
    pub actionable_older_than_history: bool,
    pub actionable_recovered: bool,
    pub cold_authority_rebuilt: bool,
}

impl LiveManifestEvidence {
    /// Compact human-readable rendering for ignored-test stdout/CI logs.
    pub fn to_report(&self) -> String {
        let mut lines = vec![
            "live_manifest evidence:".to_string(),
            format!("  scenario: {}", self.scenario_path.display()),
            format!("  manifest: {}", self.manifest_path.display()),
            format!("  scenario_run_id: {}", self.scenario_run_id),
            format!(
                "  observability: TEMPER_LOG_FORMAT={} RUST_LOG={}",
                self.temper_log_format, self.rust_log
            ),
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
                "  effective_configuration: ci_poll_cadence_secs={} poll_cadence_secs={} mechanical_cadence_secs={}",
                self.effective_configuration.ci_poll_cadence_secs,
                self.effective_configuration.poll_cadence_secs,
                self.effective_configuration.mechanical_cadence_secs
            ),
            format!("  stimuli: {}", self.stimuli.len()),
            format!(
                "  fake_llm: {} architect_requests={} engineer_requests={} tester_requests={} log={}",
                self.fake_llm.base_url,
                self.fake_llm.architect_requests,
                self.fake_llm.engineer_requests,
                self.fake_llm.tester_requests,
                self.fake_llm.log_path.display()
            ),
            format!("  forge_pull_requests: {}", self.forge_pull_requests.len()),
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
        for stimulus in &self.stimuli {
            lines.push(format!(
                "  stimulus: {} action={} status={} attempts={} timeout={:?} duration={:?}",
                stimulus.id,
                stimulus.action,
                stimulus.status.as_str(),
                stimulus.attempts,
                stimulus.timeout,
                stimulus.duration
            ));
        }
        for job in &self.final_state.ci_jobs {
            lines.push(format!(
                "    - {} id={} run={:?} attempt={:?} commit={} status={} conclusion={:?} url={:?}",
                job.name,
                job.job_id,
                job.provider_run_id,
                job.provider_attempt,
                job.commit_sha,
                job.status,
                job.conclusion,
                job.url
            ));
        }
        for head in &self.final_state.ci_heads {
            lines.push(format!(
                "  ci_head: phase={} sha={} observed_after_ms={} observations={}",
                head.phase,
                head.head_sha,
                head.observed_after_ms,
                head.observations.len()
            ));
        }
        if let Some(service) = &self.ci_failure_evidence {
            lines.push(format!(
                "  ci_failure_evidence: path={} issuer={} protected_producers={:?} published_proofs={}",
                service.endpoint_path,
                service.issuer,
                service.protected_producers,
                service.published_proofs
            ));
        }
        if let Some(history) = &self.terminal_history {
            lines.push(format!(
                "  terminal_history: actionable_issue=#{} actionable_pr=#{} target_closed_issues={} target_closed_prs={} sibling={} sibling_closed_issues={} webhook={} older={} recovered={} cold_authority_rebuilt={}",
                history.actionable_issue_number,
                history.actionable_pull_request_number,
                history.target_closed_issues,
                history.target_closed_pull_requests,
                history.sibling_repo_slug,
                history.sibling_closed_issues,
                history.webhook_delivery,
                history.actionable_older_than_history,
                history.actionable_recovered,
                history.cold_authority_rebuilt,
            ));
        }
        lines.push(format!(
            "  ci_request_provenance: retained={} dropped={}",
            self.ci_requests.len(),
            self.ci_request_capture_dropped
        ));
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

/// Exact non-secret engine cadences used by the spawned standalone process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveConfigurationEvidence {
    pub ci_poll_cadence_secs: u64,
    pub poll_cadence_secs: u64,
    pub mechanical_cadence_secs: u64,
}

/// Secret-free projection of a verified ordinary CI-failure proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedFailureProofEvidence {
    pub schema_version: u16,
    pub category: String,
    pub repository_id: String,
    pub pull_request_id: Option<String>,
    pub commit_sha: String,
    pub run_id: String,
    pub job_id: String,
    pub attempt: String,
    pub task_id: Option<String>,
    pub producer_id: String,
    pub issuer_id: String,
    pub verification: String,
    pub created_at: String,
    pub expires_at: String,
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
    pub tester_requests: usize,
    pub log_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveCodebaseMemoryEvidence {
    pub produced_file: String,
    pub expected_result: String,
    pub fake_mcp_log: PathBuf,
    pub mcp_search_calls: usize,
    /// Exact tool-call inventory retained without provider response content.
    pub mcp_call_counts: Vec<(String, usize)>,
    /// Fixture delay that the first model-visible call had to await.
    pub readiness_delay_ms: u64,
    /// Provider tool that produced the controlled systemic failure, if any.
    pub forced_failure_tool: Option<String>,
    pub safe_tools: Vec<String>,
    pub hidden_tools: Vec<String>,
    pub lifecycle: Option<String>,
    /// Secret-free confirmation that a fresh stable provider identity was
    /// rebound before the model read source from the active prepared checkout.
    pub stable_rebind: Option<LiveStableRebindEvidence>,
}

/// Contract facts retained by the stable-rebind fake provider profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveStableRebindEvidence {
    pub requested_stable_project: String,
    pub confirmed_provider_project: String,
    pub retained_project_count: usize,
    pub confirmation_call_count: usize,
    pub initial_discovery_targeted: bool,
    pub normalized_provider_identity: bool,
    pub targeted_ready_confirmation: bool,
    pub fresh_prior_binding: bool,
    pub current_root_rebound: bool,
    pub graph_reads_use_confirmed_project: bool,
    pub source_reads_use_confirmed_project: bool,
    pub source_served_from_current_root: bool,
    pub global_inventory_avoided: bool,
}

/// Terminal Forge state proving the scenario converged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalStateEvidence {
    pub issue: IssueEvidence,
    pub pull_request: PullRequestEvidence,
    pub ci_jobs: Vec<CiJobEvidence>,
    /// Independently fetched snapshots used to prove provider identities remain
    /// stable across observations. Strategies that do not observe CI leave this
    /// empty and strict provenance assertions fail closed.
    pub ci_observations: Vec<CiObservationEvidence>,
    /// Named exact-head histories retained by repair scenarios.
    pub ci_heads: Vec<CiHeadEvidence>,
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
    pub merged_sha: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiJobEvidence {
    pub job_id: String,
    pub provider_run_id: Option<String>,
    pub provider_attempt: Option<String>,
    pub commit_sha: String,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub provider_conclusion: Option<String>,
    pub url: Option<String>,
    pub verified_failure: Option<VerifiedFailureProofEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiObservationEvidence {
    pub matching_provider_run: bool,
    pub jobs: Vec<CiJobEvidence>,
}

/// CI observations retained for one named PR-head phase.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiHeadEvidence {
    pub phase: String,
    pub head_sha: String,
    pub observed_after_ms: u64,
    pub jobs: Vec<CiJobEvidence>,
    pub observations: Vec<CiObservationEvidence>,
}

/// Redacted request metadata. Header and query values are intentionally absent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiRequestEvidence {
    pub method: String,
    pub path: String,
    pub query_keys: Vec<String>,
    pub authentication_present: bool,
    pub authentication_scheme: Option<String>,
    pub accepts_json: bool,
}
