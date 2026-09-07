//! Mapped two-job acceptance through real Forgejo ownership-loss cancellation.
mod configuration;
mod evidence;
mod forge;
mod jig;
mod processes;
mod router;
#[cfg(test)]
mod tests;

pub(in crate::live_manifest) use configuration::tune;
pub(in crate::live_manifest) use jig::Control;
pub(super) use jig::start;
pub(in crate::live_manifest) use router::JigRouter;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use temper_forge_forgejo::ForgejoForge;
use temper_forge_model::{ItemNumber, RepositoryId};

use super::FakeMcpServer;
use crate::live_manifest::process::{ChildGuard, engine_block_on};
use crate::live_manifest::{FinalStateEvidence, LiveCodebaseMemoryEvidence};
use processes::{Cohort, FailureCleanup, event, rows};

const PHASE_BOUND: Duration = Duration::from_secs(30);

#[allow(clippy::too_many_arguments)]
pub(in crate::live_manifest) fn converge(
    forge: &ForgejoForge,
    repository: &RepositoryId,
    issues: &BTreeMap<String, ItemNumber>,
    admin: &str,
    standalone: &mut ChildGuard,
    timeout: Duration,
    control: &Arc<Control>,
    mcp: &FakeMcpServer,
    standalone_log: &Path,
) -> Result<(FinalStateEvidence, LiveCodebaseMemoryEvidence), String> {
    if !cfg!(target_os = "linux") {
        return Err("shared lifecycle live acceptance requires Linux process identities".into());
    }
    let deadline = Instant::now() + timeout;
    let _gates = ReleaseGates(Arc::clone(control));
    let mut cleanup = FailureCleanup::new(&mcp.log_path);
    let a_number = *issues.get("owner").ok_or("missing owner issue binding")?;
    let b_number = *issues
        .get("source")
        .ok_or("missing survivor issue binding")?;
    let (a, cohort) = wait(standalone, control, deadline, "A cold admission", || {
        if !control.arrived(true) {
            return Err("A model gate not reached".into());
        }
        let a = forge::attempt(&forge::issue(forge, repository, a_number)?)?;
        let cohort = Cohort::capture(&rows(&mcp.log_path)?)?;
        Ok((a, cohort))
    })?;
    forge::activate(forge, &forge::issue(forge, repository, b_number)?)?;
    let (b, b_frontend, b_root) =
        wait(standalone, control, deadline, "B shared admission", || {
            if !control.arrived(false) {
                return Err("B model gate not reached".into());
            }
            let b = forge::attempt(&forge::issue(forge, repository, b_number)?)?;
            if a.job == b.job || a.attempt == b.attempt || a.worker != b.worker || a.boot != b.boot
            {
                return Err("A/B are not distinct concurrent attempts under one worker".into());
            }
            let (frontend, cwd) = cohort.survivor(&rows(&mcp.log_path)?)?;
            if !cohort.a_frontend.live() {
                return Err("A closed before B joined".into());
            }
            Ok((b, frontend, cwd))
        })?;
    processes::validate_continuity(&rows(&mcp.log_path)?)?;
    let withdrawn = wait(
        standalone,
        control,
        deadline,
        "real Forgejo A withdrawal",
        || forge::revoke(forge, &forge::issue(forge, repository, a_number)?, &a),
    )?;
    let a_comments = comments(forge, &withdrawn.id)?;
    wait(
        standalone,
        control,
        deadline,
        "A production containment",
        || {
            cohort.assert_live()?;
            if !b_frontend.live()
                || forge::attempt(&forge::issue(forge, repository, b_number)?)? != b
            {
                return Err("B lost its original attempt during A cancellation".into());
            }
            if !processes::cancellation_complete(
                &rows(standalone_log)?,
                &a.job,
                &a.attempt,
                &cohort.a_frontend,
                &cohort.a_process_owner,
            ) || cohort.a_frontend.live()
            {
                return Err("A exact cancellation/recursive cleanup is incomplete".into());
            }
            Ok(())
        },
    )?;
    let released_at = rows(&mcp.log_path)?
        .last()
        .and_then(|row| row["at"].as_u64())
        .unwrap_or_default();
    control.release(true);
    control.release(false);
    wait(
        standalone,
        control,
        deadline,
        "B exact post-cancellation source",
        || {
            control.completed()?;
            evidence::survivor_source(
                &rows(&mcp.log_path)?,
                &cohort,
                &b_frontend,
                &b_root,
                released_at,
            )
        },
    )?;
    let final_state = super::drive_codebase_memory_convergence(
        forge,
        repository,
        b_number,
        admin,
        standalone,
        deadline.saturating_duration_since(Instant::now()),
    )?;
    forge::unchanged_withdrawal(&withdrawn, &forge::issue(forge, repository, a_number)?)?;
    if comments(forge, &withdrawn.id)? != a_comments {
        return Err("late A result changed source comments".into());
    }
    wait(
        standalone,
        control,
        deadline,
        "natural last-session cleanup",
        || {
            let events = rows(&mcp.log_path)?;
            processes::validate_continuity(&events)?;
            if event(&events, "daemon_exited").len() != 1
                || event(&events, "work_exited").len() != 1
                || event(&events, "last_session_closed").len() != 1
            {
                return Err("provider has not reported natural final-session exit".into());
            }
            if cohort.daemon.live()
                || cohort.work.live()
                || cohort.owner.live()
                || b_frontend.live()
                || !processes::all_frontends_and_helpers_gone(&events)?
            {
                return Err("last-session descendants or helper remain alive".into());
            }
            Ok(())
        },
    )?;
    ensure_running(standalone)?;
    let evidence = evidence::publish(mcp, &rows(&mcp.log_path)?)?;
    cleanup.disarm();
    Ok((final_state, evidence))
}

fn comments(forge: &ForgejoForge, id: &temper_forge_model::IssueId) -> Result<Vec<String>, String> {
    engine_block_on(forge.list_issue_comments(id))
        .map(|comments| comments.into_iter().map(|comment| comment.body).collect())
        .map_err(|error| format!("read A comments: {error}"))
}

fn wait<T>(
    standalone: &mut ChildGuard,
    control: &Control,
    outer: Instant,
    phase: &str,
    mut check: impl FnMut() -> Result<T, String>,
) -> Result<T, String> {
    let deadline = outer.min(Instant::now() + PHASE_BOUND);
    loop {
        ensure_running(standalone)?;
        control.healthy()?;
        match check() {
            Ok(value) => return Ok(value),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!("{phase} deadline: {error}"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn ensure_running(standalone: &mut ChildGuard) -> Result<(), String> {
    if standalone.try_wait()?.is_some() {
        Err("standalone exited before lifecycle acceptance completed".into())
    } else {
        Ok(())
    }
}

struct ReleaseGates(Arc<Control>);
impl Drop for ReleaseGates {
    fn drop(&mut self) {
        self.0.release(true);
        self.0.release(false);
    }
}
