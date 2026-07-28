use super::*;

pub(super) fn wait_for_identities(
    path: &Path,
    expected: usize,
    timeout: Duration,
) -> Result<Vec<RecordedProcessIdentity>, String> {
    let deadline = Instant::now() + timeout;
    loop {
        let identities = read_identities(path)?;
        if identities.len() >= expected {
            assert_identity_generations(&identities, expected / 3)?;
            return Ok(identities);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for {expected} compiled fixture identities in {}; found {identities:?}",
                path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn read_identities(path: &Path) -> Result<Vec<RecordedProcessIdentity>, String> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "read fixture identities {}: {error}",
                path.display()
            ));
        }
    };
    let identities = source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = line.splitn(7, '\t').collect::<Vec<_>>();
            let [role, pid, start, parent, group, session, executable] = fields.as_slice() else {
                return Err(format!("malformed fixture identity line: {line:?}"));
            };
            let number = |raw: &str, name: &str| {
                raw.parse::<u64>()
                    .map_err(|error| format!("invalid identity {name} {raw:?}: {error}"))
            };
            Ok(RecordedProcessIdentity {
                role: (*role).to_string(),
                pid: number(pid, "pid")?
                    .try_into()
                    .map_err(|_| "identity pid exceeds u32".to_string())?,
                start_time: number(start, "start_time")?,
                parent_pid: number(parent, "parent_pid")?
                    .try_into()
                    .map_err(|_| "identity parent_pid exceeds u32".to_string())?,
                process_group: number(group, "process_group")?
                    .try_into()
                    .map_err(|_| "identity process_group exceeds u32".to_string())?,
                session: number(session, "session")?
                    .try_into()
                    .map_err(|_| "identity session exceeds u32".to_string())?,
                executable: (*executable).to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut exact = std::collections::BTreeSet::new();
    for identity in &identities {
        if !exact.insert((identity.pid, identity.start_time)) {
            return Err(format!(
                "fixture recorded one PID/start identity more than once: {identity:?}"
            ));
        }
    }
    Ok(identities)
}

fn assert_identity_generations(
    identities: &[RecordedProcessIdentity],
    expected_generations: usize,
) -> Result<(), String> {
    const ROLES: [&str; 3] = [
        "standalone-mcp-supervisor",
        "standalone-mcp-root",
        "standalone-mcp-descendant",
    ];
    if identities.len() != expected_generations.saturating_mul(ROLES.len()) {
        return Err(format!(
            "compiled fixture recorded an unexpected number of process boundaries: {identities:?}"
        ));
    }
    for role in ROLES {
        let count = identities
            .iter()
            .filter(|identity| identity.role == role)
            .count();
        if count != expected_generations {
            return Err(format!(
                "compiled fixture recorded {count} `{role}` identities, expected {expected_generations}: {identities:?}"
            ));
        }
    }
    Ok(())
}

pub(super) struct ExactProcessCleanup {
    identities: Vec<RecordedProcessIdentity>,
    armed: bool,
}

impl ExactProcessCleanup {
    pub(super) fn new(identities: Vec<RecordedProcessIdentity>) -> Self {
        Self {
            identities,
            armed: true,
        }
    }

    pub(super) fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ExactProcessCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Failure-only hygiene: release a deliberately stopped supervisor,
        // then terminate only still-matching PID/start identities. The success
        // path disarms this guard after proving Temper removed every process;
        // it therefore never supplies the KILL asserted absent by the test.
        for signal in ["CONT", "KILL"] {
            for identity in &self.identities {
                if process_identity_is_live(identity) {
                    let _ = std::process::Command::new("kill")
                        .arg(format!("-{signal}"))
                        .arg(identity.pid.to_string())
                        .status();
                }
            }
        }
    }
}

pub(super) fn wait_for_processes_gone(
    identities: &[RecordedProcessIdentity],
    timeout: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let live = identities
            .iter()
            .filter(|identity| process_identity_is_live(identity))
            .collect::<Vec<_>>();
        if live.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Temper-owned emergency escalation left exact PID/start identities alive: {live:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn process_identity_is_live(identity: &RecordedProcessIdentity) -> bool {
    process_start_time(identity.pid).is_some_and(|start| start == identity.start_time)
}

fn process_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let close = stat.rfind(") ")?;
    stat[close + 2..].split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_identity_parser_preserves_pid_start_pairs() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("identities");
        fs::write(
            &path,
            "standalone-mcp-root\t41\t9001\t40\t41\t41\t/usr/bin/fixture\n",
        )
        .unwrap();
        let identities = read_identities(&path).unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].pid, 41);
        assert_eq!(identities[0].start_time, 9001);
        assert_eq!(identities[0].parent_pid, 40);
    }
}
