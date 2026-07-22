use std::io::{self, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use super::super::process::{PidFd, descendants_of, scan_proc};

const NONE: u8 = 0;
const FORCED: u8 = 1;
const HARD_KILL: u8 = 2;

pub(super) struct EmergencyStage(Arc<AtomicU8>);

impl EmergencyStage {
    pub(super) fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire) != NONE
    }

    pub(super) fn hard_kill_requested(&self) -> bool {
        self.0.load(Ordering::Acquire) == HARD_KILL
    }
}

pub(super) fn spawn_owner(
    channel: std::os::unix::net::UnixStream,
    supervisor_pid: u32,
    term_grace: Duration,
    inspection_retry: Duration,
) -> io::Result<EmergencyStage> {
    let stage = Arc::new(AtomicU8::new(NONE));
    let owner_stage = Arc::clone(&stage);
    std::thread::Builder::new()
        .name("temper-linux-supervisor-emergency".to_owned())
        .spawn(move || {
            run_owner(
                channel,
                supervisor_pid,
                owner_stage,
                term_grace,
                inspection_retry,
            )
        })?;
    Ok(EmergencyStage(stage))
}

fn run_owner(
    mut channel: std::os::unix::net::UnixStream,
    supervisor_pid: u32,
    stage: Arc<AtomicU8>,
    term_grace: Duration,
    inspection_retry: Duration,
) {
    let mut command = [0_u8; 1];
    loop {
        match channel.read(&mut command) {
            Ok(1) if command[0] == b'T' => {
                stage.fetch_max(FORCED, Ordering::AcqRel);
                signal_tree(supervisor_pid, libc::SIGTERM, 1);
            }
            Ok(1) if command[0] == b'K' => {
                stage.store(HARD_KILL, Ordering::Release);
                signal_tree(supervisor_pid, libc::SIGKILL, 3);
            }
            Ok(1) => {}
            Ok(0) => break,
            Ok(_) => unreachable!("one-byte emergency command buffer"),
            Err(error) if error.raw_os_error() == Some(libc::EINTR) => {}
            Err(_) => break,
        }
    }

    // Losing the independent owner is itself a forced-then-hard request. Give
    // payloads the configured TERM grace, then keep rescanning and signaling
    // while the main helper proves emptiness and reaps; never return and
    // abandon a late-forked payload.
    stage.fetch_max(FORCED, Ordering::AcqRel);
    signal_tree(supervisor_pid, libc::SIGTERM, 1);
    std::thread::sleep(term_grace);
    stage.store(HARD_KILL, Ordering::Release);
    loop {
        signal_tree(supervisor_pid, libc::SIGKILL, 3);
        std::thread::sleep(inspection_retry.max(Duration::from_millis(1)));
    }
}

fn signal_tree(supervisor_pid: u32, signal: i32, passes: usize) {
    for pass in 0..passes.max(1) {
        if let Ok(first) = scan_proc() {
            let descendants = descendants_of(supervisor_pid, &first);
            let mut pinned = Vec::with_capacity(descendants.len());
            for pid in descendants {
                if let Ok(pidfd) = PidFd::open(pid) {
                    let start_time = first.get(&pid).map(|process| process.start_time);
                    pinned.push((pid, start_time, pidfd));
                }
            }
            if let Ok(current) = scan_proc() {
                let current_descendants = descendants_of(supervisor_pid, &current);
                for (pid, expected_start, pidfd) in pinned {
                    if current_descendants.contains(&pid)
                        && current.get(&pid).map(|process| process.start_time) == expected_start
                    {
                        let _ = pidfd.send_signal(signal);
                    }
                }
            }
        }
        if pass + 1 < passes.max(1) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
