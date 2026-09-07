//! Exact private process identities and bounded retained-log validation.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(super) struct Identity {
    pub pid: u32,
    pub start: u64,
}

impl Identity {
    pub(super) fn live(&self) -> bool {
        fs::read_to_string(format!("/proc/{}/stat", self.pid))
            .ok()
            .and_then(|stat| stat.rsplit_once(") ").map(|(_, fields)| fields.to_string()))
            .and_then(|fields| fields.split_whitespace().nth(19)?.parse::<u64>().ok())
            == Some(self.start)
    }
}

pub(super) fn rows(path: &Path) -> Result<Vec<Value>, String> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("read lifecycle evidence: {error}")),
    };
    if raw.len() > 16 * 1024 * 1024 {
        return Err("lifecycle evidence exceeds private bound".into());
    }
    let complete = raw
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(&raw[..0], |end| &raw[..=end]);
    complete
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice(line)
                .map_err(|error| format!("malformed lifecycle evidence: {error}"))
        })
        .collect()
}

pub(super) fn event<'a>(rows: &'a [Value], name: &str) -> Vec<&'a Value> {
    rows.iter().filter(|row| row["event"] == name).collect()
}

fn one_identity(rows: &[Value], name: &str) -> Result<Identity, String> {
    let values = event(rows, name);
    if values.len() != 1 {
        return Err(format!("expected one {name}, got {}", values.len()));
    }
    serde_json::from_value(values[0]["identity"].clone())
        .map_err(|_| "invalid lifecycle identity".into())
}

pub(super) struct Cohort {
    pub daemon: Identity,
    pub work: Identity,
    pub owner: Identity,
    pub a_frontend: Identity,
    pub a_process_owner: Identity,
    pub a_cwd: String,
}

impl Cohort {
    pub(super) fn capture(rows: &[Value]) -> Result<Self, String> {
        let daemon = one_identity(rows, "daemon_started")?;
        let work = one_identity(rows, "work_started")?;
        let analysis = event(rows, "attached")
            .into_iter()
            .next()
            .ok_or("cold bootstrap did not attach")?;
        if analysis["profile"] != "analysis"
            || event(rows, "daemon_started")[0]["parent"] != analysis["identity"]
        {
            return Err(
                "daemon was not cold-started by the first parent bootstrap frontend".into(),
            );
        }
        let owner = analysis["owners"]
            .as_array()
            .and_then(|owners| owners.first())
            .ok_or("analysis frontend has no production bootstrap owner")?;
        let owner = serde_json::from_value(owner.clone())
            .map_err(|_| "invalid bootstrap owner identity")?;
        let mut serving = live_frontends(rows)?;
        if serving.len() != 1 {
            return Err("A serving handoff has not reached one live frontend".into());
        }
        let (a_frontend, a_cwd) = serving.remove(0);
        let a_process_owner = event(rows, "attached")
            .into_iter()
            .find(|row| row["identity"]["pid"] == a_frontend.pid)
            .and_then(|row| serde_json::from_value(row["parent"].clone()).ok())
            .ok_or("A frontend process owner is missing")?;
        let cohort = Self {
            daemon,
            work,
            owner,
            a_frontend,
            a_process_owner,
            a_cwd,
        };
        cohort.assert_live()?;
        Ok(cohort)
    }

    pub(super) fn assert_live(&self) -> Result<(), String> {
        if [&self.daemon, &self.work, &self.owner]
            .iter()
            .all(|identity| identity.live())
        {
            Ok(())
        } else {
            Err("shared daemon/work/bootstrap owner changed or exited".into())
        }
    }

    pub(super) fn survivor(&self, rows: &[Value]) -> Result<(Identity, String), String> {
        if one_identity(rows, "daemon_started")? != self.daemon
            || one_identity(rows, "work_started")? != self.work
        {
            return Err("shared daemon or work restarted".into());
        }
        self.assert_live()?;
        let mut candidates = live_frontends(rows)?
            .into_iter()
            .filter(|(id, cwd)| id != &self.a_frontend && cwd != &self.a_cwd)
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err("B handoff has not reached one distinct serving frontend".into());
        }
        let (frontend, cwd) = candidates.remove(0);
        let root = prepared_root(rows, &cwd)?;
        Ok((frontend, root))
    }
}

fn prepared_root(rows: &[Value], cwd: &str) -> Result<String, String> {
    let sessions = event(rows, "attached")
        .into_iter()
        .filter(|row| row["cwd"] == cwd)
        .filter_map(|row| row["session"].as_str())
        .collect::<Vec<_>>();
    let roots = event(rows, "indexed")
        .into_iter()
        .filter(|row| {
            row["session"]
                .as_str()
                .is_some_and(|session| sessions.contains(&session))
        })
        .filter_map(|row| row["root"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    if roots.len() != 1 {
        return Err("B startup did not bind exactly one prepared repo root".into());
    }
    let root = roots.into_iter().next().expect("one prepared root");
    if !Path::new(root).is_absolute() || Path::new(root) != Path::new(cwd) {
        return Err("B index root is outside its prepared workspace".into());
    }
    Ok(root.to_string())
}

fn live_frontends(rows: &[Value]) -> Result<Vec<(Identity, String)>, String> {
    event(rows, "attached")
        .into_iter()
        .filter(|row| row["profile"] == "serving")
        .map(|row| {
            let id: Identity = serde_json::from_value(row["identity"].clone())
                .map_err(|_| "invalid frontend identity")?;
            let cwd = row["cwd"]
                .as_str()
                .ok_or("missing frontend checkout")?
                .to_string();
            Ok((id, cwd))
        })
        .collect::<Result<Vec<_>, String>>()
        .map(|all| all.into_iter().filter(|(id, _)| id.live()).collect())
}

pub(super) fn validate_continuity(rows: &[Value]) -> Result<(), String> {
    one_identity(rows, "daemon_started")?;
    one_identity(rows, "work_started")?;
    let end = rows
        .iter()
        .position(|row| row["event"] == "last_session_closed")
        .unwrap_or(rows.len());
    if rows[..end]
        .iter()
        .any(|row| row["event"] == "detached" && row["active"] == 0)
        && end == rows.len()
    {
        return Err("provider lost all admitted sessions before final detach".into());
    }
    if rows[..end.saturating_sub(1)]
        .iter()
        .any(|row| row["event"] == "detached" && row["active"] == 0)
    {
        return Err("provider handoff contained a zero-session gap".into());
    }
    if rows.iter().any(|row| {
        matches!(
            row["event"].as_str(),
            Some("protocol_failure" | "fixture_deadline_expired")
        )
    }) {
        return Err("provider fixture failed or reached its safety deadline".into());
    }
    Ok(())
}

pub(super) fn cancellation_complete(
    rows: &[Value],
    job: &str,
    attempt: &str,
    frontend: &Identity,
    process_owner: &Identity,
) -> bool {
    let matches =
        |row: &&Value| row["fields"]["job_id"] == job && row["fields"]["attempt_id"] == attempt;
    let owned = rows.iter().filter(matches).collect::<Vec<_>>();
    let Some(requested) = owned.iter().position(|row| {
        row["fields"]["event"] == "worker.job.cancellation_requested"
            && row["fields"]["timeout_reason"] == "ownership_lost"
    }) else {
        return false;
    };
    let Some(cleaned) = owned
        .iter()
        .enumerate()
        .skip(requested + 1)
        .find_map(|(index, row)| {
            // Standalone runs the agent in-process. Its exact serving MCP owner
            // must prove recursive emptiness; the job completion below also waits
            // for all registered process owners. Native tests separately exercise
            // the out-of-process runner's outer Job containment.
            let root = &row["fields"]["root_pid"];
            (row["fields"]["event"] == "worker.containment.cleanup_completed"
                && row["fields"]["owner_kind"] == "mcp_server"
                && (root == frontend.pid || root == process_owner.pid)
                && row["fields"]["recursive_empty"] == "proven"
                && row["fields"]["direct_child_reap"] == "reaped")
                .then_some(index)
        })
    else {
        return false;
    };
    owned
        .iter()
        .skip(cleaned + 1)
        .any(|row| row["fields"]["event"] == "worker.job.cancellation_completed")
}

pub(super) fn all_frontends_and_helpers_gone(rows: &[Value]) -> Result<bool, String> {
    for row in event(rows, "attached") {
        for value in
            std::iter::once(&row["identity"]).chain(row["owners"].as_array().into_iter().flatten())
        {
            let id: Identity = serde_json::from_value(value.clone())
                .map_err(|_| "invalid final frontend/helper identity")?;
            if id.live() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Failure-only hygiene, never evidence for natural cleanup.
pub(super) struct FailureCleanup {
    path: PathBuf,
    armed: bool,
}
impl FailureCleanup {
    pub(super) fn new(path: &Path) -> Self {
        Self {
            path: path.into(),
            armed: true,
        }
    }
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for FailureCleanup {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(rows) = rows(&self.path) else {
            return;
        };
        for row in rows.iter().rev() {
            for value in std::iter::once(&row["identity"])
                .chain(row["owners"].as_array().into_iter().flatten())
            {
                if let Ok(id) = serde_json::from_value::<Identity>(value.clone()) {
                    if id.live() {
                        let _ = Command::new("kill")
                            .arg("-KILL")
                            .arg(id.pid.to_string())
                            .status();
                    }
                }
            }
        }
    }
}
