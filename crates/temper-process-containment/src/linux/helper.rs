use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crate::{ContainmentSignal, ProcessIdentity, SignalAttempt, SignalBatch};

use super::HELPER_MODE;
use super::process::{
    MemberRevalidation, PidFd, TrackedMember, descendants_of, revalidate_member, scan_proc,
};
use super::protocol::{
    AutomaticSignalEvidence, send_attempts, send_error, send_final, send_members,
};

mod arguments;
mod emergency;

use arguments::HelperArguments;

const HELPER_SETUP_FAILURE: u8 = 125;

/// Dispatch the hidden Linux helper mode before logging, runtimes, or ordinary
/// CLI argument parsing starts. Returns `None` for every public invocation.
#[doc(hidden)]
pub(super) fn dispatch(arguments: impl IntoIterator<Item = OsString>) -> Option<ExitCode> {
    let mut arguments = arguments.into_iter();
    if arguments.next().as_deref() != Some(OsStr::new(HELPER_MODE)) {
        return None;
    }
    Some(match arguments::parse(arguments) {
        Ok(arguments) => match run_supervisor_helper(arguments) {
            Ok(status) => mirror_payload_status(status),
            Err(_) => ExitCode::from(HELPER_SETUP_FAILURE),
        },
        Err(_) => ExitCode::from(HELPER_SETUP_FAILURE),
    })
}

fn run_supervisor_helper(arguments: HelperArguments) -> io::Result<PayloadStatus> {
    // SAFETY: the fd was inherited specifically for helper mode and ownership
    // is transferred exactly once into this UnixStream.
    let mut channel = unsafe { std::os::unix::net::UnixStream::from_raw_fd(arguments.control_fd) };
    // SAFETY: this second fd is the independently inherited emergency owner
    // channel and is transferred exactly once to the dedicated reader thread.
    let emergency_channel =
        unsafe { std::os::unix::net::UnixStream::from_raw_fd(arguments.emergency_fd) };
    if let Err(error) = become_subreaper().and_then(|()| {
        set_close_on_exec(arguments.control_fd, true)?;
        set_close_on_exec(arguments.emergency_fd, true)
    }) {
        let _ = send_error(&mut channel, &error);
        return Err(error);
    }

    let mut payload = std::process::Command::new(arguments.payload_program);
    payload.args(arguments.payload_arguments);
    let mut payload = match payload.spawn() {
        Ok(payload) => payload,
        Err(error) => {
            let _ = send_error(&mut channel, &error);
            return Err(error);
        }
    };
    let payload_pid = match i32::try_from(payload.id()) {
        Ok(pid) => pid,
        Err(_) => {
            let error = io::Error::other("payload pid does not fit pid_t");
            let _ = payload.kill();
            let _ = payload.wait();
            let _ = send_error(&mut channel, &error);
            return Err(error);
        }
    };
    let emergency_stage = match emergency::spawn_owner(
        emergency_channel,
        std::process::id(),
        arguments.term_grace,
        arguments.inspection_retry,
    ) {
        Ok(stage) => stage,
        Err(error) => {
            let _ = payload.kill();
            let _ = payload.wait();
            let error = io::Error::other(format!("spawn Linux emergency owner: {error}"));
            let _ = send_error(&mut channel, &error);
            return Err(error);
        }
    };
    drop(payload);

    let mut supervisor = SupervisorState::new(
        std::process::id(),
        payload_pid,
        arguments.term_grace,
        arguments.inspection_retry,
    );
    close_payload_stdio_copies();
    let mut owner_present = writeln!(channel, "R\t{payload_pid}")
        .and_then(|()| channel.flush())
        .is_ok();
    let mut automatic_cleanup = !owner_present;

    loop {
        if let Err(error) = supervisor.reap_eligible_children() {
            helper_blocked(&mut channel, &mut owner_present, &supervisor, &error);
            continue;
        }
        if supervisor.payload_status.is_some() || !owner_present || emergency_stage.requested() {
            automatic_cleanup = true;
        }

        if automatic_cleanup {
            let empty = match supervisor.prove_empty() {
                Ok(empty) => empty,
                Err(error) => {
                    helper_blocked(&mut channel, &mut owner_present, &supervisor, &error);
                    continue;
                }
            };
            if empty {
                let Some(status) = supervisor.payload_status else {
                    let error =
                        io::Error::other("containment became empty without payload wait status");
                    helper_blocked(&mut channel, &mut owner_present, &supervisor, &error);
                    continue;
                };
                if owner_present {
                    let _ = send_final(
                        &mut channel,
                        supervisor.inspections,
                        status.protocol_value(),
                        AutomaticSignalEvidence {
                            attempted: supervisor.automatic_term_attempted,
                            attempts: &supervisor.automatic_term_attempts,
                            omitted: supervisor.automatic_omitted_term_attempts,
                        },
                        AutomaticSignalEvidence {
                            attempted: supervisor.automatic_kill_attempted,
                            attempts: &supervisor.automatic_kill_attempts,
                            omitted: supervisor.automatic_omitted_kill_attempts,
                        },
                    );
                }
                return Ok(status);
            }
            if !supervisor.owner_cleanup_active || !owner_present || emergency_stage.requested() {
                let signal_result = if emergency_stage.hard_kill_requested()
                    && supervisor
                        .last_kill_at
                        .is_none_or(|sent| sent.elapsed() >= supervisor.inspection_retry)
                {
                    supervisor.automatic_signal_all(ContainmentSignal::Kill)
                } else if supervisor.term_sent_at.is_none() {
                    supervisor.automatic_signal_all(ContainmentSignal::Term)
                } else if supervisor
                    .term_sent_at
                    .is_some_and(|sent| sent.elapsed() >= supervisor.term_grace)
                    && supervisor
                        .last_kill_at
                        .is_none_or(|sent| sent.elapsed() >= supervisor.inspection_retry)
                {
                    supervisor.automatic_signal_all(ContainmentSignal::Kill)
                } else {
                    Ok(())
                };
                if let Err(error) = signal_result {
                    helper_blocked(&mut channel, &mut owner_present, &supervisor, &error);
                    continue;
                }
            }
        }

        if !owner_present {
            std::thread::sleep(supervisor.inspection_retry);
            continue;
        }

        let timeout = if automatic_cleanup {
            supervisor.inspection_retry
        } else {
            Duration::from_millis(25)
        };
        match poll_command(channel.as_raw_fd(), timeout) {
            Ok(CommandPoll::Timeout) => {}
            Ok(CommandPoll::OwnerLost) => {
                owner_present = false;
            }
            Ok(CommandPoll::Command(command)) => {
                if let Err(error) = handle_helper_command(command, &mut supervisor, &mut channel) {
                    if send_error(&mut channel, &error).is_err() {
                        owner_present = false;
                    }
                }
            }
            Err(_) => {
                // A broken control channel is abrupt owner loss, not permission
                // for the subreaper to abandon its payload.
                owner_present = false;
            }
        }
    }
}

fn helper_blocked(
    channel: &mut std::os::unix::net::UnixStream,
    owner_present: &mut bool,
    supervisor: &SupervisorState,
    error: &io::Error,
) {
    if *owner_present && send_error(channel, error).is_err() {
        *owner_present = false;
    }
    std::thread::sleep(supervisor.inspection_retry);
}

fn handle_helper_command(
    command: u8,
    supervisor: &mut SupervisorState,
    channel: &mut std::os::unix::net::UnixStream,
) -> io::Result<()> {
    match command {
        b'D' => {
            // Discovery is issued only by the shared cleanup state machine, so
            // it transfers escalation ownership to that caller.
            supervisor.owner_cleanup_active = true;
            supervisor.reap_eligible_children()?;
            let members = supervisor.discover_validated()?;
            send_members(channel, &members)?;
        }
        b'T' => {
            supervisor.owner_cleanup_active = true;
            let batch = supervisor.signal_all(ContainmentSignal::Term)?;
            send_attempts(channel, &batch)?;
        }
        b'K' => {
            supervisor.owner_cleanup_active = true;
            let batch = supervisor.signal_all(ContainmentSignal::Kill)?;
            send_attempts(channel, &batch)?;
        }
        b'V' => {
            if supervisor.prove_empty()? {
                writeln!(channel, "E\t{}", supervisor.inspections)?;
                channel.flush()?;
            } else {
                let members = supervisor.current_identities();
                send_members(channel, &members)?;
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unknown Linux supervisor command",
            ));
        }
    }
    Ok(())
}

struct SupervisorState {
    supervisor_pid: u32,
    payload_pid: i32,
    payload_status: Option<PayloadStatus>,
    tracked: BTreeMap<u32, TrackedMember>,
    term_grace: Duration,
    inspection_retry: Duration,
    inspections: u64,
    term_sent_at: Option<Instant>,
    last_kill_at: Option<Instant>,
    owner_cleanup_active: bool,
    automatic_term_attempted: bool,
    automatic_term_attempts: Vec<SignalAttempt>,
    automatic_omitted_term_attempts: usize,
    automatic_kill_attempted: bool,
    automatic_kill_attempts: Vec<SignalAttempt>,
    automatic_omitted_kill_attempts: usize,
}

impl SupervisorState {
    fn new(
        supervisor_pid: u32,
        payload_pid: i32,
        term_grace: Duration,
        inspection_retry: Duration,
    ) -> Self {
        Self {
            supervisor_pid,
            payload_pid,
            payload_status: None,
            tracked: BTreeMap::new(),
            term_grace,
            inspection_retry: inspection_retry.max(Duration::from_millis(1)),
            inspections: 0,
            term_sent_at: None,
            last_kill_at: None,
            owner_cleanup_active: false,
            automatic_term_attempted: false,
            automatic_term_attempts: Vec::new(),
            automatic_omitted_term_attempts: 0,
            automatic_kill_attempted: false,
            automatic_kill_attempts: Vec::new(),
            automatic_omitted_kill_attempts: 0,
        }
    }

    /// Reap every eligible direct/adopted zombie and report whether at least
    /// one live direct child remains. `waitpid(-1, WNOHANG) == 0` is an
    /// independent ownership signal even if procfs cannot render that child.
    fn reap_eligible_children(&mut self) -> io::Result<bool> {
        loop {
            let mut status = 0;
            // SAFETY: `status` is valid writable storage and waitpid(-1,
            // WNOHANG) can reap only this dedicated subreaper's direct/adopted
            // children.
            let waited = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
            if waited > 0 {
                if waited == self.payload_pid {
                    self.payload_status = Some(PayloadStatus::from_wait_status(status));
                }
                continue;
            }
            if waited == 0 {
                return Ok(true);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                return Ok(false);
            }
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
    }

    fn discover_validated(&mut self) -> io::Result<Vec<ProcessIdentity>> {
        self.inspections = self.inspections.saturating_add(1);
        let first = scan_proc()?;
        let candidates = descendants_of(self.supervisor_pid, &first);
        let mut opened = BTreeMap::new();
        for pid in candidates {
            let Some(stat) = first.get(&pid) else {
                continue;
            };
            match PidFd::open(pid) {
                Ok(pidfd) => {
                    opened.insert(pid, (stat.clone(), pidfd));
                }
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => return Err(error),
            }
        }

        // Opening a pidfd pins the process identity. Re-read procfs after every
        // fd is open and accept it only if start time and ancestry still match.
        let second = scan_proc()?;
        let second_descendants = descendants_of(self.supervisor_pid, &second);
        let mut next = BTreeMap::new();
        for (pid, (original, pidfd)) in opened {
            let Some(revalidated) = second.get(&pid) else {
                continue;
            };
            if original.start_time != revalidated.start_time || !second_descendants.contains(&pid) {
                continue;
            }
            next.insert(
                pid,
                TrackedMember {
                    identity: revalidated.identity(),
                    pidfd,
                },
            );
        }
        if next.is_empty() && self.reap_eligible_children()? {
            return Err(io::Error::other(
                "a direct/adopted subreaper child is not visible in procfs",
            ));
        }
        self.tracked = next;
        Ok(self.current_identities())
    }

    fn current_identities(&self) -> Vec<ProcessIdentity> {
        self.tracked
            .values()
            .map(|member| member.identity.clone())
            .collect()
    }

    fn signal_all(&mut self, signal: ContainmentSignal) -> io::Result<SignalBatch> {
        let sent_at = Instant::now();
        match signal {
            ContainmentSignal::Term => {
                self.term_sent_at.get_or_insert(sent_at);
            }
            ContainmentSignal::Kill => self.last_kill_at = Some(sent_at),
        }
        let _ = self.discover_validated()?;
        let current = scan_proc()?;
        let descendants = descendants_of(self.supervisor_pid, &current);
        let mut attempts = Vec::with_capacity(self.tracked.len().min(crate::MAX_SIGNAL_ATTEMPTS));
        let mut omitted = 0_usize;
        for member in self.tracked.values() {
            let process = member.identity.clone();
            let attempt = match revalidate_member(&process, &current, &descendants) {
                MemberRevalidation::Gone => SignalAttempt::process_gone(process, signal),
                MemberRevalidation::PidReused => SignalAttempt::pid_reused(process, signal),
                MemberRevalidation::AncestryChanged => {
                    return Err(io::Error::other(format!(
                        "pid {} ancestry changed before pidfd signal",
                        process.pid()
                    )));
                }
                MemberRevalidation::Current => {
                    let native_signal = match signal {
                        ContainmentSignal::Term => libc::SIGTERM,
                        ContainmentSignal::Kill => libc::SIGKILL,
                    };
                    match member.pidfd.send_signal(native_signal) {
                        Ok(()) => SignalAttempt::succeeded(process, signal),
                        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                            SignalAttempt::process_gone(process, signal)
                        }
                        Err(error) => SignalAttempt::failed(process, signal, error.to_string()),
                    }
                }
            };
            if attempts.len() < crate::MAX_SIGNAL_ATTEMPTS {
                attempts.push(attempt);
            } else {
                omitted = omitted.saturating_add(1);
            }
        }
        Ok(SignalBatch::new(attempts, omitted))
    }

    fn automatic_signal_all(&mut self, signal: ContainmentSignal) -> io::Result<()> {
        let batch = self.signal_all(signal)?;
        let (attempted, attempts, omitted) = match signal {
            ContainmentSignal::Term => (
                &mut self.automatic_term_attempted,
                &mut self.automatic_term_attempts,
                &mut self.automatic_omitted_term_attempts,
            ),
            ContainmentSignal::Kill => (
                &mut self.automatic_kill_attempted,
                &mut self.automatic_kill_attempts,
                &mut self.automatic_omitted_kill_attempts,
            ),
        };
        *attempted = true;
        *omitted = omitted.saturating_add(batch.omitted());
        let remaining = crate::MAX_SIGNAL_ATTEMPTS.saturating_sub(attempts.len());
        attempts.extend(batch.attempts().iter().take(remaining).cloned());
        *omitted = omitted.saturating_add(batch.attempts().len().saturating_sub(remaining));
        Ok(())
    }

    fn prove_empty(&mut self) -> io::Result<bool> {
        let direct_child_before = self.reap_eligible_children()?;
        if !self.discover_validated()?.is_empty() || direct_child_before {
            return Ok(false);
        }
        if self.reap_eligible_children()? {
            return Ok(false);
        }
        Ok(self.discover_validated()?.is_empty())
    }
}

#[derive(Clone, Copy)]
enum PayloadStatus {
    Exited(i32),
    Signaled(i32),
}

impl PayloadStatus {
    fn from_wait_status(status: i32) -> Self {
        if libc::WIFEXITED(status) {
            Self::Exited(libc::WEXITSTATUS(status))
        } else if libc::WIFSIGNALED(status) {
            Self::Signaled(libc::WTERMSIG(status))
        } else {
            Self::Exited(HELPER_SETUP_FAILURE.into())
        }
    }

    fn protocol_value(self) -> i32 {
        match self {
            Self::Exited(code) => code,
            Self::Signaled(signal) => -signal,
        }
    }
}

fn mirror_payload_status(status: PayloadStatus) -> ExitCode {
    match status {
        PayloadStatus::Exited(code) => {
            ExitCode::from(u8::try_from(code).unwrap_or(HELPER_SETUP_FAILURE))
        }
        PayloadStatus::Signaled(signal) => {
            // SAFETY: helper cleanup is complete. Restoring the default action
            // and raising the payload's terminating signal mirrors Unix wait
            // status for the owner; the fallback return handles blocked signals.
            unsafe {
                libc::signal(signal, libc::SIG_DFL);
                libc::raise(signal);
            }
            ExitCode::from(u8::try_from(128_i32.saturating_add(signal)).unwrap_or(u8::MAX))
        }
    }
}

enum CommandPoll {
    Timeout,
    OwnerLost,
    Command(u8),
}

fn poll_command(fd: RawFd, timeout: Duration) -> io::Result<CommandPoll> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    let timeout_ms = i32::try_from(timeout.as_millis())
        .unwrap_or(i32::MAX)
        .max(1);
    // SAFETY: descriptor points to one initialized pollfd for the call.
    let result = unsafe { libc::poll(&raw mut descriptor, 1, timeout_ms) };
    if result == 0 {
        return Ok(CommandPoll::Timeout);
    }
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            return Ok(CommandPoll::Timeout);
        }
        return Err(error);
    }
    let mut command = [0_u8; 1];
    // SAFETY: command is writable one-byte storage and fd is the live control
    // socket. A zero read is the owner-loss signal.
    let read = unsafe { libc::read(fd, command.as_mut_ptr().cast(), 1) };
    if read == 1 {
        Ok(CommandPoll::Command(command[0]))
    } else if read == 0 {
        Ok(CommandPoll::OwnerLost)
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINTR) {
            Ok(CommandPoll::Timeout)
        } else {
            Err(error)
        }
    }
}

fn become_subreaper() -> io::Result<()> {
    // SAFETY: PR_SET_CHILD_SUBREAPER takes an integer boolean and no pointers.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn set_close_on_exec(fd: RawFd, close_on_exec: bool) -> io::Result<()> {
    // SAFETY: fcntl operates on the valid inherited socket fd.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let next = if close_on_exec {
        flags | libc::FD_CLOEXEC
    } else {
        flags & !libc::FD_CLOEXEC
    };
    // SAFETY: F_SETFD consumes the integer flag value above.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, next) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn close_payload_stdio_copies() {
    for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: helper mode no longer uses payload stdio after the payload has
        // inherited it. Errors are harmless for descriptors already closed.
        let _ = unsafe { libc::close(fd) };
    }
}
