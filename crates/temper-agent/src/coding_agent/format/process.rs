//! Bounded stdin/stdout invocation; rustfmt receives no source filename arguments.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use temper_agent_core::{
    AgentContainmentContext, CleanupTrigger, ContainedProcess, ContainmentCommand, ContainmentScope,
};

use crate::workspace_format::{MAX_FORMAT_BYTES, RustEdition};

pub(super) fn format_source(
    directory: &Path,
    source: &[u8],
    edition: RustEdition,
    deadline: Instant,
    containment: &AgentContainmentContext,
) -> Result<Vec<u8>, String> {
    let mut command = ContainmentCommand::new("rustfmt");
    command.current_dir(directory).args([
        "--edition",
        edition.as_str(),
        "--emit",
        "stdout",
        "--config",
        "skip_children=true",
    ]);
    let output = run_bounded(command, source, deadline, MAX_FORMAT_BYTES * 2, containment)?;
    std::str::from_utf8(&output)
        .map_err(|error| format!("formatter output is not UTF-8: {error}"))?;
    if output.is_empty() && !source.is_empty() {
        return Err("formatter returned empty output for a nonempty file".into());
    }
    Ok(output)
}

fn run_bounded(
    mut command: ContainmentCommand,
    source: &[u8],
    deadline: Instant,
    output_limit: usize,
    containment: &AgentContainmentContext,
) -> Result<Vec<u8>, String> {
    if Instant::now() >= deadline {
        return Err("formatter exceeded 30 second time limit".into());
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let spec = containment
        .containment_spec("format_rust", ContainmentScope::Tool)
        .with_timing(Duration::ZERO, Duration::from_millis(10));
    let child = containment
        .factory()
        .prepare(spec)
        .and_then(|prepared| prepared.spawn(command))
        .map_err(|error| format!("cannot start rustfmt: {error}"))?;
    let mut stdin = child
        .take_stdin()
        .map_err(|error| error.to_string())?
        .ok_or("missing formatter stdin")?;
    let stdout = child
        .take_stdout()
        .map_err(|error| error.to_string())?
        .ok_or("missing formatter stdout")?;
    let stderr = child
        .take_stderr()
        .map_err(|error| error.to_string())?
        .ok_or("missing formatter stderr")?;
    let overflow = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(source));
        let out = scope.spawn(|| read_output(stdout, output_limit, &overflow));
        let err = scope.spawn(|| read_output(stderr, 64 * 1024, &overflow));
        let status = wait_for_formatter(&child, deadline, &overflow);
        let written = writer.join().map_err(|_| "formatter input thread failed")?;
        let output = out.join().map_err(|_| "formatter output thread failed")??;
        let errors = err
            .join()
            .map_err(|_| "formatter diagnostic thread failed")??;
        let status = status?;
        if overflow.load(Ordering::Acquire) {
            return Err("formatter exceeded output limit".into());
        }
        if !status.success() {
            return Err(format!(
                "rustfmt rejected input: {}",
                String::from_utf8_lossy(&errors).trim()
            ));
        }
        written.map_err(|error| format!("cannot send source to rustfmt: {error}"))?;
        Ok(output)
    })
}

fn wait_for_formatter(
    child: &ContainedProcess,
    deadline: Instant,
    overflow: &AtomicBool,
) -> Result<std::process::ExitStatus, String> {
    loop {
        if overflow.load(Ordering::Acquire) || Instant::now() >= deadline {
            child.cleanup(CleanupTrigger::Timeout);
            return Err("formatter exceeded time or output limit".into());
        }
        match child.try_wait_root() {
            Ok(Some(status)) => {
                // A wrapper's exit does not imply EOF: descendants can still
                // own the inherited pipes. Prove recursive emptiness before joins.
                child.cleanup(CleanupTrigger::NormalRootExit);
                return Ok(status);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                child.cleanup(CleanupTrigger::Shutdown);
                return Err(format!("cannot wait for rustfmt: {error}"));
            }
        }
    }
}

fn read_output(stream: impl Read, limit: usize, overflow: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    stream
        .take((limit + 1) as u64)
        .read_to_end(&mut output)
        .map_err(|error| format!("cannot read formatter output: {error}"))?;
    if output.len() > limit {
        overflow.store(true, Ordering::Release);
    }
    Ok(output)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn bounds_output_and_terminates_a_stalled_process() {
        let containment = AgentContainmentContext::production(None);
        let mut flood = ContainmentCommand::new("sh");
        flood.args(["-c", "while :; do printf '0123456789'; done"]);
        assert!(
            run_bounded(
                flood,
                b"",
                Instant::now() + Duration::from_secs(2),
                128,
                &containment
            )
            .unwrap_err()
            .contains("limit")
        );
        let mut stalled = ContainmentCommand::new("sh");
        stalled.args(["-c", "while :; do :; done"]);
        let started = Instant::now();
        assert!(
            run_bounded(
                stalled,
                b"",
                started + Duration::from_millis(50),
                128,
                &containment
            )
            .unwrap_err()
            .contains("limit")
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn deadline_cleans_up_descendants_holding_inherited_pipes() {
        let containment = AgentContainmentContext::production(None);
        let mut wrapper = ContainmentCommand::new("sh");
        wrapper.args(["-c", "sleep 10 & wait"]);
        let started = Instant::now();
        assert!(
            run_bounded(
                wrapper,
                b"",
                started + Duration::from_millis(100),
                128,
                &containment
            )
            .unwrap_err()
            .contains("limit")
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn root_exit_also_cleans_up_inherited_pipes() {
        let containment = AgentContainmentContext::production(None);
        let mut wrapper = ContainmentCommand::new("sh");
        wrapper.args(["-c", "sleep 10 & exit 0"]);
        let started = Instant::now();
        run_bounded(
            wrapper,
            b"",
            started + Duration::from_secs(2),
            128,
            &containment,
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
