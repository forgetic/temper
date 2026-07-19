use std::io;
use std::path::{Path, PathBuf};

use super::*;

/// One cgroup that could not be proven safe to reclaim and remove.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedStaleCgroup {
    path: PathBuf,
    diagnostic: String,
}

impl RetainedStaleCgroup {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn diagnostic(&self) -> &str {
        &self.diagnostic
    }
}

/// Bounded startup-scavenging result.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CgroupV2ScavengeReport {
    removed: Vec<PathBuf>,
    retained: Vec<RetainedStaleCgroup>,
    omitted: usize,
}

impl CgroupV2ScavengeReport {
    pub fn removed(&self) -> &[PathBuf] {
        &self.removed
    }

    pub fn retained(&self) -> &[RetainedStaleCgroup] {
        &self.retained
    }

    pub fn omitted(&self) -> usize {
        self.omitted
    }

    fn remember_removed(&mut self, path: PathBuf) {
        if self.removed.len() + self.retained.len() < MAX_SCAVENGE_DIAGNOSTICS {
            self.removed.push(path);
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }

    fn remember_retained(&mut self, path: PathBuf, error: impl ToString) {
        if self.removed.len() + self.retained.len() < MAX_SCAVENGE_DIAGNOSTICS {
            self.retained.push(RetainedStaleCgroup {
                path,
                diagnostic: bounded_diagnostic(error.to_string()),
            });
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }
}

impl CgroupV2BackendFactory {
    /// Reclaim only ownership roots whose PID/start-time fence proves that the
    /// creating process is gone. Live, malformed, legacy, and uninspectable
    /// roots are never signalled.
    pub fn scavenge_stale(&self) -> CgroupV2ScavengeReport {
        let mut report = CgroupV2ScavengeReport::default();
        let Some(root) = self.capability.dedicated_subtree() else {
            return report;
        };
        let workers = match owned_children(self.fs.as_ref(), root) {
            Ok(workers) => workers,
            Err(error) => {
                report.remember_retained(root.to_path_buf(), error);
                return report;
            }
        };
        for worker in workers {
            if !is_worker_root(&worker) {
                report.remember_retained(worker, "no worker ownership fence");
                continue;
            }
            let boots = match owned_children(self.fs.as_ref(), &worker) {
                Ok(boots) => boots,
                Err(error) => {
                    report.remember_retained(worker, error);
                    continue;
                }
            };
            for boot in boots {
                self.scavenge_boot(&mut report, boot);
            }
        }
        report
    }

    fn scavenge_boot(&self, report: &mut CgroupV2ScavengeReport, path: PathBuf) {
        let Some((pid, start_time)) = parse_boot_fence(&path) else {
            report.remember_retained(path, "invalid process-boot ownership fence");
            return;
        };
        match self.processes.identity(pid) {
            Ok(identity) if identity.start_time_identity() == start_time => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                report.remember_retained(path, format!("owner liveness probe failed: {error}"));
                return;
            }
        }
        match scavenge_one(
            self.fs.as_ref(),
            self.processes.as_ref(),
            &path,
            ROLLBACK_RETRIES,
        ) {
            Ok(()) => report.remember_removed(path),
            Err(error) => report.remember_retained(path, error),
        }
    }
}

fn owned_children(fs: &dyn CgroupFileSystem, root: &Path) -> io::Result<Vec<PathBuf>> {
    let children = fs.child_directories(root)?;
    if let Some(escaped) = children.iter().find(|path| !path.starts_with(root)) {
        return Err(io::Error::other(format!(
            "cgroup traversal escaped {} through {}",
            root.display(),
            escaped.display()
        )));
    }
    Ok(children)
}

fn is_worker_root(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("worker-") && name.len() > "worker-".len())
}

fn parse_boot_fence(path: &Path) -> Option<(u32, u64)> {
    let name = path.file_name()?.to_str()?;
    let fields = name.strip_prefix("boot-")?;
    let (pid, start_time) = fields.split_once('-')?;
    let pid = pid.parse().ok()?;
    let start_time = start_time.parse().ok()?;
    (name == format!("boot-{pid}-{start_time}")).then_some((pid, start_time))
}
