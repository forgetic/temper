//! Restart-acceptance fixture operations over the durable real-stack world.

use std::path::{Path, PathBuf};
use std::time::Duration;

use temper_engine::{MechanicalBackstopConfig, MechanicalScope, run_mechanical_backstop_tick};
use temper_forge_model::{
    CiJob, CiJobConclusion, CiJobId, CiJobQuery, CiJobStatus, Forge, ItemNumber,
};
use temper_runner::{Progress, RepositorySet, RepositoryTarget};

use super::git::{git_output_trim, path_str};
use super::stack::HermeticRealStack;

impl HermeticRealStack {
    /// Runs one production mechanical reconciliation pass over the durable
    /// primary repository. Restart scenarios call this while the replacement
    /// daemon's dispatch barrier is still closed to model startup ordering.
    pub async fn reconcile_startup_mechanical(&self) -> Result<Progress, String> {
        let repository = self
            .forge
            .get_repository(&self.primary_repo_id)
            .await
            .map_err(|error| format!("load primary repository: {error}"))?
            .ok_or_else(|| "primary repository disappeared".to_string())?;
        let target = RepositoryTarget::new(
            repository.id,
            temper_forge_model::RepositoryPath::new(repository.owner, repository.name),
        );
        run_mechanical_backstop_tick(
            self.forge.as_ref(),
            self.workflow.as_ref(),
            self.clock.now(),
            &MechanicalBackstopConfig {
                repositories: RepositorySet::new(vec![target]),
                cadence: Duration::ZERO,
                lease_policy: temper_workflow::LeasePolicy::new(chrono::Duration::seconds(300)),
                pull_request_merge_observer: None,
            },
            std::slice::from_ref(&self.mechanical_journal),
            &MechanicalScope::All,
        )
        .await
        .map_err(|error| format!("startup mechanical reconciliation: {error}"))
    }

    /// Adds one deterministic native CI observation for an exact PR head while
    /// retaining prior observations in the MemoryForge fixture.
    pub async fn seed_ci_for_head(
        &self,
        number: ItemNumber,
        head: impl Into<String>,
        status: CiJobStatus,
        conclusion: Option<CiJobConclusion>,
    ) -> Result<(), String> {
        let pull_request = self
            .forge
            .get_pull_request_by_number(&self.primary_repo_id, number)
            .await
            .map_err(|error| format!("load pull request #{number}: {error}"))?
            .ok_or_else(|| format!("pull request #{number} does not exist"))?;
        let mut jobs = self
            .forge
            .list_ci_jobs(&self.primary_repo_id, CiJobQuery::default())
            .await
            .map_err(|error| format!("list CI jobs: {error}"))?;
        let index = jobs.len() + 1;
        let now = self.clock.now() + chrono::Duration::seconds(index as i64);
        jobs.push(CiJob {
            id: CiJobId::new(format!(
                "hermetic-ci-{}-{}-{index}",
                self.primary_repo_id.as_str(),
                number.get()
            )),
            repo_id: self.primary_repo_id.clone(),
            pull_request_id: Some(pull_request.id),
            commit_sha: head.into(),
            name: "hermetic-ci".to_string(),
            status,
            conclusion,
            url: None,
            created_at: now,
            started_at: (status != CiJobStatus::Queued).then_some(now),
            completed_at: (status == CiJobStatus::Completed).then_some(now),
            updated_at: now,
        });
        self.forge.seed_ci_jobs(&self.primary_repo_id, jobs);
        Ok(())
    }

    /// Advances a branch in one local bare origin with a deterministic commit.
    pub fn advance_origin_branch(
        &self,
        repo: &str,
        branch: &str,
        path: &str,
        contents: &str,
    ) -> Result<String, String> {
        let origin = self.origin(repo)?;
        let temp = tempfile::tempdir().map_err(|error| format!("create advance clone: {error}"))?;
        let checkout = temp.path().join("checkout");
        git_output_trim(&["clone", path_str(origin)?, path_str(&checkout)?])?;
        git_output_trim(&[
            "-C",
            path_str(&checkout)?,
            "checkout",
            "-B",
            branch,
            &format!("origin/{branch}"),
        ])?;
        let destination = checkout.join(path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        std::fs::write(&destination, contents)
            .map_err(|error| format!("write {}: {error}", destination.display()))?;
        git_output_trim(&[
            "-C",
            path_str(&checkout)?,
            "-c",
            "user.name=Hermetic Remote",
            "-c",
            "user.email=remote@example.test",
            "add",
            "--",
            path,
        ])?;
        git_output_trim(&[
            "-C",
            path_str(&checkout)?,
            "-c",
            "user.name=Hermetic Remote",
            "-c",
            "user.email=remote@example.test",
            "commit",
            "-m",
            "advance hermetic target branch",
        ])?;
        let head = git_output_trim(&["-C", path_str(&checkout)?, "rev-parse", "HEAD"])?;
        git_output_trim(&["-C", path_str(&checkout)?, "push", "origin", branch])?;
        Ok(head)
    }

    /// Finds the one prepared checkout for `repo` under the coordination-scoped
    /// worker root. This intentionally fails on zero or duplicate matches.
    pub fn workspace_checkout(&self, repo: &str) -> Result<PathBuf, String> {
        let leaf = repo.rsplit('/').next().unwrap_or(repo);
        let mut matches = Vec::new();
        collect_workspace_checkouts(&self.workspace_root, leaf, &mut matches)?;
        match matches.as_slice() {
            [path] => Ok(path.clone()),
            [] => Err(format!("no prepared checkout for `{repo}`")),
            _ => Err(format!(
                "multiple prepared checkouts for `{repo}`: {}",
                matches
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// Number of persisted engineer session records in the durable workspace.
    pub fn persisted_session_count(&self) -> Result<usize, String> {
        count_named_files(&self.workspace_root, "state.json")
    }
}

fn collect_workspace_checkouts(
    root: &Path,
    repo_leaf: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read {}: {error}", root.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {} entry: {error}", root.display()))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some(repo_leaf)
            && path.join(".git").exists()
        {
            matches.push(path);
        } else {
            collect_workspace_checkouts(&path, repo_leaf, matches)?;
        }
    }
    Ok(())
}

fn count_named_files(root: &Path, name: &str) -> Result<usize, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("read {}: {error}", root.display())),
    };
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {} entry: {error}", root.display()))?;
        let path = entry.path();
        if path.is_dir() {
            count += count_named_files(&path, name)?;
        } else if path.file_name().and_then(|file| file.to_str()) == Some(name) {
            count += 1;
        }
    }
    Ok(count)
}
