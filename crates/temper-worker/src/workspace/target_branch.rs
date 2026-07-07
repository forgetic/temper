use std::ffi::OsString;

use super::{Workspace, WorkspaceError};

impl Workspace {
    /// Ensures the configured base branch exists on the forge, creating it
    /// from the repository default branch when it is missing. Issue-backed jobs
    /// use this when workflow metadata names a feature branch before any PR has
    /// created it. Callers deliberately avoid invoking it for read-only sibling
    /// repositories and PR-head repair/review checkouts.
    pub async fn ensure_base_branch_exists_from_default(
        &self,
        default_branch: &str,
    ) -> Result<(), WorkspaceError> {
        let default_branch = default_branch.trim();
        if default_branch.is_empty() || self.base_branch == default_branch {
            return Ok(());
        }

        self.ensure_checkout_repo().await?;

        let target_fetch_error = match self.fetch_remote_branch(&self.base_branch).await {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        if let Err(default_fetch_error) = self.fetch_remote_branch(default_branch).await {
            return match self.fetch_remote_branch(&self.base_branch).await {
                Ok(()) => Ok(()),
                Err(target_refetch_error) => Err(WorkspaceError::BranchMaterialization(format!(
                    "target branch `{}` is missing and default branch `{default_branch}` could not be fetched; target fetch failed: {target_fetch_error}; default fetch failed: {default_fetch_error}; target refetch failed: {target_refetch_error}",
                    self.base_branch
                ))),
            };
        }

        let refspec = format!(
            "refs/remotes/origin/{default_branch}:refs/heads/{}",
            self.base_branch
        );
        // The empty force-with-lease expectation means "create only if the
        // remote ref is still absent". It makes the create race-safe without
        // permitting even a fast-forward update of an existing target branch.
        let target_must_be_absent = format!("--force-with-lease=refs/heads/{}:", self.base_branch);
        match self
            .run_workspace_git(
                true,
                format!(
                    "git push origin --force-with-lease=<target-absent> origin/{default_branch}:refs/heads/{}",
                    self.base_branch
                ),
                vec![
                    OsString::from("push"),
                    OsString::from("origin"),
                    OsString::from(target_must_be_absent),
                    OsString::from(refspec),
                ],
            )
            .await
        {
            Ok(_) => self.fetch_remote_branch(&self.base_branch).await.map_err(|error| {
                WorkspaceError::BranchMaterialization(format!(
                    "created target branch `{}` from default branch `{default_branch}`, but refetch failed: {error}",
                    self.base_branch
                ))
            }),
            Err(create_error) => match self.fetch_remote_branch(&self.base_branch).await {
                Ok(()) => Ok(()),
                Err(target_refetch_error) => Err(WorkspaceError::BranchMaterialization(format!(
                    "target branch `{}` was missing and could not be created from default branch `{default_branch}` without force-updating an existing branch; create failed: {create_error}; target refetch failed: {target_refetch_error}",
                    self.base_branch
                ))),
            },
        }
    }
}
