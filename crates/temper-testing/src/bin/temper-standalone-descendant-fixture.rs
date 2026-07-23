//! Compiled Linux MCP/descendant fixture for standalone shutdown acceptance.
//!
//! The MCP server records exact PID/start identities, launches a descendant in
//! a fresh session that ignores TERM, and then serves the minimum initialize +
//! tools/list protocol needed by the native coding agent. On an explicit file
//! trigger the descendant stops its Temper-owned Linux supervisor during
//! ordinary recursive-empty proof. The acceptance harness later sends only
//! SIGCONT; Temper's already-queued emergency KILL remains responsible for
//! terminating and reaping the payload tree.
//!
//! There is deliberately no Cargo invocation or ambient backend selector.

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{self, OpenOptions};
    use std::io::{self, BufRead, Write as _};
    use std::path::Path;
    use std::process::{Command, ExitCode, Stdio};
    use std::time::Duration;

    pub fn main() -> io::Result<ExitCode> {
        let mut arguments = std::env::args_os().skip(1);
        let mode = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| invalid("fixture mode is required"))?;
        let arguments = arguments.collect::<Vec<_>>();
        match mode.as_str() {
            "mcp" => run_mcp(&arguments)?,
            "child" => run_child(&arguments)?,
            other => return Err(invalid(&format!("unknown fixture mode {other}"))),
        }
        Ok(ExitCode::SUCCESS)
    }

    fn run_mcp(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [identities, ready, obstruction_trigger, obstruction_ready] = arguments else {
            return Err(invalid(
                "mcp requires identities, ready, obstruction trigger, and obstruction ready paths",
            ));
        };
        let helper_pid = parent_pid(std::process::id())?;
        append_pid_identity(
            Path::new(identities),
            "standalone-mcp-supervisor",
            helper_pid,
        )?;
        append_pid_identity(
            Path::new(identities),
            "standalone-mcp-root",
            std::process::id(),
        )?;

        let executable = std::env::current_exe()?;
        // `SIG_IGN` survives exec. The compiled child therefore ignores TERM
        // without depending on signal crates, while still running the identity
        // and obstruction code in this fixture binary.
        Command::new("setsid")
            .arg("sh")
            .arg("-c")
            .arg("trap '' TERM; exec \"$1\" child \"$2\" \"$3\" \"$4\" \"$5\" \"$6\"")
            .arg("temper-standalone-descendant-launcher")
            .arg(executable)
            .arg(identities)
            .arg(ready)
            .arg(helper_pid.to_string())
            .arg(obstruction_trigger)
            .arg(obstruction_ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        wait_for_path(Path::new(ready), Duration::from_secs(5))?;

        let stdin = io::stdin();
        let mut stdout = io::stdout().lock();
        for line in stdin.lock().lines() {
            let line = line?;
            let Some(id) = json_rpc_id(&line) else {
                continue;
            };
            if line.contains("\"method\":\"initialize\"")
                || line.contains("\"method\": \"initialize\"")
            {
                writeln!(
                    stdout,
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"protocolVersion\":\"2024-11-05\",\"serverInfo\":{{\"name\":\"temper-standalone-shutdown-fixture\",\"version\":\"1\"}},\"capabilities\":{{\"tools\":{{}}}}}}}}"
                )?;
            } else if line.contains("tools/list") {
                // Keep one safe tool registered so the client (and therefore
                // this process owner) remains live while the model is blocked.
                writeln!(
                    stdout,
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"tools\":[{{\"name\":\"search_code\",\"description\":\"standalone shutdown fixture\",\"inputSchema\":{{\"type\":\"object\",\"properties\":{{\"query\":{{\"type\":\"string\"}}}}}}}}]}}}}"
                )?;
            } else if line.contains("tools/call") {
                writeln!(
                    stdout,
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"fixture result\"}}],\"isError\":false}}}}"
                )?;
            } else {
                writeln!(
                    stdout,
                    "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32601,\"message\":\"unsupported fixture method\"}}}}"
                )?;
            }
            stdout.flush()?;
        }
        Ok(())
    }

    fn run_child(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [
            identities,
            ready,
            helper_pid,
            obstruction_trigger,
            obstruction_ready,
        ] = arguments
        else {
            return Err(invalid(
                "child requires identities, ready, helper PID, obstruction trigger, and obstruction ready paths",
            ));
        };
        let helper_pid = helper_pid
            .to_str()
            .and_then(|raw| raw.parse::<u32>().ok())
            .ok_or_else(|| invalid("child helper PID is invalid"))?;
        append_pid_identity(
            Path::new(identities),
            "standalone-mcp-descendant",
            std::process::id(),
        )?;
        fs::write(ready, b"ready\n")?;

        let mut obstructed = false;
        loop {
            if !obstructed && Path::new(obstruction_trigger).exists() {
                let status = Command::new("kill")
                    .arg("-STOP")
                    .arg(helper_pid.to_string())
                    .status()?;
                if !status.success() {
                    return Err(io::Error::other(format!(
                        "could not stop Temper supervisor {helper_pid}: {status}"
                    )));
                }
                // The descendant remains runnable after stopping its parent
                // supervisor, so this marker is a deterministic synchronization
                // point for the acceptance harness.
                fs::write(obstruction_ready, format!("{helper_pid}\n"))?;
                obstructed = true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn append_pid_identity(path: &Path, role: &str, pid: u32) -> io::Result<()> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat
            .rfind(") ")
            .ok_or_else(|| invalid("malformed /proc stat"))?;
        let fields = stat[close + 2..].split_whitespace().collect::<Vec<_>>();
        if fields.len() <= 19 {
            return Err(invalid("short /proc stat"));
        }
        let ppid = fields[1];
        let process_group = fields[2];
        let session = fields[3];
        let start_time = fields[19];
        let executable = fs::read_link(format!("/proc/{pid}/exe"))
            .unwrap_or_else(|_| format!("[pid:{pid}]").into())
            .to_string_lossy()
            .replace('\t', " ");
        let mut output = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(
            output,
            "{role}\t{pid}\t{start_time}\t{ppid}\t{process_group}\t{session}\t{executable}"
        )?;
        output.flush()
    }

    fn parent_pid(pid: u32) -> io::Result<u32> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat
            .rfind(") ")
            .ok_or_else(|| invalid("malformed /proc stat"))?;
        stat[close + 2..]
            .split_whitespace()
            .nth(1)
            .and_then(|raw| raw.parse().ok())
            .ok_or_else(|| invalid("missing parent PID in /proc stat"))
    }

    fn json_rpc_id(line: &str) -> Option<u64> {
        let (_, suffix) = line.split_once("\"id\"")?;
        let (_, value) = suffix.split_once(':')?;
        value
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> io::Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {}", path.display()),
        ))
    }

    fn invalid(message: &str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }
}

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    match linux::main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("standalone descendant fixture failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
