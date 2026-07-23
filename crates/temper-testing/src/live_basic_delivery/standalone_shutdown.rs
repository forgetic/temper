//! Real-process Linux acceptance harness for bounded standalone shutdown.
//!
//! This is intentionally built on the public `temper init` and `temper serve
//! standalone` commands.  Fault injection is limited to the compiled MCP
//! fixture and Jig response gates; the production signal handler, daemon,
//! worker, agent, containment supervisor, durable assignment, result outbox,
//! workspace, and trace paths remain unchanged.

mod assertions;
mod config;
mod fake;
mod processes;
mod trace;

use assertions::{
    assert_old_protocol_rejected, exit_status, forge_snapshot, require_executable,
    shutdown_blocker, signal_pid, wait_for_attempt, wait_for_path, wait_for_replacement_pr,
};
use config::{ShutdownFixtureConfig, tune_shutdown_config, worker_token};
use fake::ShutdownFake;
use processes::{ExactProcessCleanup, wait_for_identities, wait_for_processes_gone};
use trace::{TraceJournalObstruction, old_trace_evidence, wait_for_journal_sequence};

use std::fs;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use jig_core::{Reply, RequestView, Script, StopReason, Turn};
use jig_server::FakeLlm;
use serde_json::{Value as JsonValue, json};
use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{ItemNumber, PullRequestQuery, RepositoryId};
use temper_protocol_worker::{
    ContextOutcome, FetchContext, ForgeContextErrorCode, ForgeContextOperation,
    ForgeGetItemOperation, JobResult, ReleaseDisposition, ResultStatus,
    WORKER_AUTHORIZATION_HEADER, WORKER_PROTOCOL_VERSION, WorkerAuth, WorkerProtocolMessage,
};
use temper_workflow::{ArtifactKindId, parse_metadata_block, replace_metadata_block};
use toml::Value as TomlValue;

use super::convergence::{admin_forge, repository, seed_intake};
use super::process::{
    ChildGuard, TemperInitRequest, assert_init_workflow_yaml_matches, engine_block_on, free_port,
    mint_site_admin_token, populate_repo, read_tail, run_temper_init, spawn_temper_standalone,
    tune_init_config, wait_for_standalone,
};
use super::{ScenarioBundle, TemperCommand};
use crate::forgejo_runtime::RunWorkspace;
use crate::forgejo_server::{ForgejoRunner, start_cached_bare_admin_server};

const WORKSPACE_PREFIX: &str = "temper-standalone-shutdown-e2e";
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(13);
const PROCESS_EXIT_BOUND: Duration = Duration::from_secs(16);
const FIXTURE_WAIT: Duration = Duration::from_secs(30);
const REPLACEMENT_WAIT: Duration = Duration::from_secs(120);
const REPLACEMENT_FILE: &str = "service/STANDALONE_RESTART_RECOVERED.md";
const REPLACEMENT_SUMMARY: &str =
    "Replacement standalone attempt completed after bounded recovery.";
const BOUNDED_CRASH_EXIT_CODE: i32 = 70;

/// Inputs whose executable paths are supplied by Cargo rather than discovered
/// through ambient environment variables or recursive Cargo invocations.
pub struct StandaloneShutdownRequest {
    pub scenario: ScenarioBundle,
    pub temper: TemperCommand,
    pub descendant_fixture: PathBuf,
}

/// Durable evidence retained by a successful capstone run.
pub struct StandaloneShutdownEvidence {
    _workspace: RunWorkspace,
    pub forge_url: String,
    pub repository: String,
    pub issue: u64,
    pub old_job_id: String,
    pub old_attempt_id: String,
    pub replacement_attempt_id: String,
    pub daemon_pid: u32,
    pub shutdown_elapsed: Duration,
    pub shutdown_status: String,
    pub blocker: ShutdownBlockerEvidence,
    pub recorded_processes: Vec<RecordedProcessIdentity>,
    pub old_trace_run_id: String,
    pub old_trace_pending_before_restart: bool,
    pub old_trace_forwarded_sequence: u64,
    pub implementation_pull_requests: usize,
    pub first_log: PathBuf,
    pub replacement_log: PathBuf,
    pub state_root: PathBuf,
    pub result_root: PathBuf,
    pub workspace_root: PathBuf,
    pub trace_spool_root: PathBuf,
}

impl StandaloneShutdownEvidence {
    /// Compact report suitable for `--nocapture` and CI artifact logs.
    pub fn to_report(&self) -> String {
        format!(
            "standalone shutdown acceptance evidence:\n  forge: {} repo={} issue=#{}\n  old: job={} attempt={} daemon_pid={}\n  replacement_attempt={} implementation_prs={}\n  shutdown: {:?} status={} blocker={} owner={}/{} root_pid={} phase={} age_ms={} escalation={} disposition={}\n  descendants: {} exact PID/start identities (all gone)\n  old_trace: run={} pending_before_restart={} forwarded_sequence={}\n  durable roots: state={} result={} workspace={} trace_spool={}\n  logs: first={} replacement={}",
            self.forge_url,
            self.repository,
            self.issue,
            self.old_job_id,
            self.old_attempt_id,
            self.daemon_pid,
            self.replacement_attempt_id,
            self.implementation_pull_requests,
            self.shutdown_elapsed,
            self.shutdown_status,
            self.blocker.kind,
            self.blocker.owner_scope,
            self.blocker.owner_name,
            self.blocker.root_pid,
            self.blocker.containment_phase,
            self.blocker.age_millis,
            self.blocker.escalation_stage,
            self.blocker.disposition,
            self.recorded_processes.len(),
            self.old_trace_run_id,
            self.old_trace_pending_before_restart,
            self.old_trace_forwarded_sequence,
            self.state_root.display(),
            self.result_root.display(),
            self.workspace_root.display(),
            self.trace_spool_root.display(),
            self.first_log.display(),
            self.replacement_log.display(),
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShutdownBlockerEvidence {
    pub worker_id: String,
    pub job_id: String,
    pub attempt_id: String,
    pub kind: String,
    pub owner_scope: String,
    pub owner_name: String,
    pub owner_root: String,
    pub root_pid: u32,
    pub containment_phase: String,
    pub first_seen_millis: u64,
    pub age_millis: u64,
    pub escalation_stage: String,
    pub deadline_remaining_millis: u64,
    pub disposition: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedProcessIdentity {
    pub role: String,
    pub pid: u32,
    pub start_time: u64,
    pub parent_pid: u32,
    pub process_group: u32,
    pub session: u32,
    pub executable: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttemptIdentity {
    worker_id: String,
    job_id: String,
    attempt_id: String,
    daemon_boot_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ForgeSnapshot {
    body: String,
    labels: Vec<String>,
    assignees: Vec<String>,
    comments: Vec<String>,
    pull_requests: Vec<u64>,
}

/// Runs the public-binary SIGTERM/crash-handoff/restart capstone.
pub fn run_standalone_shutdown_acceptance(
    request: StandaloneShutdownRequest,
) -> Result<StandaloneShutdownEvidence, String> {
    request.scenario.assert_workflow_matches_reference()?;
    require_executable(&request.descendant_fixture, "standalone descendant fixture")?;

    let cached = start_cached_bare_admin_server(
        "basicadmin",
        "Basic-Delivery-Admin-1!",
        "basicadmin@example.invalid",
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
    let admin_token = mint_site_admin_token(&server, "basicadmin")?;
    let fake = ShutdownFake::start()?;
    let workspace = RunWorkspace::new(WORKSPACE_PREFIX);
    let bundle_dir = workspace.dir("bundle");
    let workspace_root = workspace.dir("workspaces");
    let state_root = workspace.dir("durable-state");
    let result_root = state_root.join("worker-results");
    let trace_spool_root = state_root.join("agent-traces/worker-spool");
    let identities_path = workspace.join("descendant-identities.tsv");
    let fixture_ready = workspace.join("descendant-ready");
    let obstruction_trigger = workspace.join("obstruct-recursive-empty");
    let obstruction_ready = workspace.join("recursive-empty-obstructed");
    let init_log = workspace.join("logs/init.log");
    let populate_log = workspace.join("logs/repo-populate.log");
    let first_log = workspace.join("logs/standalone-first.log");
    let replacement_log = workspace.join("logs/standalone-replacement.log");
    let scenario_run_id = format!(
        "standalone-shutdown-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );

    let bind_port = free_port()?;
    run_temper_init(TemperInitRequest {
        temper: &request.temper,
        server: &server,
        scenario: &request.scenario,
        bundle_dir: &bundle_dir,
        workspaces_dir: &workspace_root,
        bind_port,
        fake_llm_url: &fake.base_url(),
        log: &init_log,
        admin_user: "basicadmin",
        admin_password: "Basic-Delivery-Admin-1!",
        scenario_run_id: &scenario_run_id,
    })?;
    assert_init_workflow_yaml_matches(&bundle_dir.join("workflow.yaml"), &request.scenario)?;
    tune_init_config(&bundle_dir.join("config.toml"), 1, 1)?;
    tune_shutdown_config(
        &bundle_dir.join("config.toml"),
        &ShutdownFixtureConfig {
            fixture: &request.descendant_fixture,
            identities: &identities_path,
            ready: &fixture_ready,
            obstruction_trigger: &obstruction_trigger,
            obstruction_ready: &obstruction_ready,
            state_root: &state_root,
            workspace_root: &workspace_root,
        },
    )?;
    let worker_token = worker_token(&bundle_dir)?;

    populate_repo(
        server.base_url(),
        &admin_token,
        workspace.path(),
        &request.scenario.repo,
        &populate_log,
    )?;

    let forge = admin_forge(server.base_url(), &admin_token, &request.scenario.repo);
    let repository = engine_block_on(repository(&forge, &request.scenario.repo))?;
    let mut intake = request.scenario.intake.clone();
    intake.labels = vec!["code".to_string(), "ready".to_string()];
    let mut source_metadata = parse_metadata_block(&intake.body)
        .map_err(|error| format!("parse intake metadata: {error}"))?
        .unwrap_or_default();
    source_metadata.kind = Some(ArtifactKindId::new("code"));
    intake.body = replace_metadata_block(&intake.body, &source_metadata)
        .map_err(|error| format!("stamp intake code kind: {error}"))?;

    let mut first = spawn_temper_standalone(
        &request.temper,
        &bundle_dir,
        &first_log,
        &request.scenario.observability,
        &scenario_run_id,
    )?;
    wait_for_standalone(&mut first)?;
    let daemon_pid = first.pid();
    let issue = engine_block_on(seed_intake(&forge, &repository, &intake))?;

    fake.wait_for_old_request(FIXTURE_WAIT)?;
    wait_for_path(&fixture_ready, FIXTURE_WAIT, "compiled MCP descendant")?;
    let old_attempt = wait_for_attempt(&forge, &repository, issue, None, &mut first, FIXTURE_WAIT)?;
    let before_fence = forge_snapshot(&forge, &repository, issue)?;
    let old_processes = wait_for_identities(&identities_path, 3, FIXTURE_WAIT)?;
    let process_cleanup = ExactProcessCleanup::new(old_processes.clone());
    let supervisor_pid = old_processes
        .iter()
        .find(|identity| identity.role == "standalone-mcp-supervisor")
        .map(|identity| identity.pid)
        .ok_or_else(|| "compiled MCP fixture did not record its Temper supervisor".to_string())?;
    // Hold the engine journal's public cross-process lock only after the active
    // run and its initial events exist. The cancelled terminal remains durable
    // in the worker spool, while forwarding blocks in an already-admitted
    // daemon operation. Releasing this lock after process death lets the same
    // public binary prove startup forwarding from that pending cursor.
    let trace_journal_obstruction =
        TraceJournalObstruction::acquire(&state_root.join("agent-traces/journal"), FIXTURE_WAIT)?;

    let shutdown_started = Instant::now();
    first.signal("TERM")?;
    // The model response is released only after the real signal handler has had
    // an opportunity to close daemon and attempt admission. It is a valid
    // success result, so any surviving PR/label/comment mutation would be a
    // direct stale-attempt fence failure rather than malformed-model behavior.
    std::thread::sleep(Duration::from_millis(150));
    fake.release_old_result();
    std::thread::sleep(Duration::from_millis(350));
    fs::write(
        &obstruction_trigger,
        b"stop supervisor during recursive cleanup\n",
    )
    .map_err(|error| format!("arm recursive-empty obstruction: {error}"))?;
    wait_for_path(
        &obstruction_ready,
        Duration::from_secs(5),
        "recursive-empty obstruction",
    )?;

    let first_status = first.wait_for_exit(PROCESS_EXIT_BOUND)?;
    let shutdown_elapsed = shutdown_started.elapsed();
    let shutdown_status = exit_status(&first_status);
    if shutdown_elapsed > PROCESS_EXIT_BOUND {
        return Err(format!(
            "standalone exceeded its fixed signal-to-exit bound: {shutdown_elapsed:?}"
        ));
    }
    if first_status.success()
        || first_status.code() != Some(BOUNDED_CRASH_EXIT_CODE)
        || first_status.signal().is_some()
        || first_status.core_dumped()
    {
        return Err(format!(
            "deliberately obstructed standalone exited as {shutdown_status}, expected core-dump-free immediate exit:{BOUNDED_CRASH_EXIT_CODE} bounded crash handoff\n{}",
            first.log_tail()
        ));
    }
    if !fake.old_result_released() {
        return Err("late Jig result was not released after SIGTERM".to_string());
    }
    trace_journal_obstruction.release()?;

    let blocker = shutdown_blocker(&first_log, &old_attempt, supervisor_pid)?;
    let after_fence = forge_snapshot(&forge, &repository, issue)?;
    if after_fence != before_fence {
        return Err(format!(
            "late old-attempt effects crossed the closed fence\nbefore={before_fence:?}\nafter={after_fence:?}"
        ));
    }

    // The test never sends KILL. SIGCONT only releases the deliberate helper
    // obstruction; the already-queued Temper emergency KILL and owner-loss
    // fallback remain the authorities that terminate and reap the fixture.
    signal_pid(supervisor_pid, "CONT")?;
    wait_for_processes_gone(&old_processes, Duration::from_secs(10))?;
    let _ = fs::remove_file(&obstruction_trigger);

    let old_trace = old_trace_evidence(&trace_spool_root, &old_attempt.job_id)?;
    fake.begin_replacement();
    let mut replacement = spawn_temper_standalone(
        &request.temper,
        &bundle_dir,
        &replacement_log,
        &request.scenario.observability,
        &scenario_run_id,
    )?;
    wait_for_standalone(&mut replacement)?;
    fake.wait_for_replacement_request(REPLACEMENT_WAIT)
        .map_err(|error| {
            format!(
                "{error}\n--- replacement standalone log ---\n{}\n--- Jig requests ---\n{}\n--- source snapshot ---\n{:?}",
                replacement.log_tail(),
                fake.log_tail(),
                forge_snapshot(&forge, &repository, issue)
            )
        })?;
    let replacement_attempt = wait_for_attempt(
        &forge,
        &repository,
        issue,
        Some(&old_attempt.attempt_id),
        &mut replacement,
        REPLACEMENT_WAIT,
    )?;
    if replacement_attempt.job_id != old_attempt.job_id {
        return Err(format!(
            "startup requeue changed deterministic job identity: old={} replacement={}",
            old_attempt.job_id, replacement_attempt.job_id
        ));
    }
    if replacement_attempt.daemon_boot_id == old_attempt.daemon_boot_id {
        return Err("replacement assignment retained the prior daemon boot identity".to_string());
    }

    let before_old_protocol = forge_snapshot(&forge, &repository, issue)?;
    assert_old_protocol_rejected(
        bind_port,
        &worker_token,
        &request.scenario.repo.slug,
        issue,
        &old_attempt,
    )?;
    let after_old_protocol = forge_snapshot(&forge, &repository, issue)?;
    if after_old_protocol != before_old_protocol {
        return Err(format!(
            "old-attempt result/context changed Forge state after replacement ownership\nbefore={before_old_protocol:?}\nafter={after_old_protocol:?}"
        ));
    }
    let forwarded_sequence = wait_for_journal_sequence(
        &state_root.join("agent-traces/journal"),
        &old_trace.run_id,
        old_trace.last_sequence,
        Duration::from_secs(15),
    )?;

    fake.release_replacement();
    let pull_requests = wait_for_replacement_pr(
        &forge,
        &repository,
        issue,
        &mut replacement,
        REPLACEMENT_WAIT,
    )?;
    if fake.replacement_sessions() != 1 {
        return Err(format!(
            "startup recovery dispatched {} replacement agent sessions, expected exactly one",
            fake.replacement_sessions()
        ));
    }

    replacement.signal("TERM")?;
    let replacement_status = replacement.wait_for_exit(PROCESS_EXIT_BOUND)?;
    if !replacement_status.success() {
        return Err(format!(
            "replacement standalone did not stop gracefully after completing the sole replacement attempt: {replacement_status}\n{}",
            replacement.log_tail()
        ));
    }
    let all_processes =
        wait_for_identities(&identities_path, old_processes.len() + 3, FIXTURE_WAIT)?;
    wait_for_processes_gone(&all_processes, Duration::from_secs(10))?;
    process_cleanup.disarm();

    Ok(StandaloneShutdownEvidence {
        _workspace: workspace,
        forge_url: server.base_url().to_string(),
        repository: request.scenario.repo.slug,
        issue: issue.get(),
        old_job_id: old_attempt.job_id,
        old_attempt_id: old_attempt.attempt_id,
        replacement_attempt_id: replacement_attempt.attempt_id,
        daemon_pid,
        shutdown_elapsed,
        shutdown_status,
        blocker,
        recorded_processes: all_processes,
        old_trace_run_id: old_trace.run_id,
        old_trace_pending_before_restart: old_trace.pending,
        old_trace_forwarded_sequence: forwarded_sequence,
        implementation_pull_requests: pull_requests,
        first_log,
        replacement_log,
        state_root,
        result_root,
        workspace_root,
        trace_spool_root,
    })
}
