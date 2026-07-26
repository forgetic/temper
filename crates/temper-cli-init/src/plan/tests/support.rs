use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use temper_forge::{Repository, RepositoryId};

use crate::deployment::{DeploymentBundle, DesiredRepository};
use crate::plan::inspection::{DeploymentInspector, ForgeInspection};

pub(super) struct RecordingInspector {
    pub(super) inspections: BTreeMap<String, Result<ForgeInspection, String>>,
    pub(super) calls: Vec<String>,
}

impl DeploymentInspector for RecordingInspector {
    fn inspect_repository(
        &mut self,
        _bundle: &DeploymentBundle,
        repository: &DesiredRepository,
    ) -> Result<ForgeInspection, String> {
        let path = format!(
            "{}/{}",
            repository.plan.repo.owner, repository.plan.repo.name
        );
        self.calls.push(path.clone());
        self.inspections
            .get(&path)
            .cloned()
            .unwrap_or_else(|| Ok(ForgeInspection::default()))
    }

    fn inspect_users(
        &mut self,
        _bundle: &DeploymentBundle,
        users: &[String],
    ) -> Result<BTreeMap<String, bool>, String> {
        Ok(users.iter().cloned().map(|user| (user, true)).collect())
    }
}

pub(super) fn write_bundle(root: &Path, repos: &[&str]) -> PathBuf {
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle");
    std::fs::write(bundle.join("webhook-secret"), "webhook-secret-value").expect("webhook");
    let repos = repos
        .iter()
        .map(|repo| format!("\"{repo}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        bundle.join("config.toml"),
        format!(
            "schema_version = 1\n\
             [forge]\n\
             url = \"http://forge.local:3000\"\n\
             admin = \"root\"\n\
             [engine]\n\
             bind = \"127.0.0.1:38100\"\n\
             repos = [{repos}]\n\
             roles = [\"architect\", \"engineer\"]\n\
             webhook_secret_file = \"webhook-secret\"\n"
        ),
    )
    .expect("config");
    std::fs::write(
        bundle.join("credentials.toml"),
        "schema_version = 1\n\
         [forge.users.root]\n\
         password = \"admin-pass\"\n\
         token = \"admin-token\"\n\
         [agent.providers.deepseek]\n\
         type = \"api-key\"\n\
         key = \"provider-key\"\n",
    )
    .expect("credentials");
    bundle
}

pub(super) fn repository(owner: &str, name: &str) -> Repository {
    Repository {
        id: RepositoryId::new(format!("{owner}/{name}")),
        owner: owner.to_string(),
        name: name.to_string(),
        default_branch: "main".to_string(),
        description: None,
        created_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
        updated_at: chrono::DateTime::from_timestamp(0, 0).expect("timestamp"),
    }
}
