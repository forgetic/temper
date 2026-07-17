//! Linux process-identity helpers shared by the descendant-containment fixture
//! and its aggregate acceptance driver.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

/// A PID plus the procfs start tick that makes the identity stable across reuse.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RecordedProcessIdentity {
    pub role: String,
    pub pid: u32,
    pub start_time: u64,
    pub ppid: u32,
    pub process_group: u32,
    pub session: u32,
    pub executable: PathBuf,
}

/// Reads one exact process identity from Linux procfs.
pub fn process_identity(pid: u32, role: impl Into<String>) -> io::Result<RecordedProcessIdentity> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat
        .rfind(") ")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed /proc stat"))?;
    let fields = stat[close + 2..].split_whitespace().collect::<Vec<_>>();
    if fields.len() <= 19 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "short /proc stat",
        ));
    }
    let parse = |index: usize, name: &str| -> io::Result<u64> {
        fields[index].parse().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid {name} in /proc stat: {error}"),
            )
        })
    };
    let as_u32 = |value: u64, name: &str| {
        u32::try_from(value)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("{name} exceeds u32")))
    };
    Ok(RecordedProcessIdentity {
        role: role.into(),
        pid,
        ppid: as_u32(parse(1, "ppid")?, "ppid")?,
        process_group: as_u32(parse(2, "process group")?, "process group")?,
        session: as_u32(parse(3, "session")?, "session")?,
        start_time: parse(19, "start time")?,
        executable: fs::read_link(format!("/proc/{pid}/exe"))
            .unwrap_or_else(|_| PathBuf::from(format!("[pid:{pid}]"))),
    })
}

/// Appends one record with a single write, allowing concurrent fixture members
/// to publish to the same identity log without replacing earlier identities.
pub fn append_current_identity(path: &Path, role: &str) -> io::Result<RecordedProcessIdentity> {
    let identity = process_identity(std::process::id(), role)?;
    let executable = identity.executable.to_string_lossy().replace('\t', " ");
    let line = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        identity.role,
        identity.pid,
        identity.start_time,
        identity.ppid,
        identity.process_group,
        identity.session,
        executable,
    );
    let mut output = OpenOptions::new().create(true).append(true).open(path)?;
    output.write_all(line.as_bytes())?;
    output.flush()?;
    Ok(identity)
}

/// Reads every complete identity record currently published by the fixture.
pub fn read_recorded_identities(path: &Path) -> io::Result<Vec<RecordedProcessIdentity>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_record)
        .collect()
}

fn parse_record(line: &str) -> io::Result<RecordedProcessIdentity> {
    let mut fields = line.splitn(7, '\t');
    let missing = || io::Error::new(io::ErrorKind::InvalidData, "short identity record");
    let role = fields.next().ok_or_else(missing)?.to_string();
    let parse = |value: Option<&str>, name: &str| -> io::Result<u64> {
        value
            .ok_or_else(missing)?
            .parse()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {error}")))
    };
    Ok(RecordedProcessIdentity {
        role,
        pid: u32::try_from(parse(fields.next(), "pid")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "pid exceeds u32"))?,
        start_time: parse(fields.next(), "start time")?,
        ppid: u32::try_from(parse(fields.next(), "ppid")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "ppid exceeds u32"))?,
        process_group: u32::try_from(parse(fields.next(), "process group")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "process group exceeds u32"))?,
        session: u32::try_from(parse(fields.next(), "session")?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "session exceeds u32"))?,
        executable: PathBuf::from(fields.next().ok_or_else(missing)?),
    })
}

/// Returns the current procfs identity only when the PID still denotes the
/// recorded start tick. A reused PID is absence, never a survivor match.
pub fn current_exact_identity(
    recorded: &RecordedProcessIdentity,
) -> io::Result<Option<RecordedProcessIdentity>> {
    match process_identity(recorded.pid, recorded.role.clone()) {
        Ok(current) if current.start_time == recorded.start_time => Ok(Some(current)),
        Ok(_) => Ok(None),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(3) =>
        {
            // procfs can report ESRCH while a task is disappearing even though
            // the path lookup itself succeeded. For an exact PID/start pair,
            // both ENOENT and ESRCH are authoritative absence.
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Deduplicates records by their PID/start-time identity while retaining roles.
pub fn unique_identities(
    records: impl IntoIterator<Item = RecordedProcessIdentity>,
) -> Vec<RecordedProcessIdentity> {
    let mut unique = BTreeMap::new();
    for record in records {
        unique.insert((record.pid, record.start_time), record);
    }
    unique.into_values().collect()
}
