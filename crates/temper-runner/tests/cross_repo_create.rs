use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use temper_forge::{CreateIssue, CreateRepository, Forge, IssueQuery, ItemNumber, RepositoryId};
use temper_forge_filesystem::FilesystemForge;
use temper_forge_memory::MemoryForge;
use temper_runner::RoleTools;
use temper_testing::{block_on, workflow};
use temper_workflow::{
    global_child_correlation_key, parse_metadata_block, ArtifactRef, EnsureOutcome,
    ExecutionContext, RoleId,
};

static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new() -> Self {
        let id = NEXT_TEMP_ROOT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "temper-runner-cross-repo-create-{}-{id}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self { path }
    }

    fn forge(&self) -> FilesystemForge {
        FilesystemForge::new(&self.path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn memory_ensure_issue_in_another_repo_is_repeatable() {
    let forge = MemoryForge::new();
    let source_repo = memory_repo(&forge, "source");
    let target_repo = memory_repo(&forge, "target");
    let parent = create_issue(&forge, &source_repo, "Parent", "parent body");
    let workflow = workflow();
    let tools = RoleTools::new(
        &workflow,
        &forge,
        &source_repo,
        RoleId::new("architect"),
        ExecutionContext::default(),
    );
    let key = global_child_correlation_key(&source_repo, parent, "target-child");

    let first = block_on(tools.ensure_issue_in_repo(
        &target_repo,
        &key,
        ArtifactRef::same_repo(parent),
        issue_input("Child", "child body"),
    ))
    .expect("first ensure succeeds");
    let second = block_on(tools.ensure_issue_in_repo(
        &target_repo,
        &key,
        ArtifactRef::same_repo(parent),
        issue_input("Child", "child body"),
    ))
    .expect("second ensure succeeds");

    assert!(matches!(&first, EnsureOutcome::Created(_)));
    assert!(matches!(&second, EnsureOutcome::Existing(_)));
    assert_eq!(first.artifact().number, second.artifact().number);
    assert_single_correlated_child(&forge, &target_repo, &key, &source_repo, parent);
}

#[test]
fn memory_existing_correlated_issue_is_found_and_gets_parent_backref() {
    let forge = MemoryForge::new();
    let source_repo = memory_repo(&forge, "source");
    let target_repo = memory_repo(&forge, "target");
    let parent = create_issue(&forge, &source_repo, "Parent", "parent body");
    let key = global_child_correlation_key(&source_repo, parent, "preexisting");
    let existing = block_on(forge.create_issue(
        &target_repo,
        CreateIssue {
            title: "Existing child".into(),
            body: format!(
                "body\n\n{}",
                temper_workflow::render_metadata_block(&temper_workflow::WorkflowMetadata {
                    correlation_key: Some(key.clone()),
                    ..temper_workflow::WorkflowMetadata::default()
                })
            ),
            labels: Vec::new(),
            assignees: Vec::new(),
        },
    ))
    .expect("existing issue created");
    let workflow = workflow();
    let tools = RoleTools::new(
        &workflow,
        &forge,
        &source_repo,
        RoleId::new("architect"),
        ExecutionContext::default(),
    );

    let outcome = block_on(tools.ensure_issue_in_repo(
        &target_repo,
        &key,
        ArtifactRef::same_repo(parent),
        issue_input("Child", "child body"),
    ))
    .expect("ensure succeeds");

    assert!(matches!(&outcome, EnsureOutcome::Existing(_)));
    assert_eq!(outcome.artifact().number, existing.number);
    assert_single_correlated_child(&forge, &target_repo, &key, &source_repo, parent);
}

#[test]
fn memory_missing_target_repo_reports_permission_or_visibility_error() {
    let forge = MemoryForge::new();
    let source_repo = memory_repo(&forge, "source");
    let workflow = workflow();
    let tools = RoleTools::new(
        &workflow,
        &forge,
        &source_repo,
        RoleId::new("architect"),
        ExecutionContext::default(),
    );

    let error = block_on(tools.ensure_issue_in_repo(
        &RepositoryId::new("repo-missing"),
        "key",
        ArtifactRef::same_repo(ItemNumber::new(1)),
        issue_input("Child", "child body"),
    ))
    .expect_err("missing repo fails");

    assert!(error.to_string().contains("cannot write target repository"));
    assert!(error.to_string().contains("repo-missing"));
}

#[test]
fn filesystem_distinct_handles_racing_converge_on_one_child_issue() {
    let root = TempRoot::new();
    let setup = root.forge();
    let source_repo = filesystem_repo(&setup, "source");
    let target_repo = filesystem_repo(&setup, "target");
    let parent = create_issue(&setup, &source_repo, "Parent", "parent body");
    let key = global_child_correlation_key(&source_repo, parent, "raced-child");
    let workflow = workflow();
    let barrier = Arc::new(Barrier::new(2));

    thread::scope(|scope| {
        let mut handles = Vec::new();
        for index in 0..2 {
            let barrier = Arc::clone(&barrier);
            let path = root.path.clone();
            let source_repo = source_repo.clone();
            let target_repo = target_repo.clone();
            let key = key.clone();
            let workflow = &workflow;
            handles.push(scope.spawn(move || {
                let forge = FilesystemForge::new(&path);
                let tools = RoleTools::new(
                    workflow,
                    &forge,
                    &source_repo,
                    RoleId::new("architect"),
                    ExecutionContext::default(),
                );
                barrier.wait();
                block_on(tools.ensure_issue_in_repo(
                    &target_repo,
                    &key,
                    ArtifactRef::same_repo(parent),
                    issue_input(format!("Child {index}"), "child body"),
                ))
                .expect("ensure succeeds")
                .into_artifact()
                .number
            }));
        }
        let mut numbers: Vec<ItemNumber> = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker joins"))
            .collect();
        numbers.sort();
        numbers.dedup();
        assert_eq!(numbers.len(), 1, "both workers resolved the same issue");
    });

    assert_single_correlated_child(&setup, &target_repo, &key, &source_repo, parent);
}

fn memory_repo(forge: &MemoryForge, name: &str) -> RepositoryId {
    block_on(forge.create_repository(repo_input(name)))
        .expect("repository created")
        .id
}

fn filesystem_repo(forge: &FilesystemForge, name: &str) -> RepositoryId {
    block_on(forge.create_repository(repo_input(name)))
        .expect("repository created")
        .id
}

fn repo_input(name: &str) -> CreateRepository {
    CreateRepository {
        owner: "acme".into(),
        name: name.into(),
        default_branch: "main".into(),
        description: None,
    }
}

fn issue_input(title: impl Into<String>, body: impl Into<String>) -> CreateIssue {
    CreateIssue {
        title: title.into(),
        body: body.into(),
        labels: Vec::new(),
        assignees: Vec::new(),
    }
}

fn create_issue<F: Forge + ?Sized>(
    forge: &F,
    repo: &RepositoryId,
    title: &str,
    body: &str,
) -> ItemNumber {
    block_on(forge.create_issue(repo, issue_input(title, body)))
        .expect("issue created")
        .number
}

fn assert_single_correlated_child<F: Forge + ?Sized>(
    forge: &F,
    target_repo: &RepositoryId,
    key: &str,
    source_repo: &RepositoryId,
    parent: ItemNumber,
) {
    let issues = block_on(forge.list_issues(target_repo, IssueQuery::default()))
        .expect("issues list succeeds");
    let matching: Vec<_> = issues
        .iter()
        .filter(|issue| {
            parse_metadata_block(&issue.body)
                .ok()
                .flatten()
                .and_then(|metadata| metadata.correlation_key)
                .as_deref()
                == Some(key)
        })
        .collect();
    assert_eq!(matching.len(), 1, "exactly one issue carries the key");
    let metadata = parse_metadata_block(&matching[0].body)
        .expect("metadata parses")
        .expect("metadata exists");
    assert!(metadata
        .parents
        .contains(&ArtifactRef::in_repo(source_repo.clone(), parent)));
}
