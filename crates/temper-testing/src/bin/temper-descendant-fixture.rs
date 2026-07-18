#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode, Stdio};
    use std::time::{Duration, Instant};

    use temper_testing::descendant_fixture::{
        append_current_identity, current_exact_identity, read_recorded_identities,
        unique_identities,
    };

    const POLL_INTERVAL: Duration = Duration::from_millis(2);

    pub fn main() -> io::Result<ExitCode> {
        let mut arguments = std::env::args_os().skip(1);
        let mode = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "fixture mode is required")
            })?;
        let arguments = arguments.collect::<Vec<_>>();
        match mode.as_str() {
            "parent" => run_parent(&arguments)?,
            "child" => run_child(&arguments)?,
            "agent" => return run_agent(&arguments),
            "monitor" => run_monitor(&arguments)?,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown fixture mode {other}"),
                ));
            }
        }
        Ok(ExitCode::SUCCESS)
    }

    fn run_parent(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [
            identities,
            ready,
            mutation_trigger,
            late_mutation,
            ignore_term,
        ] = arguments
        else {
            return Err(invalid(
                "parent requires identities, ready, trigger, late, ignore-term",
            ));
        };
        let identity = append_current_identity(Path::new(identities), "rust-test-shaped-parent")?;
        if identity.process_group != identity.pid || identity.session != identity.pid {
            return Err(io::Error::other(format!(
                "fixture parent did not enter a fresh process group and session: {identity:?}"
            )));
        }
        let executable = std::env::current_exe()?;
        let ignore_term = parse_bool(ignore_term)?;
        let mut child = Command::new("setsid");
        child
            .arg("/bin/bash")
            .arg("-c")
            .arg(if ignore_term {
                "trap '' TERM; exec -a temper-agent-shaped \"$1\" child \"$2\" \"$3\" \"$4\" \"$5\""
            } else {
                "exec -a temper-agent-shaped \"$1\" child \"$2\" \"$3\" \"$4\" \"$5\""
            })
            .arg("temper-agent-launcher")
            .arg(executable)
            .arg(identities)
            .arg(ready)
            .arg(mutation_trigger)
            .arg(late_mutation)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        child.spawn()?;
        wait_for_path(Path::new(ready), Duration::from_secs(3))?;
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn run_child(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [identities, ready, mutation_trigger, late_mutation] = arguments else {
            return Err(invalid(
                "child requires identities, ready, trigger, and late paths",
            ));
        };
        let identity = append_current_identity(Path::new(identities), "temper-agent-shaped-child")?;
        if identity.process_group != identity.pid || identity.session != identity.pid {
            return Err(io::Error::other(format!(
                "nested fixture did not create a new process group/session: {identity:?}"
            )));
        }
        fs::write(ready, b"ready\n")?;
        while !Path::new(mutation_trigger).exists() {
            std::thread::sleep(POLL_INTERVAL);
        }
        fs::write(late_mutation, b"late workspace mutation\n")?;
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    fn run_agent(arguments: &[std::ffi::OsString]) -> io::Result<ExitCode> {
        if arguments.len() < 7 {
            return Err(invalid(
                "agent requires identities, ready, trigger, late, ignore-term, exit, and hold",
            ));
        }
        let identities = PathBuf::from(&arguments[0]);
        let ready = PathBuf::from(&arguments[1]);
        let mutation_trigger = PathBuf::from(&arguments[2]);
        let late_mutation = PathBuf::from(&arguments[3]);
        let ignore_term = arguments[4].clone();
        let exit_code = parse_i32(&arguments[5], "agent exit code")?;
        let hold = parse_bool(&arguments[6])?;
        append_current_identity(&identities, "out-of-process-agent-root")?;

        let executable = std::env::current_exe()?;
        Command::new("setsid")
            .arg(executable)
            .arg("parent")
            .arg(&identities)
            .arg(&ready)
            .arg(&mutation_trigger)
            .arg(&late_mutation)
            .arg(ignore_term)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        wait_for_path(&ready, Duration::from_secs(3))?;

        let result_path = flag_value(&arguments[7..], "--result")
            .ok_or_else(|| invalid("worker did not supply --result to fixture agent"))?;
        fs::write(
            result_path,
            br#"{"title":"fixture","body":"contained fixture","summary":"fixture completed"}"#,
        )?;
        if hold {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        Ok(u8::try_from(exit_code).map_or(ExitCode::FAILURE, ExitCode::from))
    }

    fn run_monitor(arguments: &[std::ffi::OsString]) -> io::Result<()> {
        let [identities_path, report_path, stop_path] = arguments else {
            return Err(invalid(
                "monitor requires identities, report, and stop paths",
            ));
        };
        let identities_path = Path::new(identities_path);
        let stop_path = Path::new(stop_path);
        let mut seen = BTreeMap::new();
        let mut orphaned = BTreeSet::new();
        let mut reused = BTreeSet::new();
        let started = Instant::now();

        loop {
            for identity in unique_identities(read_recorded_identities(identities_path)?) {
                let key = (identity.pid, identity.start_time);
                seen.entry(key).or_insert(identity);
            }
            for (&key, identity) in &seen {
                match current_exact_identity(identity)? {
                    Some(current) if current.ppid == 1 => {
                        orphaned.insert(key);
                    }
                    Some(_) => {}
                    None => {
                        if let Ok(current) = temper_testing::descendant_fixture::process_identity(
                            identity.pid,
                            identity.role.clone(),
                        ) {
                            if current.start_time != identity.start_time {
                                reused.insert(key);
                            }
                        }
                    }
                }
            }
            if stop_path.exists() {
                // One quiet interval catches a record published concurrently
                // with the completion signal.
                std::thread::sleep(POLL_INTERVAL);
                for identity in unique_identities(read_recorded_identities(identities_path)?) {
                    seen.entry((identity.pid, identity.start_time))
                        .or_insert(identity);
                }
                break;
            }
            if started.elapsed() > Duration::from_secs(20) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "monitor did not receive its stop signal",
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        let mut alive = Vec::new();
        for identity in seen.values() {
            if current_exact_identity(identity)?.is_some() {
                alive.push(format!(
                    "{}:{}:{}",
                    identity.role, identity.pid, identity.start_time
                ));
            }
        }
        let report = format!(
            "identity_count={}\norphaned={}\nalive={}\nreused={}\nalive_identities={}\n",
            seen.len(),
            orphaned.len(),
            alive.len(),
            reused.len(),
            alive.join(","),
        );
        fs::write(report_path, report)
    }

    fn flag_value(arguments: &[std::ffi::OsString], name: &str) -> Option<PathBuf> {
        arguments
            .windows(2)
            .find_map(|pair| (pair[0] == name).then(|| PathBuf::from(pair[1].clone())))
    }

    fn wait_for_path(path: &Path, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.exists() {
                return Ok(());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("timed out waiting for {}", path.display()),
        ))
    }

    fn parse_bool(value: &std::ffi::OsStr) -> io::Result<bool> {
        match value.to_str() {
            Some("true") => Ok(true),
            Some("false") => Ok(false),
            _ => Err(invalid("expected true or false")),
        }
    }

    fn parse_i32(value: &std::ffi::OsStr, name: &str) -> io::Result<i32> {
        value
            .to_str()
            .ok_or_else(|| invalid(name))?
            .parse()
            .map_err(|error| invalid(&format!("invalid {name}: {error}")))
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
            eprintln!("descendant fixture failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
