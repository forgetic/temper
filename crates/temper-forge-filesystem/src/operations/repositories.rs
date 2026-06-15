use crate::FilesystemForge;
use crate::lists::{sort_labels, sort_repositories};
use crate::metadata::next_timestamp;
use crate::record_ids::label_id;
use crate::validation::{validate_create_repository, validate_upsert_label};
use temper_forge::{
    ChangeKind, CreateRepository, ForgeError, ForgeResult, Label, Repository, RepositoryId,
    RepositoryPath, RepositoryQuery, UpsertLabel, User, UserId,
};

pub(crate) fn current_user(forge: &FilesystemForge) -> ForgeResult<User> {
    let metadata = forge.read_metadata()?;
    Ok(forge.effective_user(&metadata))
}

pub(crate) fn get_user(user: User, id: &UserId) -> Option<User> {
    (&user.id == id).then_some(user)
}

pub(crate) fn list_repositories(
    forge: &FilesystemForge,
    query: RepositoryQuery,
) -> ForgeResult<Vec<Repository>> {
    let mut repositories = forge.read_repositories()?;
    sort_repositories(&mut repositories, query);
    Ok(repositories)
}

pub(crate) fn create_repository(
    forge: &FilesystemForge,
    input: CreateRepository,
) -> ForgeResult<Repository> {
    validate_create_repository(&input)?;

    let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
    if forge.find_repository_by_path(&path)?.is_some() {
        return Err(ForgeError::AlreadyExists(format!(
            "repository {}/{}",
            input.owner, input.name
        )));
    }

    let mut metadata = forge.read_metadata()?;
    let now = next_timestamp(&mut metadata)?;
    let repository = Repository {
        id: forge.next_repository_id(&mut metadata)?,
        owner: input.owner,
        name: input.name,
        default_branch: input.default_branch,
        description: input.description,
        created_at: now,
        updated_at: now,
    };

    let repository_path = forge.repository_file(&repository.id).ok_or_else(|| {
        ForgeError::Backend(format!("generated invalid repository id {}", repository.id))
    })?;
    forge.write_json(&repository_path, &repository)?;
    forge.write_metadata(&metadata)?;
    forge.publish_path_hint(
        RepositoryPath::new(repository.owner.clone(), repository.name.clone()),
        ChangeKind::Unknown,
    );

    Ok(repository)
}

pub(crate) fn list_labels(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
) -> ForgeResult<Vec<Label>> {
    forge.require_repository(repo_id)?;
    forge.read_labels_for_existing_repository(repo_id)
}

pub(crate) fn upsert_label(
    forge: &FilesystemForge,
    repo_id: &RepositoryId,
    input: UpsertLabel,
) -> ForgeResult<Label> {
    forge.require_repository(repo_id)?;
    validate_upsert_label(&input)?;

    let mut labels = forge.read_labels_for_existing_repository(repo_id)?;
    let label = if let Some(existing) = labels.iter_mut().find(|label| label.name == input.name) {
        existing.color = input.color;
        existing.description = input.description;
        existing.clone()
    } else {
        let label = Label {
            id: label_id(repo_id, &input.name),
            repo_id: repo_id.clone(),
            name: input.name,
            color: input.color,
            description: input.description,
        };
        labels.push(label.clone());
        label
    };

    sort_labels(&mut labels);
    forge.write_labels(repo_id, &labels)?;
    forge.publish_repo_hint(repo_id, ChangeKind::Label);

    Ok(label)
}
