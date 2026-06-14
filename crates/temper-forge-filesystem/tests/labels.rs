mod support;

use support::{TestRoot, block_on, repository};
use temper_forge::{Forge, ForgeError, Label, RepositoryId, UpsertLabel};

fn label(name: &str, color: Option<&str>, description: Option<&str>) -> UpsertLabel {
    UpsertLabel {
        name: name.into(),
        color: color.map(str::to_owned),
        description: description.map(str::to_owned),
    }
}

fn label_names(labels: &[Label]) -> Vec<String> {
    labels.iter().map(|label| label.name.clone()).collect()
}

#[test]
fn labels_are_empty_for_new_repository() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    assert_eq!(
        block_on(forge.list_labels(&repository.id)).unwrap(),
        Vec::new()
    );
}

#[test]
fn labels_can_be_created_and_reopened() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    let created = block_on(forge.upsert_label(
        &repository.id,
        label("bug", Some("ff0000"), Some("Something is broken")),
    ))
    .unwrap();

    assert_eq!(created.id.as_str(), "label-repo-0000000000000001-627567");
    assert_eq!(created.repo_id, repository.id);
    assert_eq!(created.name, "bug");
    assert_eq!(created.color.as_deref(), Some("ff0000"));
    assert_eq!(created.description.as_deref(), Some("Something is broken"));

    let reopened = root.forge();
    assert_eq!(
        block_on(reopened.list_labels(&repository.id)).unwrap(),
        vec![created]
    );
}

#[test]
fn upserting_existing_label_updates_by_name() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    let created =
        block_on(forge.upsert_label(&repository.id, label("bug", Some("ff0000"), None))).unwrap();
    let updated = block_on(forge.upsert_label(
        &repository.id,
        label("bug", Some("00ff00"), Some("Updated description")),
    ))
    .unwrap();

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.name, created.name);
    assert_eq!(updated.color.as_deref(), Some("00ff00"));
    assert_eq!(updated.description.as_deref(), Some("Updated description"));
    assert_eq!(
        block_on(forge.list_labels(&repository.id)).unwrap(),
        vec![updated]
    );
}

#[test]
fn labels_are_scoped_to_repositories() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let first = block_on(forge.create_repository(repository("alice", "first"))).unwrap();
    let second = block_on(forge.create_repository(repository("alice", "second"))).unwrap();

    let first_label = block_on(forge.upsert_label(&first.id, label("bug", None, None))).unwrap();
    let second_label = block_on(forge.upsert_label(&second.id, label("bug", None, None))).unwrap();

    assert_ne!(first_label.id, second_label.id);
    assert_eq!(first_label.repo_id, first.id);
    assert_eq!(second_label.repo_id, second.id);
    assert_eq!(
        block_on(forge.list_labels(&first.id)).unwrap(),
        vec![first_label]
    );
    assert_eq!(
        block_on(forge.list_labels(&second.id)).unwrap(),
        vec![second_label]
    );
}

#[test]
fn label_operations_return_not_found_for_missing_repository() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let missing = RepositoryId::new("repo-0000000000009999");

    let list_error = block_on(forge.list_labels(&missing)).unwrap_err();
    assert!(matches!(
        list_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));

    let upsert_error =
        block_on(forge.upsert_label(&missing, label("bug", None, None))).unwrap_err();
    assert!(matches!(
        upsert_error,
        ForgeError::NotFound(message) if message == "repository repo-0000000000009999"
    ));
}

#[test]
fn label_lists_are_sorted_deterministically_by_name() {
    let root = TestRoot::new("labels");
    let forge = root.forge();
    let repository = block_on(forge.create_repository(repository("alice", "project"))).unwrap();

    block_on(forge.upsert_label(&repository.id, label("zeta", None, None))).unwrap();
    block_on(forge.upsert_label(&repository.id, label("alpha", None, None))).unwrap();
    block_on(forge.upsert_label(&repository.id, label("beta", None, None))).unwrap();

    let labels = block_on(forge.list_labels(&repository.id)).unwrap();

    assert_eq!(label_names(&labels), vec!["alpha", "beta", "zeta"]);
}
