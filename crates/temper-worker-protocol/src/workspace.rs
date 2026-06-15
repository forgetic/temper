// SPDX-License-Identifier: MPL-2.0

use serde::{Deserialize, Serialize};

/// Whether a workspace repository may be edited (committed, pushed, PR'd) or is
/// present only so the combined build resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoAccess {
    Writable,
    ReadOnly,
}

/// One repository in a job's [`WorkspaceManifest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceRepo {
    /// Repository path, `owner/name`.
    pub repo: String,
    /// Relative directory under the workspace root where this repo is checked
    /// out. Chosen so inter-repo path dependencies resolve (e.g. `temper`,
    /// `smith`, `skein` as flat siblings).
    pub dir: String,
    pub access: RepoAccess,
    pub default_branch: String,
    pub base_branch: String,
    /// Work branch the worker pushes for a writable repo, e.g.
    /// `agent/coord-for-code-42`. Absent for read-only repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_hint: Option<String>,
    /// Other repos (by `owner/name` path) whose pull request must land before
    /// this repo's -- the coordinated landing order (ADR 0023). The daemon turns
    /// each into a cross-repo dependency link between the opened PRs. Empty for
    /// an independent repo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

impl WorkspaceRepo {
    pub fn is_writable(&self) -> bool {
        matches!(self.access, RepoAccess::Writable)
    }

    /// Splits `repo` into `(owner, name)`. `None` when not exactly `owner/name`.
    pub fn owner_name(&self) -> Option<(&str, &str)> {
        let mut parts = self.repo.split('/');
        let owner = parts.next()?;
        let name = parts.next()?;
        if owner.is_empty() || name.is_empty() || parts.next().is_some() {
            None
        } else {
            Some((owner, name))
        }
    }
}

/// The ordered set of repositories a coding job assembles into one workspace.
///
/// The first repo is the *primary* -- the home of the coordinating artifact, off
/// which leases, progress relay, and source-issue resolution key. The
/// `coordination_key` is the stable id for the whole pull-request set (and the
/// cross-plane progress correlation id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub coordination_key: String,
    pub repos: Vec<WorkspaceRepo>,
}

impl WorkspaceManifest {
    /// A one-repo writable manifest -- the degenerate single-repo job.
    pub fn single(
        repo: impl Into<String>,
        dir: impl Into<String>,
        default_branch: impl Into<String>,
        base_branch: impl Into<String>,
        branch_hint: impl Into<String>,
        coordination_key: impl Into<String>,
    ) -> Self {
        Self {
            coordination_key: coordination_key.into(),
            repos: vec![WorkspaceRepo {
                repo: repo.into(),
                dir: dir.into(),
                access: RepoAccess::Writable,
                default_branch: default_branch.into(),
                base_branch: base_branch.into(),
                branch_hint: Some(branch_hint.into()),
                depends_on: Vec::new(),
            }],
        }
    }

    /// The primary repository (home of the coordinating artifact).
    pub fn primary(&self) -> Option<&WorkspaceRepo> {
        self.repos.first()
    }

    pub fn writable(&self) -> impl Iterator<Item = &WorkspaceRepo> {
        self.repos.iter().filter(|repo| repo.is_writable())
    }
}
