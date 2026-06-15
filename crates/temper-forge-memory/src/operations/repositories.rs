//! User, repository, and label operations for [`MemoryForge`](crate::MemoryForge).

use crate::MemoryForge;
use crate::ids::label_id;
use crate::lists::{sort_labels, sort_repositories};
use crate::util::{validate_create_repository, validate_upsert_label};
use temper_forge_model::{
    ChangeKind, CreateRepository, ForgeError, ForgeResult, Label, Repository, RepositoryId,
    RepositoryPath, RepositoryQuery, UpsertLabel, User, UserId,
};

pub(crate) fn current_user(forge: &MemoryForge) -> ForgeResult<User> {
    let inner = forge.lock();
    Ok(forge.effective_user(&inner))
}

pub(crate) fn get_user(forge: &MemoryForge, id: &UserId) -> ForgeResult<Option<User>> {
    let inner = forge.lock();
    let user = forge.effective_user(&inner);
    Ok((&user.id == id).then_some(user))
}

pub(crate) fn list_repositories(
    forge: &MemoryForge,
    query: RepositoryQuery,
) -> ForgeResult<Vec<Repository>> {
    let mut repositories = forge.lock().state.repositories();
    sort_repositories(&mut repositories, &query);
    Ok(repositories)
}

pub(crate) fn create_repository(
    forge: &MemoryForge,
    input: CreateRepository,
) -> ForgeResult<Repository> {
    validate_create_repository(&input)?;

    let mut inner = forge.lock();
    let path = RepositoryPath::new(input.owner.clone(), input.name.clone());
    if inner.state.find_repository_by_path(&path).is_some() {
        return Err(ForgeError::AlreadyExists(format!(
            "repository {}/{}",
            input.owner, input.name
        )));
    }

    let now = inner.state.next_timestamp()?;
    let id = inner.state.allocate_repository_id()?;
    let repository = Repository {
        id,
        owner: input.owner,
        name: input.name,
        default_branch: input.default_branch,
        description: input.description,
        created_at: now,
        updated_at: now,
    };
    inner.state.insert_repository(repository.clone());
    let path = RepositoryPath::new(repository.owner.clone(), repository.name.clone());
    inner.publish_path_hint(path, ChangeKind::Unknown);
    Ok(repository)
}

pub(crate) fn get_repository(
    forge: &MemoryForge,
    id: &RepositoryId,
) -> ForgeResult<Option<Repository>> {
    Ok(forge.lock().state.find_repository_by_id(id))
}

pub(crate) fn get_repository_by_path(
    forge: &MemoryForge,
    path: &RepositoryPath,
) -> ForgeResult<Option<Repository>> {
    Ok(forge.lock().state.find_repository_by_path(path))
}

pub(crate) fn list_labels(forge: &MemoryForge, repo_id: &RepositoryId) -> ForgeResult<Vec<Label>> {
    let inner = forge.lock();
    inner.state.require_repository(repo_id)?;
    let mut labels = inner.state.labels(repo_id);
    sort_labels(&mut labels);
    Ok(labels)
}

pub(crate) fn upsert_label(
    forge: &MemoryForge,
    repo_id: &RepositoryId,
    input: UpsertLabel,
) -> ForgeResult<Label> {
    validate_upsert_label(&input)?;

    let mut inner = forge.lock();
    inner.state.require_repository(repo_id)?;
    let id = label_id(repo_id, &input.name);
    let labels = inner.state.labels_mut(repo_id);
    let label = if let Some(existing) = labels.iter_mut().find(|label| label.name == input.name) {
        existing.color = input.color;
        existing.description = input.description;
        existing.clone()
    } else {
        let label = Label {
            id,
            repo_id: repo_id.clone(),
            name: input.name,
            color: input.color,
            description: input.description,
        };
        labels.push(label.clone());
        label
    };
    sort_labels(labels);
    inner.publish_repo_hint(repo_id, ChangeKind::Label);
    Ok(label)
}
