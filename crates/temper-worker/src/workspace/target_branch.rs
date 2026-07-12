use std::ffi::OsString;
use std::path::PathBuf;

use super::{CheckoutState, PreparationOutcome, ReadOnlyTarget, Workspace, WorkspaceError};

impl Workspace {
    /// Materialize the configured base branch when necessary, then prepare the
    /// same checkout read-only. Keeping both phases in one operation preserves
    /// whether a no-checkout clone was created by this preparation attempt.
    pub(crate) async fn prepare_read_only_from_default(
        &self,
        default_branch: &str,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let checkout_state = self
            .ensure_base_branch_exists_from_default(default_branch)
            .await?;
        self.prepare_read_only_after_checkout(
            &self.base_branch,
            ReadOnlyTarget::RemoteBranch(self.base_branch.clone()),
            checkout_state,
        )
        .await
    }

    /// Materialize the configured base branch when necessary, then prepare a
    /// writable work branch while retaining checkout lifecycle ownership.
    pub(crate) async fn prepare_from_default(
        &self,
        default_branch: &str,
        work_branch: &str,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let checkout_state = self
            .ensure_base_branch_exists_from_default(default_branch)
            .await?;
        self.prepare_writable_after_checkout(work_branch, checkout_state)
            .await
    }

    /// Ensures the configured base branch exists on the forge, creating it
    /// from the repository default branch when it is missing. Issue-backed jobs
    /// use this when workflow metadata names a feature branch before any PR has
    /// created it. Callers deliberately avoid invoking it for read-only sibling
    /// repositories and PR-head repair/review checkouts.
    async fn ensure_base_branch_exists_from_default(
        &self,
        default_branch: &str,
    ) -> Result<CheckoutState, WorkspaceError> {
        if let Some(manifest) = self.existing_quarantine().await? {
            return Err(WorkspaceError::Quarantined {
                path: PathBuf::from(manifest.quarantine_path),
            });
        }

        let checkout_state = self.ensure_checkout_repo().await?;
        let default_branch = default_branch.trim();
        if default_branch.is_empty() || self.base_branch == default_branch {
            return Ok(checkout_state);
        }

        let target_fetch_error = match self.fetch_remote_branch(&self.base_branch).await {
            Ok(()) => return Ok(checkout_state),
            Err(error) => error,
        };

        if let Err(default_fetch_error) = self.fetch_remote_branch(default_branch).await {
            return match self.fetch_remote_branch(&self.base_branch).await {
                Ok(()) => Ok(checkout_state),
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
            Ok(_) => self
                .fetch_remote_branch(&self.base_branch)
                .await
                .map(|()| checkout_state)
                .map_err(|error| {
                    WorkspaceError::BranchMaterialization(format!(
                        "created target branch `{}` from default branch `{default_branch}`, but refetch failed: {error}",
                        self.base_branch
                    ))
                }),
            Err(create_error) => match self.fetch_remote_branch(&self.base_branch).await {
                Ok(()) => Ok(checkout_state),
                Err(target_refetch_error) => Err(WorkspaceError::BranchMaterialization(format!(
                    "target branch `{}` was missing and could not be created from default branch `{default_branch}` without force-updating an existing branch; create failed: {create_error}; target refetch failed: {target_refetch_error}",
                    self.base_branch
                ))),
            },
        }
    }
}
