use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::recovery_plan::{RecoveryPlanRequest, render as render_recovery_plan};
use super::{CheckoutState, Workspace, WorkspaceError};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryContext {
    pub job_id: String,
    pub correlation_key: String,
    pub repository: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuarantineManifest {
    pub job_id: String,
    pub correlation_key: String,
    pub repository: String,
    pub checkout_path: String,
    pub quarantine_path: String,
    pub original_branch: Option<String>,
    pub expected_branch: String,
    pub original_head: Option<String>,
    pub target_sha: Option<String>,
    pub original_status_paths: Vec<String>,
    pub recovery_refs: Vec<String>,
    pub failure_phase: String,
    pub failure: String,
    pub recovery_commands: Vec<String>,
    #[serde(default)]
    pub replay_commits: Vec<String>,
    #[serde(default)]
    pub recovery_notes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreparationOutcome {
    CleanReuse {
        head: String,
    },
    RecoveredLocalWork {
        original_head: String,
        prepared_head: String,
        recovery_refs: Vec<String>,
    },
    Quarantined(Box<QuarantineManifest>),
}

pub(super) enum ReadOnlyTarget {
    RemoteBranch(String),
    PullRequest(u64),
}

#[derive(Debug)]
pub(super) struct RepositoryState {
    pub(super) branch: Option<String>,
    pub(super) head: Option<String>,
    pub(super) status_paths: Vec<String>,
    pub(super) operation: Option<String>,
}

#[derive(Default)]
struct RecoveryArtifacts {
    refs: Vec<String>,
    stash_ref: Option<String>,
    replay_commits: Vec<String>,
}

impl Workspace {
    pub fn with_recovery_context(mut self, context: RecoveryContext) -> Self {
        self.recovery_context = context;
        self
    }

    pub(super) fn quarantine_path(&self) -> PathBuf {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("checkout");
        self.path
            .with_file_name(format!("{name}.temper-quarantine"))
    }

    pub(super) async fn existing_quarantine(
        &self,
    ) -> Result<Option<QuarantineManifest>, WorkspaceError> {
        let quarantine_path = self.quarantine_path();
        if !quarantine_path.exists() {
            return Ok(None);
        }
        let manifest_path = quarantine_path.join("temper-recovery.json");
        let bytes = self
            .run_blocking("temper-workspace-quarantine-read", move || {
                std::fs::read(&manifest_path)
            })
            .await??;
        let manifest = serde_json::from_slice(&bytes).map_err(|error| {
            WorkspaceError::Recovery(format!(
                "quarantine {} exists but its manifest cannot be read: {error}",
                quarantine_path.display()
            ))
        })?;
        Ok(Some(manifest))
    }

    pub(super) async fn prepare_writable_recovering(
        &self,
        work_branch: &str,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        if let Some(manifest) = self.existing_quarantine().await? {
            return Ok(PreparationOutcome::Quarantined(Box::new(manifest)));
        }

        let checkout_state = self.ensure_checkout_repo().await?;
        self.prepare_writable_after_checkout(work_branch, checkout_state)
            .await
    }

    pub(super) async fn prepare_writable_after_checkout(
        &self,
        work_branch: &str,
        checkout_state: CheckoutState,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let state = if checkout_state == CheckoutState::Reused {
            let state = self.inspect_repository().await?;
            state.head.is_some().then_some(state)
        } else {
            None
        };

        if let Some(state) = state.as_ref() {
            if let Some(operation) = &state.operation {
                return self
                    .preserve_and_quarantine(
                        state,
                        work_branch,
                        None,
                        "inspect-operation",
                        format!("repository has unresolved {operation} state"),
                    )
                    .await;
            }
            if state.branch.as_deref() != Some(work_branch) {
                return self
                    .preserve_and_quarantine(
                        state,
                        work_branch,
                        None,
                        "inspect-branch",
                        format!(
                            "expected branch `{work_branch}`, found `{}`",
                            state.branch.as_deref().unwrap_or("detached HEAD")
                        ),
                    )
                    .await;
            }
        }

        let previous_remote_head = if state.is_some() {
            self.optional_ref(&format!("refs/remotes/origin/{work_branch}"))
                .await?
        } else {
            None
        };

        // Fetches are non-destructive. Do them after inspecting local state but
        // before stashing so a transient remote failure leaves local work exactly
        // where it was and remains safely retryable.
        self.fetch_remote_branch(&self.base_branch).await?;
        let anchor = if self.remote_branch_exists(work_branch).await? {
            self.fetch_remote_branch(work_branch).await?;
            format!("origin/{work_branch}")
        } else {
            format!("origin/{}", self.base_branch)
        };
        let target_sha = self.resolve_ref(&anchor).await?;

        let Some(state) = state else {
            self.checkout_branch(work_branch, &anchor).await?;
            return Ok(PreparationOutcome::CleanReuse {
                head: self.head_sha().await?,
            });
        };
        let original_head = state.head.clone().ok_or_else(|| {
            WorkspaceError::Recovery("existing checkout has no resolvable HEAD".to_string())
        })?;
        let commits = if previous_remote_head.as_deref() == Some(original_head.as_str()) {
            // This checkout exactly matched the remote-tracking branch before
            // the fetch. If the forge branch was force-updated since then (as
            // happens when an issue workstream transitions into PR repair), the
            // old commits are not interrupted local-only work and must not be
            // replayed over the newly assigned remote head.
            Vec::new()
        } else {
            self.local_commits(&anchor, &original_head).await?
        };
        let needs_recovery = !commits.is_empty() || !state.status_paths.is_empty();
        if !needs_recovery {
            self.checkout_branch(work_branch, &anchor).await?;
            return Ok(PreparationOutcome::CleanReuse {
                head: self.head_sha().await?,
            });
        }

        let mut artifacts = RecoveryArtifacts {
            replay_commits: commits.clone(),
            ..RecoveryArtifacts::default()
        };
        if let Err(error) = self.preserve_state(&state, &mut artifacts).await {
            return self
                .quarantine(
                    &state,
                    work_branch,
                    Some(target_sha),
                    "preserve",
                    error.to_string(),
                    artifacts,
                )
                .await;
        }

        if let Err(error) = self.checkout_branch(work_branch, &anchor).await {
            return self
                .quarantine(
                    &state,
                    work_branch,
                    Some(target_sha),
                    "checkout-anchor",
                    error.to_string(),
                    artifacts,
                )
                .await;
        }
        if !commits.is_empty() {
            let pre_replay_head = self.head_sha().await?;
            let mut args = vec![OsString::from("cherry-pick")];
            args.extend(commits.iter().map(OsString::from));
            if let Err(error) = self
                .run_workspace_git(
                    false,
                    "git cherry-pick <recovery-commits>".to_string(),
                    args,
                )
                .await
            {
                let failure = error.to_string();
                let (phase, failure) = match self.normalize_failed_replay(&pre_replay_head).await {
                    Ok(()) => (
                        "replay-commits",
                        format!("{failure}; failed cherry-pick was aborted and normalized"),
                    ),
                    Err(normalization) => (
                        "replay-commits-ambiguous",
                        format!("{failure}; cherry-pick normalization failed: {normalization}"),
                    ),
                };
                return self
                    .quarantine(
                        &state,
                        work_branch,
                        Some(target_sha),
                        phase,
                        failure,
                        artifacts,
                    )
                    .await;
            }
        }
        if let Some(stash_ref) = artifacts.stash_ref.as_ref() {
            let pre_apply_head = self.head_sha().await?;
            let cleanup_paths = self.stash_cleanup_paths(stash_ref).await;
            if let Err(error) = self
                .run_workspace_git(
                    false,
                    format!("git stash apply --index {stash_ref}"),
                    vec![
                        OsString::from("stash"),
                        OsString::from("apply"),
                        OsString::from("--index"),
                        OsString::from(stash_ref),
                    ],
                )
                .await
            {
                let failure = error.to_string();
                let normalization = match cleanup_paths {
                    Ok(paths) => {
                        self.normalize_failed_stash_apply(&pre_apply_head, &paths)
                            .await
                    }
                    Err(error) => Err(error),
                };
                let (phase, failure) = match normalization {
                    Ok(()) => (
                        "restore-worktree",
                        format!("{failure}; failed stash apply was reset and normalized"),
                    ),
                    Err(normalization) => (
                        "restore-worktree-ambiguous",
                        format!("{failure}; stash-apply normalization failed: {normalization}"),
                    ),
                };
                return self
                    .quarantine(
                        &state,
                        work_branch,
                        Some(target_sha),
                        phase,
                        failure,
                        artifacts,
                    )
                    .await;
            }
        }

        if let Err(error) = self.verify_recovery(&state, &artifacts).await {
            return self
                .quarantine(
                    &state,
                    work_branch,
                    Some(target_sha),
                    "verify",
                    error.to_string(),
                    artifacts,
                )
                .await;
        }

        let prepared_head = self.head_sha().await?;
        Ok(PreparationOutcome::RecoveredLocalWork {
            original_head,
            prepared_head,
            recovery_refs: artifacts.refs,
        })
    }

    pub(super) async fn prepare_read_only_target(
        &self,
        expected_branch: &str,
        target: ReadOnlyTarget,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        if let Some(manifest) = self.existing_quarantine().await? {
            return Ok(PreparationOutcome::Quarantined(Box::new(manifest)));
        }
        let checkout_state = self.ensure_checkout_repo().await?;
        self.prepare_read_only_after_checkout(expected_branch, target, checkout_state)
            .await
    }

    pub(super) async fn prepare_read_only_after_checkout(
        &self,
        expected_branch: &str,
        target: ReadOnlyTarget,
        checkout_state: CheckoutState,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let state = if checkout_state == CheckoutState::Reused {
            let state = self.inspect_repository().await?;
            state.head.is_some().then_some(state)
        } else {
            None
        };
        if let Some(state) = state.as_ref() {
            let reason = state
                .operation
                .as_ref()
                .map(|operation| format!("repository has unresolved {operation} state"))
                .or_else(|| {
                    (!state.status_paths.is_empty()).then(|| {
                        "read-only checkout contains staged, tracked, or untracked edits"
                            .to_string()
                    })
                })
                .or_else(|| {
                    (state.branch.as_deref() != Some(expected_branch)).then(|| {
                        format!(
                            "expected branch `{expected_branch}`, found `{}`",
                            state.branch.as_deref().unwrap_or("detached HEAD")
                        )
                    })
                });
            if let Some(reason) = reason {
                return self
                    .preserve_and_quarantine(
                        state,
                        expected_branch,
                        None,
                        "inspect-read-only",
                        reason,
                    )
                    .await;
            }
        }

        let target_ref = match target {
            ReadOnlyTarget::RemoteBranch(branch) => {
                self.fetch_remote_branch(&branch).await?;
                format!("origin/{branch}")
            }
            ReadOnlyTarget::PullRequest(number) => {
                // Fetch the base as well so the checkout retains the same complete
                // remote view as writable preparation.
                self.fetch_remote_branch(&self.base_branch).await?;
                let remote_ref = format!("refs/pull/{number}/head");
                let local_ref = format!("refs/temper/pr/{number}/head");
                let refspec = format!("+{remote_ref}:{local_ref}");
                self.run_workspace_git(
                    true,
                    format!("git fetch origin {refspec}"),
                    vec![
                        OsString::from("fetch"),
                        OsString::from("origin"),
                        OsString::from(refspec),
                    ],
                )
                .await?;
                local_ref
            }
        };
        let target_sha = self.resolve_ref(&target_ref).await?;
        if let Some(state) = state.as_ref() {
            if let Some(original_head) = state.head.as_ref() {
                if !self
                    .local_commits(&target_ref, original_head)
                    .await?
                    .is_empty()
                {
                    return self
                        .preserve_and_quarantine(
                            state,
                            expected_branch,
                            Some(target_sha),
                            "read-only-local-commits",
                            "read-only checkout contains local-only commits".to_string(),
                        )
                        .await;
                }
            }
        }
        self.checkout_branch(expected_branch, &target_ref).await?;
        Ok(PreparationOutcome::CleanReuse {
            head: self.head_sha().await?,
        })
    }

    async fn preserve_and_quarantine(
        &self,
        state: &RepositoryState,
        expected_branch: &str,
        target_sha: Option<String>,
        phase: &str,
        reason: String,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let mut artifacts = RecoveryArtifacts::default();
        let (phase, reason) = match self.preserve_state(state, &mut artifacts).await {
            Ok(()) => (phase.to_string(), reason),
            Err(error) => (
                "preserve".to_string(),
                format!("{reason}; preservation failed: {error}"),
            ),
        };
        self.quarantine(
            state,
            expected_branch,
            target_sha,
            &phase,
            reason,
            artifacts,
        )
        .await
    }

    async fn preserve_state(
        &self,
        state: &RepositoryState,
        artifacts: &mut RecoveryArtifacts,
    ) -> Result<(), WorkspaceError> {
        let Some(head) = state.head.as_ref() else {
            return Err(WorkspaceError::Recovery(
                "cannot preserve a checkout without HEAD".to_string(),
            ));
        };
        let namespace = self.recovery_namespace(head);
        let head_ref = format!("{namespace}/original-head");
        self.create_immutable_ref(&head_ref, head).await?;
        artifacts.refs.push(head_ref);

        if state.status_paths.is_empty() {
            return Ok(());
        }
        self.run_workspace_git(
            false,
            "git stash push --include-untracked <recovery>".to_string(),
            vec![
                OsString::from("stash"),
                OsString::from("push"),
                OsString::from("--include-untracked"),
                OsString::from("--message"),
                OsString::from(format!("temper recovery for {head}")),
            ],
        )
        .await?;
        let stash_sha = self.resolve_ref("refs/stash").await?;
        let stash_ref = format!("{namespace}/worktree-{stash_sha}");
        self.create_immutable_ref(&stash_ref, &stash_sha).await?;
        artifacts.refs.push(stash_ref.clone());
        artifacts.stash_ref = Some(stash_ref);
        Ok(())
    }

    async fn create_immutable_ref(&self, reference: &str, sha: &str) -> Result<(), WorkspaceError> {
        if let Some(existing) = self.optional_ref(reference).await? {
            if existing == sha {
                return Ok(());
            }
            return Err(WorkspaceError::Recovery(format!(
                "immutable recovery ref {reference} already resolves to {existing}, not {sha}"
            )));
        }
        self.run_workspace_git(
            false,
            format!("git update-ref {reference} {sha} <absent>"),
            vec![
                OsString::from("update-ref"),
                OsString::from(reference),
                OsString::from(sha),
                OsString::from("0000000000000000000000000000000000000000"),
            ],
        )
        .await?;
        let resolved = self.resolve_ref(reference).await?;
        if resolved != sha {
            return Err(WorkspaceError::Recovery(format!(
                "recovery ref {reference} resolved to {resolved}, expected {sha}"
            )));
        }
        Ok(())
    }

    async fn verify_recovery(
        &self,
        state: &RepositoryState,
        artifacts: &RecoveryArtifacts,
    ) -> Result<(), WorkspaceError> {
        for reference in &artifacts.refs {
            self.resolve_ref(reference).await?;
        }
        if !state.status_paths.is_empty() {
            let represented: BTreeSet<_> = self.status_paths().await?.into_iter().collect();
            let missing: Vec<_> = state
                .status_paths
                .iter()
                .filter(|path| !represented.contains(*path))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Err(WorkspaceError::Recovery(format!(
                    "restored checkout no longer represents changed paths: {}",
                    missing.join(", ")
                )));
            }
        }
        Ok(())
    }

    async fn quarantine(
        &self,
        state: &RepositoryState,
        expected_branch: &str,
        target_sha: Option<String>,
        phase: &str,
        failure: String,
        artifacts: RecoveryArtifacts,
    ) -> Result<PreparationOutcome, WorkspaceError> {
        let quarantine_path = self.quarantine_path();
        if quarantine_path.exists() {
            if let Some(manifest) = self.existing_quarantine().await? {
                return Ok(PreparationOutcome::Quarantined(Box::new(manifest)));
            }
        }
        let plan = render_recovery_plan(RecoveryPlanRequest {
            failure_phase: phase,
            checkout_path: &self.path,
            quarantine_path: &quarantine_path,
            expected_branch,
            git_user: &self.identity.user,
            git_email: &self.identity.email,
            target_sha: target_sha.as_deref(),
            recovery_refs: &artifacts.refs,
            stash_ref: artifacts.stash_ref.as_deref(),
            replay_commits: &artifacts.replay_commits,
        });
        let manifest = QuarantineManifest {
            job_id: self.recovery_context.job_id.clone(),
            correlation_key: self.recovery_context.correlation_key.clone(),
            repository: self.recovery_context.repository.clone(),
            checkout_path: self.path.display().to_string(),
            quarantine_path: quarantine_path.display().to_string(),
            original_branch: state.branch.clone(),
            expected_branch: expected_branch.to_string(),
            original_head: state.head.clone(),
            target_sha,
            original_status_paths: state.status_paths.clone(),
            recovery_refs: artifacts.refs,
            failure_phase: phase.to_string(),
            failure,
            recovery_commands: plan.commands,
            replay_commits: artifacts.replay_commits,
            recovery_notes: plan.notes,
        };
        let bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| WorkspaceError::Recovery(error.to_string()))?;
        let checkout = self.path.clone();
        let destination = quarantine_path.clone();
        self.run_blocking(
            "temper-workspace-quarantine",
            move || -> Result<(), std::io::Error> {
                let temporary = checkout.join("temper-recovery.json.tmp");
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(&temporary, checkout.join("temper-recovery.json"))?;
                std::fs::rename(&checkout, &destination)?;
                Ok(())
            },
        )
        .await?
        .map_err(|error| {
            WorkspaceError::Recovery(format!(
                "failed to quarantine checkout at {}: {error}",
                quarantine_path.display()
            ))
        })?;
        Ok(PreparationOutcome::Quarantined(Box::new(manifest)))
    }

    async fn checkout_branch(&self, branch: &str, target: &str) -> Result<(), WorkspaceError> {
        self.run_workspace_git(
            false,
            format!("git checkout -B {branch} {target}"),
            vec![
                OsString::from("checkout"),
                OsString::from("-B"),
                OsString::from(branch),
                OsString::from(target),
            ],
        )
        .await?;
        Ok(())
    }

    async fn remote_branch_exists(&self, branch: &str) -> Result<bool, WorkspaceError> {
        let remote_ref = format!("refs/heads/{branch}");
        let output = self
            .run_workspace_git(
                true,
                format!("git ls-remote --heads origin {remote_ref}"),
                vec![
                    OsString::from("ls-remote"),
                    OsString::from("--heads"),
                    OsString::from("origin"),
                    OsString::from(remote_ref),
                ],
            )
            .await?;
        Ok(!output.stdout.is_empty())
    }

    async fn local_commits(
        &self,
        anchor: &str,
        original_head: &str,
    ) -> Result<Vec<String>, WorkspaceError> {
        let range = format!("{anchor}..{original_head}");
        let output = self
            .run_workspace_git(
                false,
                format!("git rev-list --reverse {range}"),
                vec![
                    OsString::from("rev-list"),
                    OsString::from("--reverse"),
                    OsString::from(range),
                ],
            )
            .await?;
        Ok(output_string(output.stdout)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect())
    }

    pub(super) async fn optional_ref(
        &self,
        reference: &str,
    ) -> Result<Option<String>, WorkspaceError> {
        // `show-ref --verify --quiet` would discard the value and its expected
        // non-zero "missing" status is awkward through the strict git wrapper.
        // `rev-parse --verify --quiet` is therefore run in one blocking command
        // with the same non-secret identity configuration.
        let command_reference = reference.to_string();
        let output = self
            .run_workspace_git_unchecked(
                false,
                vec![
                    OsString::from("rev-parse"),
                    OsString::from("--verify"),
                    OsString::from("--quiet"),
                    OsString::from(reference),
                ],
            )
            .await?;
        if output.status.success() {
            return Ok(Some(output_string(output.stdout)?.trim().to_string()));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        Err(WorkspaceError::Git {
            command: format!("git rev-parse --verify --quiet {command_reference}"),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn resolve_ref(&self, reference: &str) -> Result<String, WorkspaceError> {
        self.optional_ref(reference).await?.ok_or_else(|| {
            WorkspaceError::Recovery(format!("git ref `{reference}` does not resolve"))
        })
    }

    fn recovery_namespace(&self, head: &str) -> String {
        format!(
            "refs/temper/recovery/{}/{head}",
            recovery_ref_component(&self.recovery_context.correlation_key)
        )
    }
}

pub(super) fn output_string(output: Vec<u8>) -> Result<String, WorkspaceError> {
    String::from_utf8(output).map_err(|error| WorkspaceError::Utf8(error.to_string()))
}

fn recovery_ref_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if component.is_empty() {
        "unknown".to_string()
    } else {
        component
    }
}
