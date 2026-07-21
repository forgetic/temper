use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::recovery::{RepositoryState, output_string};
use super::{Workspace, WorkspaceError};

impl Workspace {
    pub(super) async fn inspect_repository(&self) -> Result<RepositoryState, WorkspaceError> {
        let head = self.optional_ref("HEAD").await?;
        let branch_output = self
            .run_workspace_git(
                false,
                "git branch --show-current".to_string(),
                vec![OsString::from("branch"), OsString::from("--show-current")],
            )
            .await?;
        let branch = output_string(branch_output.stdout)?;
        let branch = (!branch.trim().is_empty()).then(|| branch.trim().to_string());
        let status_paths = self.status_paths().await?;
        let git_dir = self.resolve_git_dir().await?;
        let operation = self
            .run_blocking("temper-workspace-operation-inspect", move || {
                detect_operation(&git_dir)
            })
            .await?;
        Ok(RepositoryState {
            branch,
            head,
            status_paths,
            operation,
        })
    }

    async fn resolve_git_dir(&self) -> Result<PathBuf, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                "git rev-parse --absolute-git-dir".to_string(),
                vec![
                    OsString::from("rev-parse"),
                    OsString::from("--absolute-git-dir"),
                ],
            )
            .await?;
        Ok(PathBuf::from(output_string(output.stdout)?.trim()))
    }

    pub(super) async fn stash_cleanup_paths(
        &self,
        stash_ref: &str,
    ) -> Result<Vec<String>, WorkspaceError> {
        let stash_parent = format!("{stash_ref}^1");
        let output = self
            .run_workspace_git(
                false,
                "git diff --name-only -z <stash-parent> <preserved-stash>".to_string(),
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from("-z"),
                    OsString::from(stash_parent),
                    OsString::from(stash_ref),
                ],
            )
            .await?;
        let mut paths = nul_paths(output.stdout)?
            .into_iter()
            .collect::<BTreeSet<_>>();

        let untracked_parent = format!("{stash_ref}^3");
        if self.optional_ref(&untracked_parent).await?.is_some() {
            let output = self
                .run_workspace_git(
                    false,
                    "git ls-tree -r -z --name-only <preserved-stash-untracked>".to_string(),
                    vec![
                        OsString::from("ls-tree"),
                        OsString::from("-r"),
                        OsString::from("-z"),
                        OsString::from("--name-only"),
                        OsString::from(untracked_parent),
                    ],
                )
                .await?;
            paths.extend(nul_paths(output.stdout)?);
        }
        Ok(paths.into_iter().collect())
    }

    pub(super) async fn normalize_failed_replay(
        &self,
        expected_head: &str,
    ) -> Result<(), WorkspaceError> {
        match self.inspect_repository().await?.operation.as_deref() {
            Some("cherry-pick") => {
                self.run_workspace_git(
                    false,
                    "git cherry-pick --abort".to_string(),
                    vec![OsString::from("cherry-pick"), OsString::from("--abort")],
                )
                .await?;
            }
            Some(operation) => {
                return Err(WorkspaceError::Recovery(format!(
                    "expected failed cherry-pick state, found active {operation} operation"
                )));
            }
            None => {}
        }
        self.verify_normalized_state(expected_head).await
    }

    pub(super) async fn normalize_failed_stash_apply(
        &self,
        expected_head: &str,
        untracked_paths: &[String],
    ) -> Result<(), WorkspaceError> {
        self.run_workspace_git(
            false,
            format!("git reset --hard {expected_head}"),
            vec![
                OsString::from("reset"),
                OsString::from("--hard"),
                OsString::from(expected_head),
            ],
        )
        .await?;
        for paths in untracked_paths.chunks(128) {
            let mut args = vec![
                OsString::from("clean"),
                OsString::from("-f"),
                OsString::from("--"),
            ];
            args.extend(paths.iter().map(OsString::from));
            self.run_workspace_git(
                false,
                "git clean -f -- <preserved-stash-untracked-paths>".to_string(),
                args,
            )
            .await?;
        }
        self.verify_normalized_state(expected_head).await
    }

    async fn verify_normalized_state(&self, expected_head: &str) -> Result<(), WorkspaceError> {
        let state = self.inspect_repository().await?;
        if state.head.as_deref() != Some(expected_head) {
            return Err(WorkspaceError::Recovery(format!(
                "normalization HEAD is `{}`, expected `{expected_head}`",
                state.head.as_deref().unwrap_or("unresolved")
            )));
        }
        if let Some(operation) = state.operation {
            return Err(WorkspaceError::Recovery(format!(
                "normalization left an active {operation} operation"
            )));
        }
        let unmerged = self.unmerged_paths().await?;
        if !unmerged.is_empty() {
            return Err(WorkspaceError::Recovery(format!(
                "normalization left unmerged paths: {}",
                unmerged.join(", ")
            )));
        }
        if !state.status_paths.is_empty() {
            return Err(WorkspaceError::Recovery(format!(
                "normalization left changed paths: {}",
                state.status_paths.join(", ")
            )));
        }
        Ok(())
    }

    async fn unmerged_paths(&self) -> Result<Vec<String>, WorkspaceError> {
        let output = self
            .run_workspace_git(
                false,
                "git diff --name-only --diff-filter=U -z".to_string(),
                vec![
                    OsString::from("diff"),
                    OsString::from("--name-only"),
                    OsString::from("--diff-filter=U"),
                    OsString::from("-z"),
                ],
            )
            .await?;
        nul_paths(output.stdout)
    }
}

fn nul_paths(output: Vec<u8>) -> Result<Vec<String>, WorkspaceError> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            String::from_utf8(path.to_vec())
                .map_err(|error| WorkspaceError::Utf8(error.to_string()))
        })
        .collect()
}

fn detect_operation(git_dir: &Path) -> Option<String> {
    [
        ("MERGE_HEAD", "merge"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_LOG", "bisect"),
        ("sequencer", "sequencer"),
    ]
    .into_iter()
    .find_map(|(marker, operation)| git_dir.join(marker).exists().then(|| operation.to_string()))
}
