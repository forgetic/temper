use serde::Serialize;

use super::*;

pub(super) fn emit_cleanup_blocked(event: &CleanupBlocked) {
    let owner = &event.owner;
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "temper::worker",
                $level,
                service = "worker",
                event = "worker.containment.cleanup_blocked",
                worker_id = owner.context.worker_id.as_str(),
                job_id = owner.context.job_id.as_str(),
                attempt_id = owner.context.attempt_id.as_str(),
                owner_kind = owner.owner_kind.as_str(),
                tool_command_id = owner.tool_command_id.as_str(),
                backend = owner.backend,
                root = owner.root.as_str(),
                trigger = event.trigger,
                phase = event.phase,
                repeated_failures = event.repeated_failures,
                term_syscall_outcomes = event.term_outcomes.as_str(),
                omitted_term_outcomes = event.omitted_term_outcomes,
                kill_syscall_outcomes = event.kill_outcomes.as_str(),
                omitted_kill_outcomes = event.omitted_kill_outcomes,
                direct_child_reap = event.direct_child_reap,
                direct_child_pid = event.direct_child_pid,
                recursive_empty = event.recursive_empty,
                recursive_empty_inspections = event.recursive_empty_inspections,
                survivors = event.survivors.as_str(),
                omitted_survivors = event.omitted_survivors,
                "descendant cleanup is blocked and completion remains fenced"
            );
        };
    }
    if event.trigger == "shutdown" || event.repeated_failures >= 3 {
        emit!(tracing::Level::ERROR);
    } else {
        emit!(tracing::Level::WARN);
    }
}

pub(super) fn emit_cleanup_completed(event: &CleanupCompleted) {
    let owner = &event.owner;
    let recovered = event.disposition != "already_empty"
        || event.recovered_inspection_failures > 0
        || event.omitted_inspection_failures > 0;
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "temper::worker",
                $level,
                service = "worker",
                event = "worker.containment.cleanup_completed",
                worker_id = owner.context.worker_id.as_str(),
                job_id = owner.context.job_id.as_str(),
                attempt_id = owner.context.attempt_id.as_str(),
                owner_kind = owner.owner_kind.as_str(),
                tool_command_id = owner.tool_command_id.as_str(),
                backend = owner.backend,
                root = owner.root.as_str(),
                trigger = event.trigger,
                disposition = event.disposition,
                term_syscall_outcomes = event.term_outcomes.as_str(),
                omitted_term_outcomes = event.omitted_term_outcomes,
                kill_syscall_outcomes = event.kill_outcomes.as_str(),
                omitted_kill_outcomes = event.omitted_kill_outcomes,
                direct_child_reap = event.direct_child_reap,
                direct_child_pid = event.direct_child_pid,
                direct_child_exit_code = event.direct_child_exit_code,
                recursive_empty = event.recursive_empty,
                recursive_empty_inspections = event.recursive_empty_inspections,
                survivors = event.survivors.as_str(),
                omitted_survivors = event.omitted_survivors,
                recovered_inspection_failures = event.recovered_inspection_failures,
                omitted_inspection_failures = event.omitted_inspection_failures,
                "descendant cleanup completed with recursive-empty proof"
            );
        };
    }
    if recovered {
        emit!(tracing::Level::WARN);
    } else {
        emit!(tracing::Level::DEBUG);
    }
}

pub(super) fn emit_fallback(event: &ContainmentFallbackActivated) {
    let owner = &event.owner;
    tracing::warn!(
        target: "temper::worker",
        service = "worker",
        event = "worker.containment.fallback_activated",
        worker_id = owner.context.worker_id.as_str(),
        job_id = owner.context.job_id.as_str(),
        attempt_id = owner.context.attempt_id.as_str(),
        owner_kind = owner.owner_kind.as_str(),
        tool_command_id = owner.tool_command_id.as_str(),
        backend = owner.backend,
        root = owner.root.as_str(),
        fallback_reason = event.fallback_reason.as_str(),
        term_syscall_outcomes = event.term_outcomes.as_str(),
        kill_syscall_outcomes = event.kill_outcomes.as_str(),
        direct_child_reap = event.direct_child_reap,
        recursive_empty = event.recursive_empty,
        survivors = event.survivors.as_str(),
        "delegated cgroup-v2 containment was unavailable; descendant-complete fallback activated"
    );
}

pub(super) fn emit_startup(event: &ContainmentStartupCapability) {
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "temper::worker",
                $level,
                service = "worker",
                event = "worker.containment.startup_capability",
                worker_id = event.worker_id.as_str(),
                cgroup_v2_mount = event.cgroup_v2_mount.as_str(),
                delegation = event.delegation,
                nested_subtree_writable = event.nested_subtree_writable,
                cgroup_kill = event.cgroup_kill,
                pidfd = event.pidfd,
                selected_backend = event.selected_backend,
                fallback_reason = event.fallback_reason.as_str(),
                "worker selected its descendant-containment backend"
            );
        };
    }
    if event.selected_backend == "linux_cgroup_v2" || event.selected_backend == "windows_job" {
        emit!(tracing::Level::DEBUG);
    } else {
        emit!(tracing::Level::WARN);
    }
}

pub(super) fn emit_startup_scavenge(event: &ContainmentStartupScavenge) {
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(
                target: "temper::worker",
                $level,
                service = "worker",
                event = "worker.containment.startup_scavenge",
                worker_id = event.worker_id.as_str(),
                removed_count = event.removed_count,
                protected_count = event.protected_count,
                retained_count = event.retained_count,
                retained_diagnostics = event.retained_diagnostics.as_str(),
                omitted_diagnostics = event.omitted_diagnostics,
                "worker scavenged stale delegated cgroups before accepting jobs"
            );
        };
    }
    if event.retained_count > 0 || event.omitted_diagnostics > 0 {
        emit!(tracing::Level::WARN);
    } else {
        emit!(tracing::Level::DEBUG);
    }
}

#[derive(Serialize)]
struct SerializableProcess {
    pid: u32,
    ppid: u32,
    pgid: u32,
    session_id: u32,
    start_time: u64,
    executable: String,
}

impl From<&ProcessIdentity> for SerializableProcess {
    fn from(process: &ProcessIdentity) -> Self {
        Self {
            pid: process.pid(),
            ppid: process.ppid(),
            pgid: process.process_group_id(),
            session_id: process.session_id(),
            start_time: process.start_time_identity(),
            executable: bounded_diagnostic(
                &process.executable().to_string_lossy(),
                MAX_EVENT_EXECUTABLE_BYTES,
            ),
        }
    }
}

#[derive(Serialize)]
struct SerializableSignalAttempt {
    process: SerializableProcess,
    outcome: &'static str,
}

pub(super) fn serialize_processes(processes: &[ProcessIdentity]) -> String {
    let processes = processes
        .iter()
        .take(MAX_EVENT_SURVIVORS)
        .map(SerializableProcess::from)
        .collect::<Vec<_>>();
    serde_json::to_string(&processes).expect("bounded process evidence serializes")
}

pub(super) fn serialize_signal_attempts(attempts: &[SignalAttempt]) -> String {
    let attempts = attempts
        .iter()
        .take(MAX_EVENT_SIGNAL_OUTCOMES)
        .map(|attempt| SerializableSignalAttempt {
            process: SerializableProcess::from(attempt.process()),
            outcome: match attempt.outcome() {
                SignalAttemptOutcome::Succeeded => "succeeded",
                SignalAttemptOutcome::ProcessGone => "process_gone",
                SignalAttemptOutcome::PidReused => "pid_reused",
                // Error bodies are intentionally omitted: the typed outcome is
                // sufficient operator evidence and cannot carry secrets.
                SignalAttemptOutcome::Failed(_) => "failed",
            },
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&attempts).expect("bounded signal evidence serializes")
}

pub(super) fn reap_fields(reap: &DirectChildReap) -> (&'static str, u32, i64) {
    match reap {
        DirectChildReap::NotSpawned => ("not_spawned", 0, -1),
        DirectChildReap::Pending { pid } => ("pending", *pid, -1),
        DirectChildReap::Reaped { pid, exit_code } => {
            ("reaped", *pid, exit_code.map_or(-1, i64::from))
        }
        DirectChildReap::AlreadyReaped { pid, exit_code } => {
            ("already_reaped", *pid, exit_code.map_or(-1, i64::from))
        }
    }
}

pub(super) fn recursive_empty_fields(proof: &RecursiveEmptyProof) -> (&'static str, u64) {
    match proof {
        RecursiveEmptyProof::Proven { inspections } => ("proven", *inspections),
        RecursiveEmptyProof::NotEmpty { .. } => ("not_empty", 0),
    }
}

pub(super) fn owner_kind(scope: &ContainmentScope) -> String {
    let name = match scope {
        ContainmentScope::Job => "job",
        ContainmentScope::Tool => "tool",
        ContainmentScope::Agent => "agent",
        ContainmentScope::McpServer => "mcp_server",
        ContainmentScope::WorkerCommand => "worker_command",
        ContainmentScope::PrePush => "pre_push",
        ContainmentScope::Custom(name) => name,
    };
    bounded(name, MAX_EVENT_IDENTIFIER_BYTES)
}

pub(super) const fn backend_name(backend: ContainmentBackendKind) -> &'static str {
    match backend {
        ContainmentBackendKind::NoProcess => "no_process",
        ContainmentBackendKind::LinuxCgroupV2 => "linux_cgroup_v2",
        ContainmentBackendKind::LinuxSupervisor => "linux_supervisor",
        ContainmentBackendKind::WindowsJob => "windows_job",
    }
}

pub(super) const fn trigger_name(trigger: CleanupTrigger) -> &'static str {
    match trigger {
        CleanupTrigger::NormalRootExit => "normal_root_exit",
        CleanupTrigger::Timeout => "timeout",
        CleanupTrigger::Cancellation => "cancellation",
        CleanupTrigger::OwnerDrop => "owner_drop",
        CleanupTrigger::Watchdog => "watchdog",
        CleanupTrigger::Shutdown => "shutdown",
    }
}

pub(super) const fn phase_name(phase: CleanupPhase) -> &'static str {
    match phase {
        CleanupPhase::Discover => "discover",
        CleanupPhase::Term => "term",
        CleanupPhase::Grace => "grace",
        CleanupPhase::Kill => "kill",
        CleanupPhase::Reap => "reap",
        CleanupPhase::VerifyEmpty => "verify_empty",
    }
}

pub(super) const fn disposition_name(disposition: CleanupDisposition) -> &'static str {
    match disposition {
        CleanupDisposition::AlreadyEmpty => "already_empty",
        CleanupDisposition::Terminated => "terminated",
        CleanupDisposition::Killed => "killed",
    }
}

pub(super) fn bounded_diagnostic(value: &str, limit: usize) -> String {
    let lower = value.to_ascii_lowercase();
    if [
        "secret-token-sentinel",
        "authorization:",
        "bearer ",
        "credential=",
        "password=",
        "token=",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "[redacted]".to_string();
    }
    bounded(value, limit)
}

pub(super) fn bounded(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}
